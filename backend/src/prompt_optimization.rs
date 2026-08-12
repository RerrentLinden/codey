use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use codey_runtime_core::{
    protocol_proxy::{chat_completion_to_response_with_request, responses_to_chat_completions},
    settings::RelayProtocol,
};
use reqwest::{
    Client, RequestBuilder,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
};
use serde_json::{Value, json};

use crate::config::PromptOptimizationConfig;

/// Built-in optimizer instruction used when the user leaves the custom
/// instruction empty. The model must return only the rewritten prompt so the
/// result can replace the composer content directly.
pub const DEFAULT_OPTIMIZER_INSTRUCTION: &str = "你是提示词优化专家。用户会提供一段提示词，请在不改变其意图的前提下，把它重写为更清晰、更具体、可执行的高质量提示词。只输出优化后的提示词本身，不要添加任何解释、前言、后记或代码围栏。";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_INPUT_CHARS: usize = 32 * 1024;
const MAX_OUTPUT_CHARS: usize = 8192;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TOKENS: u32 = 2048;
const MAX_MODELS: usize = 2000;
const MAX_MODEL_ID_CHARS: usize = 512;

#[derive(Debug)]
enum ModelListBodyError {
    InvalidJson(serde_json::Error),
    UnsupportedFormat,
    Empty,
}

impl fmt::Display for ModelListBodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "模型列表不是有效 JSON：{error}"),
            Self::UnsupportedFormat => formatter.write_str("模型列表格式不受支持"),
            Self::Empty => formatter.write_str("模型列表为空"),
        }
    }
}

#[derive(Debug)]
struct OptimizedResponseError {
    message: String,
    retryable_with_v1: bool,
}

impl OptimizedResponseError {
    fn fatal(message: String) -> Self {
        Self {
            message,
            retryable_with_v1: false,
        }
    }

    fn retryable(message: String) -> Self {
        Self {
            message,
            retryable_with_v1: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedPromptOptimizationConfig {
    pub base_url: String,
    pub api_key: String,
    pub request_headers: BTreeMap<String, String>,
    pub protocol: RelayProtocol,
    pub model: String,
    pub instruction: String,
}

impl ResolvedPromptOptimizationConfig {
    pub fn from_custom(config: &PromptOptimizationConfig) -> Self {
        Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            request_headers: BTreeMap::new(),
            protocol: config.protocol,
            model: config.model.clone(),
            instruction: config.instruction.clone(),
        }
    }
}

/// Builds the dedicated HTTP client for optimizer requests. The shared
/// `AppState` client caps connects at 5s, which is too tight for provider
/// relays behind a system proxy (CONNECT + TLS handshake routinely exceed
/// that). The per-request `.timeout()` still bounds the whole call.
pub fn optimizer_http_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("创建优化 HTTP 客户端失败：{error}"))
}

/// Builds the Chat Completions endpoint from the configured base URL. The
/// user may fill in either the bare base (`https://api.example.com/v1`) or
/// the complete endpoint (`https://api.example.com/v1/chat/completions`);
/// both forms are accepted without double-appending the suffix.
#[cfg(test)]
pub fn chat_completions_endpoint(config: &PromptOptimizationConfig) -> Result<String, String> {
    request_endpoint(&config.base_url, RelayProtocol::ChatCompletions)
}

fn request_endpoint(base_url: &str, protocol: RelayProtocol) -> Result<String, String> {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("请先配置 OpenAI 兼容 API 地址".to_string());
    }
    let url =
        reqwest::Url::parse(base_url).map_err(|_| "API 地址不是有效的 HTTP(S) 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("API 地址必须是有效的 HTTP(S) 地址".to_string());
    }
    let suffix = match protocol {
        RelayProtocol::ChatCompletions => "/chat/completions",
        RelayProtocol::Responses => "/responses",
    };
    if base_url.ends_with(suffix) {
        return Ok(base_url.to_string());
    }
    let base_url = base_url
        .strip_suffix("/chat/completions")
        .or_else(|| base_url.strip_suffix("/responses"))
        .unwrap_or(base_url);
    Ok(format!("{base_url}{suffix}"))
}

/// Whether a 404 on the built endpoint should trigger the `/v1` retry. The
/// retry only applies when the user supplied a bare base URL that does not
/// already carry the `/v1` prefix or the complete endpoint.
fn should_retry_with_v1(base_url: &str) -> bool {
    !base_url.ends_with("/v1")
        && !base_url.ends_with("/chat/completions")
        && !base_url.ends_with("/responses")
}

