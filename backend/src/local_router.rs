use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, Semaphore, oneshot};
use uuid::Uuid;

use crate::config::CodeyConfig;

pub(crate) const ROUTER_PROVIDER_ID: &str = "codey_router";

const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_CONCURRENT_CONNECTIONS: usize = 16;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub(crate) struct RuntimeRouterEndpoint {
    pub base_url: String,
    pub token: String,
}

pub(crate) struct LocalRouter {
    endpoint: RuntimeRouterEndpoint,
    snapshot: Arc<RwLock<RouterSnapshot>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl LocalRouter {
    pub(crate) async fn start(config: &CodeyConfig) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("启动 Codey 本地路由失败")?;
        let port = listener
            .local_addr()
            .context("读取 Codey 本地路由监听地址失败")?
            .port();
        let token = format!("codey-router-{}", Uuid::new_v4());
        let endpoint = RuntimeRouterEndpoint {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            token,
        };
        let snapshot = Arc::new(RwLock::new(RouterSnapshot::from_config(config)));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let server = RouterServer {
            token: endpoint.token.clone(),
            snapshot: Arc::clone(&snapshot),
            connection_limit: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
            client: reqwest::Client::builder()
                .user_agent(format!("Codey-Router/{}", env!("CARGO_PKG_VERSION")))
                .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
                .build()
                .context("创建 Codey 本地路由 HTTP 客户端失败")?,
        };
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, _)) => {
                                let server = server.clone();
                                tokio::spawn(async move {
                                    let permit = match Arc::clone(&server.connection_limit)
                                        .try_acquire_owned()
                                    {
                                        Ok(permit) => permit,
                                        Err(_) => {
                                            let mut stream = stream;
                                            let _ = write_error_response(
                                                &mut stream,
                                                503,
                                                "router_busy",
                                                "Codey 本地路由当前请求过多，请稍后重试",
                                                None,
                                            )
                                            .await;
                                            return;
                                        }
                                    };
                                    let _permit = permit;
                                    if let Err(error) = server.handle_connection(stream).await {
                                        crate::error_log::record_failure(
                                            "local_router_request_failed",
                                            "handle_local_router_connection",
                                            format!("{error:#}"),
                                            serde_json::json!({}),
                                        );
                                    }
                                });
                            }
                            Err(error) => {
                                crate::error_log::record_failure(
                                    "local_router_accept_failed",
                                    "accept_local_router_connection",
                                    error.to_string(),
                                    serde_json::json!({}),
                                );
                                break;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });
        Ok(Self {
            endpoint,
            snapshot,
            shutdown: Mutex::new(Some(shutdown_tx)),
            task: Mutex::new(Some(task)),
        })
    }

    pub(crate) fn endpoint(&self) -> RuntimeRouterEndpoint {
        self.endpoint.clone()
    }

    pub(crate) async fn update_config(&self, config: &CodeyConfig) {
        *self.snapshot.write().await = RouterSnapshot::from_config(config);
    }

    pub(crate) async fn stop(&self) -> Result<()> {
        if let Some(shutdown) = self
            .shutdown
            .lock()
            .expect("local router shutdown mutex poisoned")
            .take()
        {
            let _ = shutdown.send(());
        }
        let task = self
            .task
            .lock()
            .expect("local router task mutex poisoned")
            .take();
        if let Some(task) = task {
            task.await.context("关闭 Codey 本地路由任务异常退出")?;
        }
        Ok(())
    }
}

impl Drop for LocalRouter {
    fn drop(&mut self) {
        if let Ok(mut shutdown) = self.shutdown.lock()
            && let Some(shutdown) = shutdown.take()
        {
            let _ = shutdown.send(());
        }
        if let Ok(mut task) = self.task.lock()
            && let Some(task) = task.take()
        {
            task.abort();
        }
    }
}

#[derive(Clone)]
struct RouterServer {
    token: String,
    snapshot: Arc<RwLock<RouterSnapshot>>,
    connection_limit: Arc<Semaphore>,
    client: reqwest::Client,
}

