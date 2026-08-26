use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use qrcode::{QrCode, render::svg};
use reqwest::{Client, RequestBuilder, StatusCode, Url, header::HeaderMap, redirect};
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
    base_url: String,
    created_at: Instant,
    poll_in_flight: bool,
    phase: WechatClawLoginPhase,
}

#[derive(Debug, Clone)]
enum WechatClawLoginPhase {
    Qr {
        qr_code: String,
    },
    Activating {
        bot_token: String,
        recipient_id: String,
        get_updates_buf: String,
        notify_started: bool,
    },
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
            base_url: ILINK_BASE_URL.to_string(),
            created_at: Instant::now(),
            poll_in_flight: false,
            phase: WechatClawLoginPhase::Qr {
                qr_code: qr_code.clone(),
            },
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
    let (base_url, phase) = {
        let mut logins = state.wechat_claw_logins.lock().await;
        logins.remove_expired();
        let Some(session) = logins.sessions.get_mut(&login_id) else {
            return Ok(json!({
                "status": "expired",
                "message": "二维码已过期，请重新开始扫码",
            }));
        };
        if session.poll_in_flight {
            return Ok(pending_login_response(&session.phase));
        }
        session.poll_in_flight = true;
        (session.base_url.clone(), session.phase.clone())
    };

    let result = match phase {
        WechatClawLoginPhase::Qr { qr_code } => {
            poll_wechat_claw_qr(state, &login_id, qr_code, base_url).await
        }
        WechatClawLoginPhase::Activating {
            bot_token,
            recipient_id,
            get_updates_buf,
            notify_started,
        } => {
            poll_wechat_claw_activation(
                state,
                &login_id,
                base_url,
                bot_token,
                recipient_id,
                get_updates_buf,
                notify_started,
            )
            .await
        }
    };
    if let Some(session) = state
        .wechat_claw_logins
        .lock()
        .await
        .sessions
        .get_mut(&login_id)
    {
        session.poll_in_flight = false;
    }
    result
}

async fn poll_wechat_claw_qr(
    state: &AppState,
    login_id: &str,
    qr_code: String,
    base_url: String,
) -> Result<Value, String> {
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
            if let Some(session) = logins.sessions.get_mut(login_id) {
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
                .unwrap_or_default()
                .to_string();
            let mut logins = state.wechat_claw_logins.lock().await;
            let Some(session) = logins.sessions.get_mut(login_id) else {
                return Ok(json!({
                    "status": "expired",
                    "message": "激活已过期，请重新开始扫码",
                }));
            };
            session.base_url = confirmed_base_url;
            session.created_at = Instant::now();
            session.phase = WechatClawLoginPhase::Activating {
                bot_token: token,
                recipient_id,
                get_updates_buf: String::new(),
                notify_started: false,
            };
            Ok(json!({
                "status": "activating",
                "message": "扫码已确认。请在微信中打开 ClawBot，并发送一条消息完成激活。",
            }))
        }
        "expired" => {
            state
                .wechat_claw_logins
                .lock()
                .await
                .sessions
                .remove(login_id);
            Ok(json!({"status":"expired", "message":"二维码已过期，请重新开始扫码"}))
        }
        _ => {
            state
                .wechat_claw_logins
                .lock()
                .await
                .sessions
                .remove(login_id);
            Ok(json!({"status":"failed", "message":"微信 ClawBot 登录未完成，请重新开始扫码"}))
        }
    }
}

fn pending_login_response(phase: &WechatClawLoginPhase) -> Value {
    match phase {
        WechatClawLoginPhase::Qr { .. } => json!({"status":"wait"}),
        WechatClawLoginPhase::Activating { .. } => json!({
            "status": "activating",
            "message": "正在等待微信消息完成 ClawBot 激活。",
        }),
    }
}

#[derive(Debug)]
enum ActivationRequestError {
    Retryable,
    Fatal(String),
}

#[derive(Debug, Clone, Copy)]
enum ActivationResponseContract {
    Strict,
    GetUpdates,
}