fn v1_request_endpoint(base_url: &str, protocol: RelayProtocol) -> String {
    let suffix = match protocol {
        RelayProtocol::ChatCompletions => "chat/completions",
        RelayProtocol::Responses => "responses",
    };
    format!("{}/v1/{suffix}", base_url.trim().trim_end_matches('/'))
}

/// Builds the first model-list endpoint from the same compatibility rules used
/// by current-provider model synchronization.
#[cfg(test)]
fn models_endpoint(config: &PromptOptimizationConfig) -> Result<String, String> {
    models_endpoints_from_base(&config.base_url).map(|endpoints| endpoints[0].clone())
}

fn models_endpoints_from_base(base_url: &str) -> Result<Vec<String>, String> {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("请先配置 OpenAI 兼容 API 地址".to_string());
    }
    let mut url =
        reqwest::Url::parse(base_url).map_err(|_| "API 地址不是有效的 HTTP(S) 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("API 地址必须是有效的 HTTP(S) 地址".to_string());
    }
    url.set_query(None);
    url.set_fragment(None);
    let mut path = url.path().trim_end_matches('/').to_string();
    let mut base = url.as_str().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/responses"] {
        if path.to_ascii_lowercase().ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            base.truncate(base.len() - suffix.len());
            break;
        }
    }
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    if last_segment.eq_ignore_ascii_case("models") {
        Ok(vec![base])
    } else if has_version_suffix(last_segment) {
        Ok(vec![format!("{base}/models")])
    } else {
        Ok(vec![format!("{base}/v1/models"), format!("{base}/models")])
    }
}

fn has_version_suffix(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .or_else(|| segment.strip_prefix('V'))
        .is_some_and(|version| version.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
}

/// Fetches the model IDs advertised by the configured OpenAI-compatible
/// service (`GET /models`), with the same `/v1` retry and error sanitization
/// as the completion requests. The result keeps upstream order, is deduped
/// and bounded so a misbehaving service cannot balloon the console.
pub async fn fetch_models(
    client: &Client,
    config: &PromptOptimizationConfig,
) -> Result<Vec<String>, String> {
    if config.api_key.trim().is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    fetch_models_resolved(
        client,
        &ResolvedPromptOptimizationConfig::from_custom(config),
    )
    .await
}

pub async fn fetch_models_resolved(
    client: &Client,
    config: &ResolvedPromptOptimizationConfig,
) -> Result<Vec<String>, String> {
    let endpoints = models_endpoints_from_base(&config.base_url)?;
    let api_key = config.api_key.trim();
    for (index, endpoint) in endpoints.iter().enumerate() {
        let response = authenticated_request(client.get(endpoint), config)
            .header(ACCEPT, "application/json")
            .timeout(MODELS_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                sanitize_resolved_error(
                    &format!("获取模型列表失败：{}", format_error_chain(&error)),
                    config,
                )
            })?;
        let status = response.status();
        let has_fallback = index + 1 < endpoints.len();
        if has_fallback
            && (matches!(
                status,
                reqwest::StatusCode::NOT_FOUND
                    | reqwest::StatusCode::METHOD_NOT_ALLOWED
                    | reqwest::StatusCode::REQUEST_TIMEOUT
                    | reqwest::StatusCode::TOO_MANY_REQUESTS
            ) || status.is_server_error())
        {
            continue;
        }
        if !status.is_success() {
            let status = status.as_u16();
            let detail =
                sanitize_resolved_error(&response_body_preview(response, api_key).await?, config);
            let detail: String = detail.chars().take(200).collect();
            return Err(format!(
                "获取模型列表失败（HTTP {status}，{endpoint}）：{detail}"
            ));
        }
        let body = read_bounded_body(response, MAX_RESPONSE_BYTES, "模型列表响应", api_key).await?;
        match parse_model_ids(&body) {
            Ok(models) => return Ok(models),
            Err(_) if has_fallback => continue,
            Err(error) => {
                let preview =
                    sanitize_resolved_error(String::from_utf8_lossy(&body).trim(), config);
                let preview: String = preview.chars().take(200).collect();
                let preview = if preview.is_empty() {
                    "空响应".to_string()
                } else {
                    preview
                };
                return Err(format!(
                    "解析模型列表失败（{endpoint}）：{error}。响应摘要：{preview}"
                ));
            }
        }
    }
    Err("服务端没有返回可用模型".to_string())
}

#[cfg(test)]
fn extract_model_ids(value: &Value) -> Vec<String> {
    extract_model_ids_with_recognition(value).1
}

fn parse_model_ids(body: &[u8]) -> Result<Vec<String>, ModelListBodyError> {
    let value = serde_json::from_slice::<Value>(body).map_err(ModelListBodyError::InvalidJson)?;
    let (recognized, models) = extract_model_ids_with_recognition(&value);
    if !recognized {
        return Err(ModelListBodyError::UnsupportedFormat);
    }
    if models.is_empty() {
        return Err(ModelListBodyError::Empty);
    }
    Ok(models)
}