#[derive(Clone, Debug, Default)]
struct RouterSnapshot {
    routes: BTreeMap<String, RouteTarget>,
    aliases: BTreeMap<String, AliasTarget>,
}

impl RouterSnapshot {
    fn from_config(config: &CodeyConfig) -> Self {
        let mut routes = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        for profile in &config.profiles {
            if profile.cc_switch_read_only {
                continue;
            }
            let provider_id = profile.provider_id().trim();
            if provider_id.is_empty() {
                continue;
            }
            let base_url = profile.normalized_base_url();
            if base_url.is_empty() {
                continue;
            }
            let target = RouteTarget {
                provider_id: provider_id.to_string(),
                route_name: profile.name.trim().to_string(),
                base_url,
                api_key: profile.api_key.trim().to_string(),
                headers: profile.model_request_headers.clone(),
            };
            for model in route_models(config, provider_id) {
                aliases.insert(
                    model_alias(provider_id, &model),
                    AliasTarget {
                        provider_id: provider_id.to_string(),
                        model,
                    },
                );
            }
            routes.insert(provider_id.to_string(), target);
        }
        Self { routes, aliases }
    }

    fn target_for_model(&self, requested_model: &str) -> Result<ResolvedTarget> {
        let requested_model = requested_model.trim();
        if requested_model.is_empty() {
            anyhow::bail!("请求缺少 model 字段");
        }
        if let Some(alias) = self.aliases.get(requested_model) {
            let target = self
                .routes
                .get(&alias.provider_id)
                .ok_or_else(|| anyhow::anyhow!("线路已不存在：{}", alias.provider_id))?;
            return Ok(ResolvedTarget {
                route: target.clone(),
                upstream_model: alias.model.clone(),
            });
        }
        anyhow::bail!("模型未在线路路由表中启用：{requested_model}")
    }

    fn model_aliases(&self) -> Vec<String> {
        self.aliases.keys().cloned().collect()
    }
}

#[derive(Clone, Debug)]
struct RouteTarget {
    provider_id: String,
    route_name: String,
    base_url: String,
    api_key: String,
    headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct AliasTarget {
    provider_id: String,
    model: String,
}

#[derive(Clone, Debug)]
struct ResolvedTarget {
    route: RouteTarget,
    upstream_model: String,
}

fn route_models(config: &CodeyConfig, provider_id: &str) -> Vec<String> {
    config.enabled_route_models(provider_id)
}

pub(crate) fn model_alias(provider_id: &str, model: &str) -> String {
    format!("{}/{}", encode_alias_component(provider_id), model.trim())
}

fn encode_alias_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.trim().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

impl RouterServer {
    async fn handle_connection(&self, mut stream: TcpStream) -> Result<()> {
        let request = match tokio::time::timeout(
            REQUEST_READ_TIMEOUT,
            read_http_request(&mut stream),
        )
        .await
        {
            Ok(Ok(request)) => request,
            Ok(Err(error)) => {
                write_error_response(
                    &mut stream,
                    400,
                    "invalid_http_request",
                    format!("本地路由请求无效：{error:#}"),
                    None,
                )
                .await?;
                return Ok(());
            }
            Err(_) => {
                write_error_response(
                    &mut stream,
                    408,
                    "request_timeout",
                    "读取本地路由请求超时",
                    None,
                )
                .await?;
                return Ok(());
            }
        };
        if request.path == "/healthz" {
            write_json_response(&mut stream, 200, &json!({"status":"ok"})).await?;
            return Ok(());
        }
        if !self.authorized(&request) {
            write_error_response(
                &mut stream,
                401,
                "invalid_router_token",
                "Codey 本地路由认证失败",
                None,
            )
            .await?;
            return Ok(());
        }
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/v1/models") | ("GET", "/models") => {
                let models = self.snapshot.read().await.model_aliases();
                let data = models
                    .into_iter()
                    .map(|id| json!({"id":id,"object":"model","owned_by":"codey"}))
                    .collect::<Vec<_>>();
                write_json_response(&mut stream, 200, &json!({"object":"list","data":data}))
                    .await?;
            }
            ("POST", "/v1/responses") | ("POST", "/responses") => {
                self.proxy_responses(request, stream).await?;
            }
            _ => {
                write_error_response(
                    &mut stream,
                    404,
                    "route_not_found",
                    "Codey 本地路由不支持该路径",
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }

    fn authorized(&self, request: &HttpRequest) -> bool {
        let expected = format!("Bearer {}", self.token);
        request.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && constant_time_eq(value.trim().as_bytes(), expected.as_bytes())
        })
    }