async fn poll_wechat_claw_activation(
    state: &AppState,
    login_id: &str,
    base_url: String,
    bot_token: String,
    recipient_id: String,
    get_updates_buf: String,
    notify_started: bool,
) -> Result<Value, String> {
    if !notify_started {
        let request =
            notify_start_request(&state.wechat_claw_login_http_client, &base_url, &bot_token)?;
        match activation_response_json(request, "激活", ActivationResponseContract::Strict).await
        {
            Ok(_) => {
                let mut logins = state.wechat_claw_logins.lock().await;
                if let Some(PendingWechatClawLogin {
                    phase: WechatClawLoginPhase::Activating { notify_started, .. },
                    ..
                }) = logins.sessions.get_mut(login_id)
                {
                    *notify_started = true;
                }
            }
            Err(ActivationRequestError::Retryable) => {
                return Ok(activation_retry_response());
            }
            Err(ActivationRequestError::Fatal(message)) => {
                return Ok(fail_activation(state, login_id, message).await);
            }
        }
    }

    let request = get_updates_request(
        &state.wechat_claw_login_http_client,
        &base_url,
        &bot_token,
        &get_updates_buf,
    )?;
    let payload =
        match activation_response_json(request, "消息同步", ActivationResponseContract::GetUpdates)
            .await
        {
            Ok(payload) => payload,
            Err(ActivationRequestError::Retryable) => return Ok(activation_retry_response()),
            Err(ActivationRequestError::Fatal(message)) => {
                return Ok(fail_activation(state, login_id, message).await);
            }
        };
    let next_updates_buf = response_updates_buffer(&payload)
        .unwrap_or(get_updates_buf.as_str())
        .to_string();

    if let Some((from_user_id, context_token)) = activation_context(&payload, &recipient_id) {
        state
            .wechat_claw_logins
            .lock()
            .await
            .sessions
            .remove(login_id);
        return Ok(json!({
            "status": "confirmed",
            "baseUrl": base_url,
            "botToken": bot_token,
            "recipientId": from_user_id,
            "contextToken": context_token,
        }));
    }

    let mut logins = state.wechat_claw_logins.lock().await;
    if let Some(PendingWechatClawLogin {
        phase: WechatClawLoginPhase::Activating {
            get_updates_buf, ..
        },
        ..
    }) = logins.sessions.get_mut(login_id)
    {
        *get_updates_buf = next_updates_buf;
    }
    Ok(json!({
        "status": "activating",
        "message": "请在微信中打开 ClawBot，并发送一条消息完成激活。",
    }))
}

fn activation_retry_response() -> Value {
    json!({
        "status": "activating",
        "message": "微信 ClawBot 激活服务暂时无响应，正在自动重试；请保持当前页面打开。",
    })
}

async fn fail_activation(state: &AppState, login_id: &str, message: String) -> Value {
    state
        .wechat_claw_logins
        .lock()
        .await
        .sessions
        .remove(login_id);
    json!({"status":"failed", "message":message})
}

fn notify_start_request(
    client: &Client,
    base_url: &str,
    bot_token: &str,
) -> Result<RequestBuilder, String> {
    ilink_post_request(
        client,
        base_url,
        bot_token,
        "ilink/bot/msg/notifystart",
        json!({"base_info": wechat_claw_base_info()}),
    )
}

fn get_updates_request(
    client: &Client,
    base_url: &str,
    bot_token: &str,
    get_updates_buf: &str,
) -> Result<RequestBuilder, String> {
    ilink_post_request(
        client,
        base_url,
        bot_token,
        "ilink/bot/getupdates",
        json!({
            "get_updates_buf": get_updates_buf,
            "base_info": wechat_claw_base_info(),
        }),
    )
}

fn ilink_post_request(
    client: &Client,
    base_url: &str,
    bot_token: &str,
    endpoint: &str,
    body: Value,
) -> Result<RequestBuilder, String> {
    Ok(client
        .post(endpoint_url(base_url, endpoint)?)
        .headers(ilink_headers(Some(bot_token)))
        .json(&body))
}