fn extract_model_ids_with_recognition(value: &Value) -> (bool, Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    let recognized = match value {
        Value::Array(items) => {
            collect_model_ids(items, &mut seen, &mut models);
            true
        }
        Value::Object(object) => {
            let mut recognized = false;
            for key in ["data", "models", "items"] {
                if let Some(items) = object.get(key).and_then(Value::as_array) {
                    recognized = true;
                    collect_model_ids(items, &mut seen, &mut models);
                }
            }
            if !recognized && let Some(id) = model_id_value(value) {
                push_model_id(id, &mut seen, &mut models);
                recognized = true;
            }
            recognized
        }
        _ => false,
    };
    (recognized, models)
}

fn collect_model_ids(
    items: &[Value],
    seen: &mut std::collections::HashSet<String>,
    models: &mut Vec<String>,
) {
    for item in items {
        if let Some(id) = model_id_value(item) {
            push_model_id(id, seen, models);
        }
        if models.len() >= MAX_MODELS {
            break;
        }
    }
}

fn model_id_value(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| {
        let object = value.as_object()?;
        ["id", "name", "slug", "model"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
    })
}

fn push_model_id(id: &str, seen: &mut std::collections::HashSet<String>, models: &mut Vec<String>) {
    let id = id.trim();
    if id.is_empty()
        || id.chars().count() > MAX_MODEL_ID_CHARS
        || models.len() >= MAX_MODELS
        || !seen.insert(id.to_string())
    {
        return;
    }
    models.push(id.to_string());
}

#[cfg(test)]
pub fn optimizer_payload(config: &PromptOptimizationConfig, text: &str) -> Value {
    let request = responses_payload(&config.model, &config.instruction, text);
    upstream_request_payload(&request, config.protocol).expect("optimizer payload should convert")
}

fn effective_instruction(instruction: &str) -> &str {
    let instruction = instruction.trim();
    if instruction.is_empty() {
        DEFAULT_OPTIMIZER_INSTRUCTION
    } else {
        instruction
    }
}

fn responses_payload(model: &str, instruction: &str, text: &str) -> Value {
    json!({
        "model": model.trim(),
        "instructions": effective_instruction(instruction),
        "input": text,
        "max_output_tokens": MAX_TOKENS,
        "stream": false,
    })
}

fn upstream_request_payload(request: &Value, protocol: RelayProtocol) -> Result<Value, String> {
    match protocol {
        RelayProtocol::Responses => Ok(request.clone()),
        RelayProtocol::ChatCompletions => responses_to_chat_completions(request.clone())
            .map_err(|error| format!("转换 Responses 请求到 Chat Completions 失败：{error:#}")),
    }
}

/// Optimizes a user prompt through the resolved Chat Completions or Responses
/// API and returns the rewritten prompt. All returned error messages are
/// sanitized so provider credentials never reach the renderer or logs.
#[cfg(test)]
pub async fn optimize_prompt(
    client: &Client,
    config: &PromptOptimizationConfig,
    text: &str,
) -> Result<String, String> {
    if config.api_key.trim().is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    optimize_prompt_resolved(
        client,
        &ResolvedPromptOptimizationConfig::from_custom(config),
        text,
    )
    .await
}

pub async fn optimize_prompt_resolved(
    client: &Client,
    config: &ResolvedPromptOptimizationConfig,
    text: &str,
) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("请输入要优化的提示词".to_string());
    }
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err(format!("提示词过长，最多支持 {MAX_INPUT_CHARS} 个字符"));
    }
    let endpoint = request_endpoint(&config.base_url, config.protocol)?;
    if config.model.trim().is_empty() {
        return Err("请先配置优化模型".to_string());
    }

    let original_request = responses_payload(&config.model, &config.instruction, text);
    let payload = upstream_request_payload(&original_request, config.protocol)?;
    let response = post_optimization_request(client, &endpoint, config, &payload).await?;
    let status = response.status().as_u16();

    // 404 且用户填的是缺少 /v1 前缀的纯 base 地址时自动补 /v1 重试，
    // 与线路测试一致；已带 /v1 或已填完整端点时不再追加。
    let base_url = config.base_url.trim().trim_end_matches('/');
    if status == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = v1_request_endpoint(base_url, config.protocol);
        let v1_response = post_optimization_request(client, &v1_endpoint, config, &payload).await?;
        return parse_optimized_response(v1_response, &v1_endpoint, config, &original_request)
            .await
            .map_err(|error| error.message);
    }

    match parse_optimized_response(response, &endpoint, config, &original_request).await {
        Ok(optimized) => Ok(optimized),
        Err(error) if error.retryable_with_v1 && should_retry_with_v1(base_url) => {
            let v1_endpoint = v1_request_endpoint(base_url, config.protocol);
            let v1_response =
                post_optimization_request(client, &v1_endpoint, config, &payload).await?;
            parse_optimized_response(v1_response, &v1_endpoint, config, &original_request)
                .await
                .map_err(|error| error.message)
        }
        Err(error) => Err(error.message),
    }
}