    async fn proxy_responses(&self, request: HttpRequest, mut stream: TcpStream) -> Result<()> {
        let mut body = match serde_json::from_slice::<Value>(&request.body) {
            Ok(body) if body.is_object() => body,
            Ok(_) => {
                write_error_response(
                    &mut stream,
                    400,
                    "invalid_request_body",
                    "Responses 请求体必须是 JSON 对象",
                    None,
                )
                .await?;
                return Ok(());
            }
            Err(error) => {
                write_error_response(
                    &mut stream,
                    400,
                    "invalid_request_body",
                    format!("Responses 请求体不是有效 JSON：{error}"),
                    None,
                )
                .await?;
                return Ok(());
            }
        };
        let model = body
            .as_object()
            .and_then(|body| body.get("model"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if model.is_empty() {
            write_error_response(
                &mut stream,
                400,
                "model_required",
                "Responses 请求缺少有效的 model 字段",
                None,
            )
            .await?;
            return Ok(());
        }
        let resolved = match self.snapshot.read().await.target_for_model(&model) {
            Ok(resolved) => resolved,
            Err(error) => {
                write_error_response(
                    &mut stream,
                    404,
                    "model_not_enabled",
                    format!("{error:#}"),
                    None,
                )
                .await?;
                return Ok(());
            }
        };
        body.as_object_mut()
            .expect("validated Responses body must remain an object")
            .insert(
                "model".to_string(),
                Value::String(resolved.upstream_model.clone()),
            );
        let upstream_url = match responses_endpoint(&resolved.route.base_url) {
            Ok(url) => url,
            Err(error) => {
                write_error_response(
                    &mut stream,
                    502,
                    "route_configuration_error",
                    format!(
                        "线路「{}」的 API URL 无效：{error:#}",
                        resolved.route.route_name
                    ),
                    Some(&resolved.route),
                )
                .await?;
                return Ok(());
            }
        };
        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            if should_forward_incoming_header(name)
                && let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                )
            {
                headers.insert(name, value);
            }
        }
        let has_custom_authorization =
            resolved.route.headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("authorization") && !value.is_empty()
            });
        if !resolved.route.api_key.is_empty() && !has_custom_authorization {
            let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", resolved.route.api_key))
            else {
                write_error_response(
                    &mut stream,
                    502,
                    "route_configuration_error",
                    format!("线路「{}」的 API Key 格式无效", resolved.route.route_name),
                    Some(&resolved.route),
                )
                .await?;
                return Ok(());
            };
            headers.insert(AUTHORIZATION, value);
        }
        for (name, value) in &resolved.route.headers {
            if value.trim().is_empty() {
                continue;
            }
            if is_hop_by_hop_header(name) {
                write_error_response(
                    &mut stream,
                    502,
                    "route_configuration_error",
                    format!(
                        "线路「{}」包含不允许覆盖的请求头 {name}",
                        resolved.route.route_name
                    ),
                    Some(&resolved.route),
                )
                .await?;
                return Ok(());
            }
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                write_error_response(
                    &mut stream,
                    502,
                    "route_configuration_error",
                    format!("线路「{}」包含非法请求头名称", resolved.route.route_name),
                    Some(&resolved.route),
                )
                .await?;
                return Ok(());
            };
            let Ok(value) = HeaderValue::from_str(value) else {
                write_error_response(
                    &mut stream,
                    502,
                    "route_configuration_error",
                    format!("线路「{}」包含非法请求头值", resolved.route.route_name),
                    Some(&resolved.route),
                )
                .await?;
                return Ok(());
            };
            headers.insert(name, value);
        }
        let response = match self
            .client
            .post(upstream_url)
            .headers(headers)
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let timeout = error.is_timeout();
                let sanitized_error = error.without_url().to_string();
                crate::error_log::record_failure(
                    "local_router_upstream_failed",
                    "proxy_local_router_request",
                    sanitized_error,
                    serde_json::json!({
                        "routeId": resolved.route.provider_id.as_str(),
                        "routeName": resolved.route.route_name.as_str(),
                        "model": resolved.upstream_model.as_str(),
                        "timeout": timeout,
                    }),
                );
                write_error_response(
                    &mut stream,
                    if timeout { 504 } else { 502 },
                    if timeout {
                        "upstream_timeout"
                    } else {
                        "upstream_unreachable"
                    },
                    if timeout {
                        format!("线路「{}」请求超时", resolved.route.route_name)
                    } else {
                        format!("无法连接线路「{}」", resolved.route.route_name)
                    },
                    Some(&resolved.route),
                )
                .await?;
                return Ok(());
            }
        };
        write_proxy_response(&mut stream, response).await
    }
}