async fn activation_response_json(
    request: RequestBuilder,
    action: &str,
    contract: ActivationResponseContract,
) -> Result<Value, ActivationRequestError> {
    let response = request
        .send()
        .await
        .map_err(|_| ActivationRequestError::Retryable)?;
    let status = response.status();
    if !status.is_success() {
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(ActivationRequestError::Retryable);
        }
        return Err(ActivationRequestError::Fatal(format!(
            "微信 ClawBot {action}服务返回 HTTP {status}，请重新扫码"
        )));
    }
    let payload = response.json::<Value>().await.map_err(|_| {
        ActivationRequestError::Fatal(format!(
            "微信 ClawBot {action}服务返回了无法解析的响应，请重新扫码"
        ))
    })?;
    validate_activation_response(&payload, action, contract)
        .map_err(ActivationRequestError::Fatal)?;
    Ok(payload)
}

fn validate_activation_response(
    payload: &Value,
    action: &str,
    contract: ActivationResponseContract,
) -> Result<(), String> {
    let message = remote_error_message(payload);
    if let Some(result) = response_code(payload, "ret") {
        if result != 0 {
            return Err(format!(
                "微信 ClawBot {action}失败（{result}）：{}",
                bounded_remote_message(message)
            ));
        }
    } else if matches!(contract, ActivationResponseContract::Strict) {
        return Err(format!(
            "微信 ClawBot {action}服务没有返回明确结果，请重新扫码"
        ));
    }

    for key in ["errcode", "err_code"] {
        if let Some(errcode) = response_code(payload, key) {
            if errcode != 0 {
                return Err(format!(
                    "微信 ClawBot {action}失败（{errcode}）：{}",
                    bounded_remote_message(message)
                ));
            }
        }
    }
    Ok(())
}

fn activation_context(payload: &Value, expected_recipient_id: &str) -> Option<(String, String)> {
    let messages = response_messages(payload)?;
    let expected = expected_recipient_id.trim();
    messages.iter().find_map(|message| {
        let from_user_id = message_string(message, "from_user_id")?;
        if !expected.is_empty() && from_user_id != expected {
            return None;
        }
        let context_token = message_string(message, "context_token")?;
        Some((from_user_id.to_string(), context_token.to_string()))
    })
}

fn message_string<'a>(message: &'a Value, field: &str) -> Option<&'a str> {
    message
        .get(field)
        .and_then(Value::as_str)
        .or_else(|| {
            message
                .get("msg")
                .and_then(|nested| nested.get(field))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn response_updates_buffer(payload: &Value) -> Option<&str> {
    response_string(payload, &["get_updates_buf", "sync_buf"])
}

fn response_messages(payload: &Value) -> Option<&Vec<Value>> {
    for key in ["msgs", "messages", "message_list", "updates"] {
        if let Some(messages) = payload.get(key).and_then(Value::as_array) {
            return Some(messages);
        }
    }
    for key in ["data", "result", "body", "payload"] {
        if let Some(messages) = payload.get(key).and_then(response_messages) {
            return Some(messages);
        }
    }
    None
}

fn response_string<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(value) = payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    for key in ["data", "result", "body", "payload"] {
        if let Some(value) = payload
            .get(key)
            .and_then(|nested| response_string(nested, keys))
        {
            return Some(value);
        }
    }
    None
}

fn response_code(payload: &Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
    })
}

fn remote_error_message(payload: &Value) -> &str {
    response_string(payload, &["errmsg", "error_message", "err_msg", "message"])
        .unwrap_or("未知错误")
}

fn bounded_remote_message(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = normalized.chars().take(160).collect::<String>();
    if value.is_empty() {
        "未知错误".to_string()
    } else {
        value
    }
}