/// Sends a minimal request in the resolved protocol to verify connectivity and
/// credentials. Returns the HTTP status, endpoint and a sanitized response
/// preview so the console can show the outcome without a full optimization.
#[cfg(test)]
pub async fn test_configuration(
    client: &Client,
    config: &PromptOptimizationConfig,
) -> Result<Value, String> {
    if config.api_key.trim().is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    test_configuration_resolved(
        client,
        &ResolvedPromptOptimizationConfig::from_custom(config),
    )
    .await
}

pub async fn test_configuration_resolved(
    client: &Client,
    config: &ResolvedPromptOptimizationConfig,
) -> Result<Value, String> {
    let endpoint = request_endpoint(&config.base_url, config.protocol)?;
    let model = config.model.trim();
    if model.is_empty() {
        return Err("请先配置优化模型".to_string());
    }
    let request = json!({
        "model": model,
        "input": "hi",
        "max_output_tokens": 16,
        "stream": false,
    });
    let payload = upstream_request_payload(&request, config.protocol)?;
    let response = post_optimization_request(client, &endpoint, config, &payload).await?;
    let status = response.status().as_u16();

    let base_url = config.base_url.trim().trim_end_matches('/');
    if status == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = v1_request_endpoint(base_url, config.protocol);
        let v1_response = post_optimization_request(client, &v1_endpoint, config, &payload).await?;
        return configuration_test_result(v1_response, &v1_endpoint, config).await;
    }

    configuration_test_result(response, &endpoint, config).await
}

async fn configuration_test_result(
    response: reqwest::Response,
    endpoint: &str,
    config: &ResolvedPromptOptimizationConfig,
) -> Result<Value, String> {
    let status = response.status().as_u16();
    let preview = response_body_preview(response, config.api_key.trim()).await?;
    let preview = sanitize_resolved_error(&preview, config)
        .chars()
        .take(280)
        .collect::<String>();
    if status >= 400 {
        return Err(format!(
            "优化 API 连通性测试失败（HTTP {status}，{endpoint}）：{preview}"
        ));
    }
    Ok(json!({
        "httpStatus": status,
        "endpoint": endpoint,
        "responsePreview": preview,
    }))
}

fn authenticated_request(
    mut request: RequestBuilder,
    config: &ResolvedPromptOptimizationConfig,
) -> RequestBuilder {
    let has_custom_authorization = config.request_headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case(AUTHORIZATION.as_str()) && !value.trim().is_empty()
    });
    if !config.api_key.trim().is_empty() && !has_custom_authorization {
        request = request.bearer_auth(config.api_key.trim());
    }
    for (name, value) in &config.request_headers {
        if name.eq_ignore_ascii_case(AUTHORIZATION.as_str()) && value.trim().is_empty() {
            continue;
        }
        request = request.header(name, value);
    }
    request
}

async fn post_optimization_request(
    client: &Client,
    endpoint: &str,
    config: &ResolvedPromptOptimizationConfig,
    payload: &Value,
) -> Result<reqwest::Response, String> {
    authenticated_request(client.post(endpoint), config)
        .header(CONTENT_TYPE, "application/json")
        .json(payload)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            sanitize_resolved_error(
                &format!("请求优化 API 失败：{}", format_error_chain(&error)),
                config,
            )
        })
}

