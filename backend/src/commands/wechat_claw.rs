use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use qrcode::{QrCode, render::svg};
use reqwest::{Client, Url, header::HeaderMap, redirect};
use serde_json::{Value, json};
use uuid::Uuid;

use super::AppState;
use crate::notifications::NotificationChannelConfig;

const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// QR status is an iLink long-poll endpoint. Keep its client separate from
/// notification delivery so a temporary scan can wait efficiently without
/// widening the short timeout used for one-way notifications.
pub(super) fn wechat_claw_login_http_client() -> Client {
    Client::builder()
        .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(40))
        .redirect(redirect::Policy::none())
        .build()
        .expect("create WeChat ClawBot login HTTP client")
}

#[derive(Debug, Default)]
pub(super) struct WechatClawLoginState {
    sessions: HashMap<String, PendingWechatClawLogin>,
}

#[derive(Debug)]
struct PendingWechatClawLogin {
    qr_code: String,
    base_url: String,
    created_at: Instant,
}

pub(super) async fn start_wechat_claw_login(state: &AppState) -> Result<Value, String> {
    let response = get_bot_qrcode_request(&state.wechat_claw_login_http_client)?
        .send()
        .await
        .map_err(|_| "无法连接微信 ClawBot 登录服务，请检查网络后重试".to_string())?;
    let payload = login_response_json(response).await?;
    let qr_code = payload
        .get("qrcode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "微信 ClawBot 登录服务没有返回二维码，请重新开始扫码".to_string())?
        .to_string();
    // `qrcode` is the opaque status-poll token. `qrcode_img_content` is the URL
    // that must be encoded into the scannable image; it is not an image URL.
    // Generate it locally to avoid another request and webview policy differences.
    let qr_code_image_src = qr_code_image_data_uri(qr_code_scan_payload(&payload)?)?;
    let login_id = Uuid::new_v4().to_string();
    let mut logins = state.wechat_claw_logins.lock().await;
    logins.remove_expired();
    logins.sessions.insert(
        login_id.clone(),
        PendingWechatClawLogin {
            qr_code: qr_code.clone(),
            base_url: ILINK_BASE_URL.to_string(),
            created_at: Instant::now(),
        },
    );
    Ok(json!({
        "loginId": login_id,
        "status": "wait",
        "qrCode": qr_code,
        "qrCodeImageUrl": qr_code_image_src,
    }))
}

pub(super) async fn poll_wechat_claw_login(
    state: &AppState,
    login_id: String,
) -> Result<Value, String> {
    let (qr_code, base_url) = {
        let mut logins = state.wechat_claw_logins.lock().await;
        logins.remove_expired();
        let Some(session) = logins.sessions.get(&login_id) else {
            return Ok(json!({
                "status": "expired",
                "message": "二维码已过期，请重新开始扫码",
            }));
        };
        (session.qr_code.clone(), session.base_url.clone())
    };

    let url = endpoint_url(&base_url, "ilink/bot/get_qrcode_status")?;
    let response = state
        .wechat_claw_login_http_client
        .get(url)
        .query(&[("qrcode", qr_code)])
        .headers(ilink_headers(None))
        .send()
        .await
        .map_err(|_| "无法查询微信 ClawBot 扫码状态，请检查网络后重试".to_string())?;
    let payload = login_response_json(response).await?;
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("failed");

    match status {
        "wait" => Ok(json!({"status":"wait"})),
        "scaned" => Ok(json!({"status":"scanned", "message":"已扫码，请在微信中确认授权"})),
        "scaned_but_redirect" => {
            let Some(next_base_url) = redirect_base_url(&payload)? else {
                return Ok(
                    json!({"status":"failed", "message":"微信 ClawBot 返回了无效的登录地址，请重新开始扫码"}),
                );
            };
            let mut logins = state.wechat_claw_logins.lock().await;
            if let Some(session) = logins.sessions.get_mut(&login_id) {
                session.base_url = next_base_url;
            }
            Ok(json!({"status":"scanned", "message":"已扫码，请在微信中确认授权"}))
        }
        "confirmed" => {
            let token = payload
                .get("bot_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "扫码完成但没有获得有效凭据，请重新开始扫码".to_string())?
                .to_string();
            let confirmed_base_url = payload
                .get("baseurl")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(validate_base_url)
                .transpose()?
                .unwrap_or(base_url);
            let recipient_id = payload
                .get("ilink_user_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or_default();
            state
                .wechat_claw_logins
                .lock()
                .await
                .sessions
                .remove(&login_id);
            Ok(json!({
                "status": "confirmed",
                "baseUrl": confirmed_base_url,
                "botToken": token,
                "recipientId": recipient_id,
            }))
        }
        "expired" => {
            state
                .wechat_claw_logins
                .lock()
                .await
                .sessions
                .remove(&login_id);
            Ok(json!({"status":"expired", "message":"二维码已过期，请重新开始扫码"}))
        }
        _ => {
            state
                .wechat_claw_logins
                .lock()
                .await
                .sessions
                .remove(&login_id);
            Ok(json!({"status":"failed", "message":"微信 ClawBot 登录未完成，请重新开始扫码"}))
        }
    }
}

impl WechatClawLoginState {
    fn remove_expired(&mut self) {
        self.sessions
            .retain(|_, session| session.created_at.elapsed() < LOGIN_TIMEOUT);
    }
}

fn endpoint_url(base_url: &str, endpoint: &str) -> Result<Url, String> {
    let base_url = validate_base_url(base_url)?;
    Url::parse(&base_url)
        .map_err(|_| "微信 ClawBot 服务地址无效".to_string())?
        .join(endpoint)
        .map_err(|_| "微信 ClawBot 服务地址无效".to_string())
}

fn get_bot_qrcode_request(client: &Client) -> Result<reqwest::RequestBuilder, String> {
    let url = endpoint_url(ILINK_BASE_URL, "ilink/bot/get_bot_qrcode")?;
    Ok(client
        .post(url)
        .query(&[("bot_type", "3")])
        .headers(ilink_headers(None))
        // The official client accepts a list of known local bot tokens here.
        // Codey intentionally keeps this isolated notification binding stateless.
        .json(&json!({"local_token_list": []})))
}

fn redirect_base_url(payload: &Value) -> Result<Option<String>, String> {
    let Some(host) = payload
        .get("redirect_host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    validate_base_url(&format!("https://{host}"))
        .map(Some)
        .map_err(|_| "微信 ClawBot 返回了不受信任的登录地址".to_string())
}

fn validate_base_url(value: &str) -> Result<String, String> {
    let config = NotificationChannelConfig {
        url: value.trim().to_string(),
        ..NotificationChannelConfig::default()
    };
    config
        .wechat_claw_base_url()
        .map(|url| url.as_str().trim_end_matches('/').to_string())
        .map_err(ToString::to_string)
}

fn qr_code_scan_payload(payload: &Value) -> Result<&str, String> {
    let value = payload
        .get("qrcode_img_content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "微信 ClawBot 登录服务没有返回二维码内容，请重新开始扫码".to_string())?;
    let url = Url::parse(value)
        .map_err(|_| "微信 ClawBot 返回了无效的二维码内容，请重新开始扫码".to_string())?;
    let host = url.host_str().unwrap_or_default();
    let official_host = host == "weixin.qq.com" || host.ends_with(".weixin.qq.com");
    if url.scheme() != "https"
        || !official_host
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("微信 ClawBot 返回了不受信任的二维码内容，请重新开始扫码".to_string());
    }
    Ok(value)
}

fn qr_code_image_data_uri(value: &str) -> Result<String, String> {
    let code = QrCode::new(value.as_bytes())
        .map_err(|_| "无法生成微信 ClawBot 登录二维码，请重新开始扫码".to_string())?;
    let image = code.render::<svg::Color>().module_dimensions(1, 1).build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(image)
    ))
}

fn ilink_headers(token: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "AuthorizationType",
        "ilink_bot_token".parse().expect("static header"),
    );
    headers.insert(
        "X-WECHAT-UIN",
        random_wechat_uin().parse().expect("base64 header"),
    );
    headers.insert("iLink-App-Id", "bot".parse().expect("static header"));
    headers.insert(
        "iLink-App-ClientVersion",
        ilink_client_version().parse().expect("numeric header"),
    );
    if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
        headers.insert(
            "Authorization",
            format!("Bearer {token}").parse().expect("token header"),
        );
    }
    headers
}