fn should_forward_incoming_header(name: &str) -> bool {
    !name.eq_ignore_ascii_case("authorization") && !is_hop_by_hop_header(name)
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "connection"
            | "proxy-connection"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "accept-encoding"
    )
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (*left ^ *right)
        })
        == 0
}

fn responses_endpoint(base_url: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(base_url.trim()).context("线路 API URL 格式无效")?;
    url.set_query(None);
    url.set_fragment(None);
    let base = url.as_str().trim_end_matches('/').to_string();
    if base.to_ascii_lowercase().ends_with("/responses") {
        Ok(base)
    } else {
        Ok(format!("{base}/responses"))
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = stream
            .read(&mut chunk)
            .await
            .context("读取 Codey 本地路由请求失败")?;
        if read == 0 {
            anyhow::bail!("请求在 HTTP 头读取完成前断开");
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_HEADER_BYTES {
            anyhow::bail!("HTTP 请求头超过 Codey 本地路由安全上限");
        }
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
    };
    let header_text = std::str::from_utf8(&buffer[..header_end]).context("HTTP 头不是 UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("HTTP 请求缺少方法"))?
        .to_string();
    let raw_path = request_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("HTTP 请求缺少路径"))?;
    let path = raw_path.split('?').next().unwrap_or(raw_path).to_string();
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    let mut saw_content_length = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value.parse::<usize>().context("HTTP Content-Length 无效")?;
            if saw_content_length && parsed != content_length {
                anyhow::bail!("HTTP 请求包含冲突的 Content-Length");
            }
            saw_content_length = true;
            content_length = parsed;
        }
        if name.eq_ignore_ascii_case("transfer-encoding") && !value.eq_ignore_ascii_case("identity")
        {
            anyhow::bail!("Codey 本地路由不接受分块请求体");
        }
        headers.push((name, value));
    }
    if content_length > MAX_REQUEST_BYTES {
        anyhow::bail!("请求体超过 Codey 本地路由安全上限");
    }
    let body_start = header_end + 4;
    let mut body = buffer.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let remaining = content_length - body.len();
        let mut chunk = vec![0_u8; remaining.min(8192)];
        let read = stream
            .read(&mut chunk)
            .await
            .context("读取 Codey 本地路由请求体失败")?;
        if read == 0 {
            anyhow::bail!("请求体读取完成前连接断开");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

async fn write_error_response(
    stream: &mut TcpStream,
    status: u16,
    code: &str,
    message: impl Into<String>,
    route: Option<&RouteTarget>,
) -> Result<()> {
    let mut codey = serde_json::Map::new();
    if let Some(route) = route {
        codey.insert("routeId".into(), Value::String(route.provider_id.clone()));
        codey.insert("routeName".into(), Value::String(route.route_name.clone()));
    }
    let mut error = serde_json::Map::from_iter([
        ("message".into(), Value::String(message.into())),
        ("type".into(), Value::String("codey_route_error".into())),
        ("code".into(), Value::String(code.to_string())),
    ]);
    if !codey.is_empty() {
        error.insert("codey".into(), Value::Object(codey));
    }
    write_json_response(stream, status, &json!({ "error": error })).await
}

async fn write_json_response(stream: &mut TcpStream, status: u16, value: &Value) -> Result<()> {
    let mut body = serde_json::to_vec(value).context("序列化 Codey 本地路由响应失败")?;
    body.push(b'\n');
    let reason = reason_phrase(status);
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(&body).await?;
    Ok(())
}

async fn write_proxy_response(
    stream: &mut TcpStream,
    mut response: reqwest::Response,
) -> Result<()> {
    let status = response.status().as_u16();
    let reason = response.status().canonical_reason().unwrap_or("OK");
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json");
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    while let Some(chunk) = response.chunk().await.context("读取上游响应失败")? {
        if chunk.is_empty() {
            continue;
        }
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(&chunk).await?;
        stream.write_all(b"\r\n").await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    Ok(())
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        408 => "Request Timeout",
        404 => "Not Found",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "OK",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderProfile;

    fn router_config(base_url: String) -> (CodeyConfig, String, String) {
        let mut route = ProviderProfile::new("Relay");
        route.id = "route-a".into();
        route.base_url = base_url;
        route.api_key = "sk-upstream".into();
        route.normalize();
        let provider_id = route.provider_id().to_string();
        let model = "provider-model".to_string();
        let mut config = CodeyConfig {
            active_profile_id: route.id.clone(),
            profiles: vec![route],
            ..CodeyConfig::default()
        }
        .normalize();
        config
            .selected_models_by_provider
            .insert(provider_id.clone(), vec![model.clone()]);
        (config, provider_id, model)
    }

    #[test]
    fn router_snapshot_maps_route_aliases_to_upstream_models() {
        let (config, provider_id, model) = router_config("https://relay.example/v1".to_string());

        let snapshot = RouterSnapshot::from_config(&config);
        let resolved = snapshot
            .target_for_model(&model_alias(&provider_id, &model))
            .unwrap();

        assert_eq!(resolved.route.base_url, "https://relay.example/v1");
        assert_eq!(resolved.upstream_model, model);
        assert!(snapshot.target_for_model("provider-model").is_err());
    }

    #[test]
    fn router_snapshot_keeps_all_third_party_routes_active_at_once() {
        let (mut config, provider_a, model) =
            router_config("https://relay-a.example/v1".to_string());
        let mut route_b = config.profiles[0].clone();
        route_b.id = "route-b".into();
        route_b.name = "Relay B".into();
        route_b.base_url = "https://relay-b.example/v1".into();
        route_b.api_key = "sk-route-b".into();
        route_b.normalize();
        let provider_b = route_b.provider_id().to_string();
        config.profiles.push(route_b);
        config
            .selected_models_by_provider
            .insert(provider_b.clone(), vec![model.clone()]);

        let snapshot = RouterSnapshot::from_config(&config);
        let resolved_a = snapshot
            .target_for_model(&model_alias(&provider_a, &model))
            .unwrap();
        let resolved_b = snapshot
            .target_for_model(&model_alias(&provider_b, &model))
            .unwrap();

        assert_eq!(resolved_a.route.base_url, "https://relay-a.example/v1");
        assert_eq!(resolved_b.route.base_url, "https://relay-b.example/v1");
        assert_eq!(snapshot.model_aliases().len(), 2);
    }

    #[test]
    fn router_does_not_invent_models_for_an_unconfigured_api_route() {
        let (mut config, provider_id, _) = router_config("https://relay.example/v1".to_string());
        config.selected_models_by_provider.remove(&provider_id);

        let snapshot = RouterSnapshot::from_config(&config);

        assert!(snapshot.model_aliases().is_empty());
        assert!(
            snapshot
                .target_for_model(&model_alias(&provider_id, "gpt-5.6-sol"))
                .is_err()
        );
    }

    #[test]
    fn official_looking_ids_declared_on_an_api_route_remain_route_scoped() {
        let (mut config, provider_id, _) = router_config("https://relay.example/v1".to_string());
        config.selected_models_by_provider.remove(&provider_id);
        config
            .declared_official_models_by_provider
            .insert(provider_id.clone(), vec!["gpt-5.6-sol".into()]);

        let snapshot = RouterSnapshot::from_config(&config);
        let resolved = snapshot
            .target_for_model(&model_alias(&provider_id, "gpt-5.6-sol"))
            .unwrap();

        assert_eq!(resolved.route.provider_id, provider_id);
        assert_eq!(resolved.upstream_model, "gpt-5.6-sol");
    }

    #[test]
    fn responses_endpoint_reuses_explicit_responses_url() {
        assert_eq!(
            responses_endpoint("https://relay.example/v1/responses").unwrap(),
            "https://relay.example/v1/responses"
        );
        assert_eq!(
            responses_endpoint("https://relay.example/v1").unwrap(),
            "https://relay.example/v1/responses"
        );
    }

    #[test]
    fn model_aliases_do_not_collapse_distinct_provider_ids() {
        assert_ne!(
            model_alias("team/relay", "shared-model"),
            model_alias("team_relay", "shared-model")
        );
        assert_eq!(
            model_alias("team/relay", "shared-model"),
            "team%2Frelay/shared-model"
        );
    }

    #[tokio::test]
    async fn router_rewrites_alias_and_keeps_upstream_credentials_private() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let request = read_http_request(&mut stream).await.unwrap();
            let authorization = request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.clone());
            let body = serde_json::from_slice::<Value>(&request.body).unwrap();
            write_json_response(
                &mut stream,
                200,
                &json!({"object":"response","model":body["model"]}),
            )
            .await
            .unwrap();
            (request.path, authorization, body)
        });
        let (config, provider_id, model) = router_config(format!("http://{upstream_address}/v1"));
        let router = LocalRouter::start(&config).await.unwrap();
        let endpoint = router.endpoint();
        let alias = model_alias(&provider_id, &model);

        let response = reqwest::Client::new()
            .post(format!("{}/responses", endpoint.base_url))
            .bearer_auth(&endpoint.token)
            .json(&json!({"model":alias,"input":"hello","stream":true}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap()["model"], model);
        let (path, authorization, body) = upstream_task.await.unwrap();
        assert_eq!(path, "/v1/responses");
        assert_eq!(authorization.as_deref(), Some("Bearer sk-upstream"));
        assert_eq!(body["model"], "provider-model");
        assert!(!body.to_string().contains(&endpoint.token));
        router.stop().await.unwrap();
    }

    #[tokio::test]
    async fn router_rejects_unknown_raw_models_instead_of_guessing_a_route() {
        let (config, _, _) = router_config("http://127.0.0.1:9/v1".to_string());
        let router = LocalRouter::start(&config).await.unwrap();
        let endpoint = router.endpoint();

        let response = reqwest::Client::new()
            .post(format!("{}/responses", endpoint.base_url))
            .bearer_auth(&endpoint.token)
            .json(&json!({"model":"provider-model","input":"hello"}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["error"]["code"], "model_not_enabled");
        router.stop().await.unwrap();
    }

    #[tokio::test]
    async fn router_rejects_requests_without_the_launch_token() {
        let (config, provider_id, model) = router_config("http://127.0.0.1:9/v1".to_string());
        let router = LocalRouter::start(&config).await.unwrap();
        let endpoint = router.endpoint();

        let response = reqwest::Client::new()
            .post(format!("{}/responses", endpoint.base_url))
            .json(&json!({"model":model_alias(&provider_id, &model),"input":"hello"}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_router_token");
        router.stop().await.unwrap();
    }
}