/// Expands the reqwest error chain (`error sending request …；client error
/// (Connect)；operation timed out`) so transport-level failures show the
/// actual cause instead of only the outer wrapper.
fn format_error_chain(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push('；');
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

async fn parse_optimized_response(
    response: reqwest::Response,
    endpoint: &str,
    config: &ResolvedPromptOptimizationConfig,
    original_request: &Value,
) -> Result<String, OptimizedResponseError> {
    let status = response.status().as_u16();
    if status >= 400 {
        let detail = sanitize_resolved_error(
            &response_body_preview(response, config.api_key.trim())
                .await
                .map_err(OptimizedResponseError::fatal)?,
            config,
        );
        let detail: String = detail.chars().take(200).collect();
        return Err(OptimizedResponseError::fatal(format!(
            "优化 API 请求失败（HTTP {status}，{endpoint}）：{detail}"
        )));
    }
    let body = read_bounded_body(
        response,
        MAX_RESPONSE_BYTES,
        "优化 API 响应",
        config.api_key.trim(),
    )
    .await
    .map_err(OptimizedResponseError::fatal)?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| {
        let preview = sanitize_resolved_error(String::from_utf8_lossy(&body).trim(), config);
        let preview: String = preview.chars().take(200).collect();
        let preview = if preview.is_empty() {
            "空响应".to_string()
        } else {
            preview
        };
        OptimizedResponseError::retryable(format!(
            "优化 API 返回的不是有效 JSON（{endpoint}）。响应摘要：{preview}"
        ))
    })?;
    let response = match config.protocol {
        RelayProtocol::Responses => value,
        RelayProtocol::ChatCompletions => {
            chat_completion_to_response_with_request(value, original_request).map_err(|error| {
                OptimizedResponseError::retryable(format!(
                    "转换 Chat Completions 响应到 Responses 失败（{endpoint}）：{error:#}"
                ))
            })?
        }
    };
    let optimized = extract_responses_optimized_text(&response).ok_or_else(|| {
        OptimizedResponseError::retryable("优化 API 响应中缺少优化结果".to_string())
    })?;
    let optimized = optimized.trim();
    if optimized.is_empty() {
        return Err(OptimizedResponseError::fatal(
            "优化 API 返回了空的优化结果".to_string(),
        ));
    }
    Ok(optimized.chars().take(MAX_OUTPUT_CHARS).collect())
}

/// Extracts `choices[0].message.content`, accepting both the plain string
/// form and the newer part-array form (`{type:"text",text:...}`).
#[cfg(test)]
fn extract_optimized_text(response: &Value) -> Option<String> {
    let content = response.pointer("/choices/0/message/content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if let Some(segment) = part.get("text").and_then(Value::as_str) {
                    text.push_str(segment);
                }
            }
            Some(text)
        }
        _ => None,
    }
}