fn random_wechat_uin() -> String {
    let uuid = Uuid::new_v4();
    let value = u32::from_be_bytes(uuid.as_bytes()[..4].try_into().expect("UUID prefix"));
    STANDARD.encode(value.to_string())
}

fn ilink_client_version() -> String {
    let mut components = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let major = components.next().unwrap_or(0) & 0xff;
    let minor = components.next().unwrap_or(0) & 0xff;
    let patch = components.next().unwrap_or(0) & 0xff;
    ((major << 16) | (minor << 8) | patch).to_string()
}

async fn login_response_json(response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "微信 ClawBot 登录服务返回 HTTP {}",
            response.status()
        ));
    }
    response
        .json::<Value>()
        .await
        .map_err(|_| "微信 ClawBot 登录服务返回了无法解析的响应".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_urls_are_pinned_to_official_https_hosts() {
        assert_eq!(
            validate_base_url("https://ilinkai.weixin.qq.com/").unwrap(),
            "https://ilinkai.weixin.qq.com"
        );
        assert!(validate_base_url("https://region.weixin.qq.com").is_ok());
        assert!(validate_base_url("http://ilinkai.weixin.qq.com").is_err());
        assert!(validate_base_url("https://weixin.qq.com.evil.example").is_err());
        assert!(validate_base_url("https://ilinkai.weixin.qq.com/path").is_err());
    }

    #[test]
    fn redirect_hosts_cannot_escape_the_official_domain() {
        assert_eq!(
            redirect_base_url(&json!({"redirect_host":"region.weixin.qq.com"})).unwrap(),
            Some("https://region.weixin.qq.com".to_string())
        );
        assert!(redirect_base_url(&json!({"redirect_host":"evil.example"})).is_err());
        assert!(
            redirect_base_url(&json!({"redirect_host":"region.weixin.qq.com/escape"})).is_err()
        );
    }

    #[test]
    fn qr_code_image_is_generated_locally_as_svg_data_uri() {
        let payload = json!({
            "qrcode": "opaque-status-poll-token",
            "qrcode_img_content": "https://login.weixin.qq.com/l/login-fixture"
        });
        let scan_payload = qr_code_scan_payload(&payload).unwrap();
        assert_eq!(scan_payload, "https://login.weixin.qq.com/l/login-fixture");

        let image = qr_code_image_data_uri(scan_payload).unwrap();
        let encoded = image
            .strip_prefix("data:image/svg+xml;base64,")
            .expect("SVG data URI");
        let svg = String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<path"));
        assert!(svg.contains("shape-rendering=\"crispEdges\""));
    }

    #[test]
    fn qr_code_scan_payload_requires_an_official_https_url() {
        for rejected in [
            json!({"qrcode_img_content":"http://login.weixin.qq.com/l/code"}),
            json!({"qrcode_img_content":"https://weixin.qq.com.evil.example/l/code"}),
            json!({"qrcode_img_content":"https://user@login.weixin.qq.com/l/code"}),
            json!({"qrcode_img_content":"opaque-status-poll-token"}),
        ] {
            assert!(qr_code_scan_payload(&rejected).is_err());
        }
    }

    #[test]
    fn login_state_expires_old_qr_codes() {
        let mut state = WechatClawLoginState::default();
        state.sessions.insert(
            "old".to_string(),
            PendingWechatClawLogin {
                qr_code: "qr".to_string(),
                base_url: ILINK_BASE_URL.to_string(),
                created_at: Instant::now() - LOGIN_TIMEOUT,
            },
        );
        state.remove_expired();
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn login_headers_include_the_required_ilink_identifiers() {
        let headers = ilink_headers(None);
        assert_eq!(headers["authorizationtype"], "ilink_bot_token");
        assert_eq!(headers["ilink-app-id"], "bot");
        assert!(headers.contains_key("x-wechat-uin"));
        assert!(!headers.contains_key("authorization"));
    }

    #[test]
    fn qr_code_request_uses_the_current_official_post_contract() {
        let request = get_bot_qrcode_request(&Client::new())
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://ilinkai.weixin.qq.com/ilink/bot/get_bot_qrcode?bot_type=3"
        );
        assert_eq!(request.headers()["authorizationtype"], "ilink_bot_token");
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .unwrap();
        assert_eq!(body["local_token_list"], json!([]));
    }
}