fn wechat_claw_base_info() -> Value {
    json!({
        "channel_version": env!("CARGO_PKG_VERSION"),
        "bot_agent": format!("Codey/{}", env!("CARGO_PKG_VERSION")),
    })
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
                base_url: ILINK_BASE_URL.to_string(),
                created_at: Instant::now() - LOGIN_TIMEOUT,
                poll_in_flight: false,
                phase: WechatClawLoginPhase::Qr {
                    qr_code: "qr".to_string(),
                },
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

    #[test]
    fn activation_requests_use_notify_start_then_buffered_get_updates() {
        let client = Client::new();
        let notify = notify_start_request(&client, ILINK_BASE_URL, "secret")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            notify.url().as_str(),
            "https://ilinkai.weixin.qq.com/ilink/bot/msg/notifystart"
        );
        assert_eq!(notify.headers()["authorization"], "Bearer secret");

        let updates = get_updates_request(&client, ILINK_BASE_URL, "secret", "next-buffer")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            updates.url().as_str(),
            "https://ilinkai.weixin.qq.com/ilink/bot/getupdates"
        );
        let body = updates
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .unwrap();
        assert_eq!(body["get_updates_buf"], "next-buffer");
        assert!(
            body["base_info"]["bot_agent"]
                .as_str()
                .is_some_and(|value| value.starts_with("Codey/"))
        );
    }

    #[test]
    fn activation_only_accepts_an_inbound_context_for_the_bound_user() {
        let payload = json!({
            "ret": 0,
            "get_updates_buf": "next",
            "msgs": [
                {"from_user_id":"other@im.wechat", "context_token":"other-context"},
                {"from_user_id":"user@im.wechat", "context_token":"user-context"}
            ]
        });

        assert_eq!(
            activation_context(&payload, "user@im.wechat"),
            Some(("user@im.wechat".to_string(), "user-context".to_string()))
        );
        assert_eq!(response_updates_buffer(&payload), Some("next"));
        assert_eq!(
            activation_context(&payload, ""),
            Some(("other@im.wechat".to_string(), "other-context".to_string()))
        );
    }

    #[test]
    fn activation_parses_nested_messages_and_legacy_sync_buffers() {
        let payload = json!({
            "ret": 0,
            "data": {
                "sync_buf": "legacy-next",
                "updates": [{
                    "msg": {
                        "from_user_id": "user@im.wechat",
                        "context_token": "nested-context"
                    }
                }]
            }
        });

        assert_eq!(
            activation_context(&payload, "user@im.wechat"),
            Some(("user@im.wechat".to_string(), "nested-context".to_string()))
        );
        assert_eq!(response_updates_buffer(&payload), Some("legacy-next"));
    }

    #[test]
    fn notify_start_responses_require_explicit_success_fields() {
        assert!(
            validate_activation_response(
                &json!({"ret":0}),
                "激活",
                ActivationResponseContract::Strict
            )
            .is_ok()
        );
        assert!(
            validate_activation_response(
                &json!({"ret":0,"errcode":0}),
                "激活",
                ActivationResponseContract::Strict,
            )
            .is_ok()
        );
        assert!(
            validate_activation_response(&json!({}), "激活", ActivationResponseContract::Strict)
                .is_err()
        );
        assert!(
            validate_activation_response(
                &json!({"ret":-2,"errmsg":"prepare failed"}),
                "激活",
                ActivationResponseContract::Strict,
            )
            .is_err()
        );
    }

    #[test]
    fn get_updates_responses_allow_empty_long_poll_results() {
        for payload in [
            json!({}),
            json!({"get_updates_buf":"next"}),
            json!({"ret":0}),
            json!({"err_code":0,"messages":[]}),
            json!({"data":{"sync_buf":"nested-next","updates":[]}}),
        ] {
            assert!(
                validate_activation_response(
                    &payload,
                    "消息同步",
                    ActivationResponseContract::GetUpdates,
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn get_updates_responses_reject_explicit_remote_errors() {
        assert!(
            validate_activation_response(
                &json!({"ret":-14,"errmsg":"token expired"}),
                "消息同步",
                ActivationResponseContract::GetUpdates,
            )
            .is_err()
        );
        assert!(
            validate_activation_response(
                &json!({"err_code":"-2","err_msg":"prepare failed"}),
                "消息同步",
                ActivationResponseContract::GetUpdates,
            )
            .is_err()
        );
    }
}