fn extract_responses_optimized_text(response: &Value) -> Option<String> {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let mut text = String::new();
    for item in response.get("output").and_then(Value::as_array)? {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            if let Some(segment) = part.get("text").and_then(Value::as_str) {
                text.push_str(segment);
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
    label: &str,
    api_key: &str,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("{label}过大，已停止读取"));
    }

    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(max_bytes);
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| sanitize_error(&format!("读取{label}失败：{error}"), api_key))?
    {
        let next_length = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| format!("{label}过大，已停止读取"))?;
        if next_length > max_bytes {
            return Err(format!("{label}过大，已停止读取"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn response_body_preview(
    response: reqwest::Response,
    api_key: &str,
) -> Result<String, String> {
    let body =
        read_bounded_body(response, MAX_ERROR_RESPONSE_BYTES, "API 错误响应", api_key).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn sanitize_error(error: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return error.to_string();
    }
    error.replace(api_key, "***")
}

fn sanitize_resolved_error(error: &str, config: &ResolvedPromptOptimizationConfig) -> String {
    let mut sanitized = sanitize_error(error, config.api_key.trim());
    for value in config.request_headers.values() {
        let value = value.trim();
        if !value.is_empty() {
            sanitized = sanitized.replace(value, "***");
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> PromptOptimizationConfig {
        PromptOptimizationConfig {
            enabled: true,
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test-key".to_string(),
            api_key_configured: true,
            model: "gpt-test".to_string(),
            ..PromptOptimizationConfig::default()
        }
    }

    #[test]
    fn endpoint_building_trims_and_validates() {
        let mut config = configured();
        assert_eq!(
            chat_completions_endpoint(&config).unwrap(),
            "https://api.example.com/v1/chat/completions"
        );
        config.base_url = "https://api.example.com/v1/".to_string();
        assert_eq!(
            chat_completions_endpoint(&config).unwrap(),
            "https://api.example.com/v1/chat/completions"
        );
        config.base_url = "http://127.0.0.1:11434".to_string();
        assert_eq!(
            chat_completions_endpoint(&config).unwrap(),
            "http://127.0.0.1:11434/chat/completions"
        );
        // 直接填写完整端点时不得重复拼接后缀。
        config.base_url = "https://opencode.ai/zen/v1/chat/completions".to_string();
        assert_eq!(
            chat_completions_endpoint(&config).unwrap(),
            "https://opencode.ai/zen/v1/chat/completions"
        );
        config.base_url = "https://opencode.ai/zen/v1/chat/completions/".to_string();
        assert_eq!(
            chat_completions_endpoint(&config).unwrap(),
            "https://opencode.ai/zen/v1/chat/completions"
        );
        config.base_url = "  ".to_string();
        assert!(
            chat_completions_endpoint(&config)
                .unwrap_err()
                .contains("配置")
        );
        config.base_url = "ftp://api.example.com".to_string();
        assert!(
            chat_completions_endpoint(&config)
                .unwrap_err()
                .contains("HTTP")
        );
        config.base_url = "not a url".to_string();
        assert!(
            chat_completions_endpoint(&config)
                .unwrap_err()
                .contains("HTTP")
        );
        assert_eq!(
            request_endpoint(
                "https://api.example.com/v1/chat/completions",
                RelayProtocol::Responses
            )
            .unwrap(),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn v1_retry_applies_only_to_bare_base_urls() {
        assert!(should_retry_with_v1("https://api.example.com/zen"));
        assert!(should_retry_with_v1("https://api.example.com"));
        assert!(!should_retry_with_v1("https://api.example.com/zen/v1"));
        assert!(!should_retry_with_v1("https://api.example.com/v1"));
        assert!(!should_retry_with_v1(
            "https://opencode.ai/zen/v1/chat/completions"
        ));
        assert!(!should_retry_with_v1(
            "https://api.example.com/chat/completions"
        ));
        assert!(!should_retry_with_v1(
            "https://api.example.com/v1/responses"
        ));
    }

    #[test]
    fn models_endpoint_reuses_the_base_url_shapes() {
        let mut config = configured();
        config.base_url = "https://opencode.ai/zen/v1".to_string();
        assert_eq!(
            models_endpoint(&config).unwrap(),
            "https://opencode.ai/zen/v1/models"
        );
        config.base_url = "https://opencode.ai/zen/v1/chat/completions".to_string();
        assert_eq!(
            models_endpoint(&config).unwrap(),
            "https://opencode.ai/zen/v1/models"
        );
        config.base_url = "https://api.example.com".to_string();
        assert_eq!(
            models_endpoint(&config).unwrap(),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            models_endpoints_from_base("https://api.example.com").unwrap(),
            [
                "https://api.example.com/v1/models",
                "https://api.example.com/models",
            ]
        );
        config.base_url = "https://api.example.com/v1/responses".to_string();
        assert_eq!(
            models_endpoint(&config).unwrap(),
            "https://api.example.com/v1/models"
        );
        config.base_url = "  ".to_string();
        assert!(models_endpoint(&config).unwrap_err().contains("配置"));
    }

    #[test]
    fn extracts_bounded_deduped_model_ids() {
        let response = json!({
            "data": [
                {"id": " model-a "},
                {"id": "model-b"},
                {"id": "model-a"},
                {"id": ""},
                {"id": "x".repeat(600)},
                {"id": "model-c"},
            ]
        });
        let models = extract_model_ids(&response);
        assert_eq!(models, ["model-a", "model-b", "model-c"]);

        assert_eq!(
            extract_model_ids(&json!({"object": "list"})),
            Vec::<String>::new()
        );
        assert_eq!(
            extract_model_ids(&json!({"data": "nope"})),
            Vec::<String>::new()
        );
        assert_eq!(
            extract_model_ids(&json!({"models": ["model-d", {"name": "model-e"}]})),
            ["model-d", "model-e"]
        );
        assert_eq!(
            extract_model_ids(&json!({"items": [{"model": "model-f"}]})),
            ["model-f"]
        );
    }

    #[tokio::test]
    async fn fetch_models_parses_the_upstream_list_via_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 8192];
            let _ = socket.read(&mut request).await.unwrap();
            let body = r#"{"object":"list","data":[{"id":"model-a"},{"id":"model-b"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let mut config = configured();
        config.base_url = format!("http://{address}");
        let client = Client::new();
        let models = fetch_models(&client, &config).await.unwrap();
        assert_eq!(models, ["model-a", "model-b"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_models_retries_alternate_endpoint_after_invalid_json() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected_path, body) in [
                ("/v1/models", "<html>not a model list</html>"),
                ("/models", r#"{"models":["fallback-model"]}"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 8192];
                let bytes_read = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..bytes_read]);
                assert!(
                    request.starts_with(&format!("GET {expected_path} ")),
                    "{request}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let mut config = configured();
        config.base_url = format!("http://{address}");
        let models = fetch_models(&Client::new(), &config).await.unwrap();
        assert_eq!(models, ["fallback-model"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn configuration_test_rejects_http_errors_and_redacts_the_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 8192];
            let _ = socket.read(&mut request).await.unwrap();
            let body = r#"{"error":{"message":"invalid api key sk-test-key"}}"#;
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let mut config = configured();
        config.base_url = format!("http://{address}/v1");
        let error = test_configuration(&Client::new(), &config)
            .await
            .unwrap_err();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("***"), "{error}");
        assert!(!error.contains("sk-test-key"), "{error}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn configuration_test_reports_the_v1_retry_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (status, body) in [
                ("404 Not Found", r#"{"error":{"message":"missing route"}}"#),
                (
                    "401 Unauthorized",
                    r#"{"error":{"message":"retry credentials rejected"}}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 8192];
                let _ = socket.read(&mut request).await.unwrap();
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let mut config = configured();
        config.base_url = format!("http://{address}");
        let error = test_configuration(&Client::new(), &config)
            .await
            .unwrap_err();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("/v1/chat/completions"), "{error}");
        assert!(error.contains("retry credentials rejected"), "{error}");
        assert!(!error.contains("missing route"), "{error}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn chunked_error_response_is_stopped_at_the_preview_limit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 8192];
            let _ = socket.read(&mut request).await.unwrap();
            let body = "x".repeat(MAX_ERROR_RESPONSE_BYTES + 1);
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:X}\r\n{}\r\n0\r\n\r\n",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let mut config = configured();
        config.base_url = format!("http://{address}/v1");
        let error = test_configuration(&Client::new(), &config)
            .await
            .unwrap_err();
        assert!(error.contains("过大"), "{error}");
        server.await.unwrap();
    }

    #[test]
    fn payload_uses_custom_instruction_or_the_default() {
        let mut config = configured();
        let payload = optimizer_payload(&config, " 你好 ");
        assert_eq!(
            payload["messages"][0]["content"],
            DEFAULT_OPTIMIZER_INSTRUCTION
        );
        assert_eq!(payload["messages"][1]["content"], " 你好 ");
        assert_eq!(payload["model"], "gpt-test");

        config.instruction = " 简短回复 ".to_string();
        let payload = optimizer_payload(&config, "你好");
        assert_eq!(payload["messages"][0]["content"], "简短回复");

        config.protocol = RelayProtocol::Responses;
        let payload = optimizer_payload(&config, "你好");
        assert_eq!(payload["instructions"], "简短回复");
        assert_eq!(payload["input"], "你好");
        assert_eq!(payload["max_output_tokens"], MAX_TOKENS);
        assert_eq!(payload["stream"], false);
        assert!(payload.get("messages").is_none());
    }

    #[test]
    fn extracts_text_content_from_string_and_part_arrays() {
        let string_response = json!({"choices": [{"message": {"content": "优化结果"}}]});
        assert_eq!(
            extract_optimized_text(&string_response).as_deref(),
            Some("优化结果")
        );

        let array_response = json!({"choices": [{"message": {"content": [
            {"type": "text", "text": "优化"},
            {"type": "text", "text": "结果"},
        ]}}]});
        assert_eq!(
            extract_optimized_text(&array_response).as_deref(),
            Some("优化结果")
        );

        assert_eq!(extract_optimized_text(&json!({"choices": []})), None);
        assert_eq!(
            extract_optimized_text(&json!({"error": {"message": "boom"}})),
            None
        );
        assert_eq!(
            extract_optimized_text(&json!({"choices": [{"message": {"content": 42}}]})),
            None
        );
    }

    #[test]
    fn extracts_responses_text_from_convenience_and_output_fields() {
        assert_eq!(
            extract_responses_optimized_text(&json!({"output_text": "直接结果"})).as_deref(),
            Some("直接结果")
        );
        assert_eq!(
            extract_responses_optimized_text(&json!({
                "output": [{
                    "type": "message",
                    "content": [
                        {"type": "output_text", "text": "优化"},
                        {"type": "output_text", "text": "结果"}
                    ]
                }]
            }))
            .as_deref(),
            Some("优化结果")
        );
    }

    #[test]
    fn sanitize_error_hides_the_api_key() {
        assert_eq!(
            sanitize_error("401 unauthorized for sk-test-key", "sk-test-key"),
            "401 unauthorized for ***"
        );
        assert_eq!(sanitize_error("boom", ""), "boom");
    }

    #[test]
    fn resolved_error_hides_provider_header_credentials() {
        let mut request_headers = BTreeMap::new();
        request_headers.insert(
            "Authorization".to_string(),
            "provider-header-secret".to_string(),
        );
        let config = ResolvedPromptOptimizationConfig {
            base_url: "https://provider.example/v1".to_string(),
            api_key: "provider-api-secret".to_string(),
            request_headers,
            protocol: RelayProtocol::Responses,
            model: "gpt-provider".to_string(),
            instruction: String::new(),
        };

        let sanitized = sanitize_resolved_error(
            "provider-api-secret provider-header-secret rejected",
            &config,
        );
        assert_eq!(sanitized, "*** *** rejected");
    }

    #[tokio::test]
    async fn optimize_prompt_validates_before_any_request() {
        let client = Client::new();
        let config = configured();

        assert!(
            optimize_prompt(&client, &config, "   ")
                .await
                .unwrap_err()
                .contains("输入")
        );
        assert!(
            optimize_prompt(&client, &config, &"a".repeat(40_000))
                .await
                .unwrap_err()
                .contains("过长")
        );

        let mut no_key = configured();
        no_key.api_key.clear();
        assert!(
            optimize_prompt(&client, &no_key, "你好")
                .await
                .unwrap_err()
                .contains("API Key")
        );

        let mut no_model = configured();
        no_model.model.clear();
        assert!(
            optimize_prompt(&client, &no_model, "你好")
                .await
                .unwrap_err()
                .contains("模型")
        );

        let mut no_base = configured();
        no_base.base_url.clear();
        assert!(
            optimize_prompt(&client, &no_base, "你好")
                .await
                .unwrap_err()
                .contains("配置")
        );
    }

    #[tokio::test]
    async fn optimize_prompt_replaces_text_via_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let bytes_read = socket.read(&mut chunk).await.unwrap();
                if bytes_read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..bytes_read]);
                let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("POST /chat/completions "), "{request}");
            assert!(request.contains("\"messages\""), "{request}");
            assert!(!request.contains("\"input\":"), "{request}");
            let body = r#"{"choices":[{"message":{"content":"优化后的提示词"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let mut config = configured();
        config.base_url = format!("http://{address}");
        let client = Client::new();
        let result = optimize_prompt(&client, &config, "写个博客").await.unwrap();
        assert_eq!(result, "优化后的提示词");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn optimize_prompt_retries_v1_after_successful_non_json_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected_path, body) in [
                ("/chat/completions", "<html>missing API prefix</html>"),
                (
                    "/v1/chat/completions",
                    r#"{"choices":[{"message":{"content":"回退后的优化结果"}}]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 4096];
                    let bytes_read = socket.read(&mut chunk).await.unwrap();
                    if bytes_read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..bytes_read]);
                    let Some(header_end) =
                        request.windows(4).position(|window| window == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                assert!(
                    request.starts_with(&format!("POST {expected_path} ")),
                    "{request}"
                );
                assert!(request.contains("\"stream\":false"), "{request}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let mut config = configured();
        config.base_url = format!("http://{address}");
        let result = optimize_prompt(&Client::new(), &config, "写个博客")
            .await
            .unwrap();
        assert_eq!(result, "回退后的优化结果");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn resolved_responses_request_uses_provider_headers_and_protocol() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let bytes_read = socket.read(&mut chunk).await.unwrap();
                if bytes_read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..bytes_read]);
                let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            let request_lower = request.to_ascii_lowercase();
            assert!(request.starts_with("POST /v1/responses "));
            assert!(
                request_lower.contains("authorization: custom provider token"),
                "{request}"
            );
            assert!(!request_lower.contains("bearer ignored-key"), "{request}");
            assert!(
                request.contains(r#""instructions":"保持原意""#),
                "{request}"
            );
            assert!(request.contains(r#""input":"写个博客""#), "{request}");

            let body = r#"{"output":[{"content":[{"type":"output_text","text":"优化后的响应"}]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let mut request_headers = BTreeMap::new();
        request_headers.insert(
            "Authorization".to_string(),
            "Custom Provider Token".to_string(),
        );
        let config = ResolvedPromptOptimizationConfig {
            base_url: format!("http://{address}/v1"),
            api_key: "ignored-key".to_string(),
            request_headers,
            protocol: RelayProtocol::Responses,
            model: "gpt-responses".to_string(),
            instruction: "保持原意".to_string(),
        };
        let result = optimize_prompt_resolved(&Client::new(), &config, "写个博客")
            .await
            .unwrap();
        assert_eq!(result, "优化后的响应");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn optimize_prompt_reports_upstream_error_without_the_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 8192];
            let _ = socket.read(&mut request).await.unwrap();
            let body = r#"{"error":{"message":"invalid api key sk-test-key"}}"#;
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let mut config = configured();
        config.base_url = format!("http://{address}");
        let client = Client::new();
        let error = optimize_prompt(&client, &config, "你好").await.unwrap_err();
        assert!(error.contains("401"), "{error}");
        assert!(!error.contains("sk-test-key"), "{error}");
        assert!(error.contains("***"), "{error}");
        server.await.unwrap();
    }
}
