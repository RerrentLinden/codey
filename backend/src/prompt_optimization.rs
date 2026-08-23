use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use reqwest::{
    Client, RequestBuilder,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
};
use serde_json::{Value, json};

use crate::config::PromptOptimizationConfig;
use crate::model_list::{self, ModelEndpointError};

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
    pub model: String,
    pub instruction: String,
}

impl ResolvedPromptOptimizationConfig {
    pub fn from_custom(config: &PromptOptimizationConfig) -> Self {
        Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            request_headers: BTreeMap::new(),
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

fn request_endpoint(base_url: &str) -> Result<String, String> {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("请先配置 OpenAI 兼容 API 地址".to_string());
    }
    let url =
        reqwest::Url::parse(base_url).map_err(|_| "API 地址不是有效的 HTTP(S) 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("API 地址必须是有效的 HTTP(S) 地址".to_string());
    }
    if base_url.ends_with("/chat/completions") {
        return Err(
            "Codey 已移除协议转换能力；请改用提供 Responses API 的第三方网关地址".to_string(),
        );
    }
    if base_url.ends_with("/responses") {
        return Ok(base_url.to_string());
    }
    Ok(format!("{base_url}/responses"))
}

/// Whether a 404 on the built endpoint should trigger the `/v1` retry. The
/// retry only applies when the user supplied a bare base URL that does not
/// already carry the `/v1` prefix or the complete endpoint.
fn should_retry_with_v1(base_url: &str) -> bool {
    !base_url.ends_with("/v1") && !base_url.ends_with("/responses")
}

fn v1_request_endpoint(base_url: &str) -> String {
    format!("{}/v1/responses", base_url.trim().trim_end_matches('/'))
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
    model_list::model_endpoints(base_url, false, true).map_err(|error| match error {
        ModelEndpointError::InvalidUrl => "API 地址不是有效的 HTTP(S) 地址".to_string(),
        ModelEndpointError::UnsupportedSchemeOrHost => {
            "API 地址必须是有效的 HTTP(S) 地址".to_string()
        }
    })
}

/// Fetches the model IDs advertised by the configured OpenAI-compatible
/// service (`GET /models`), with the same `/v1` retry and error sanitization
/// as the completion requests. The result keeps upstream order, is deduped
/// and bounded so a misbehaving service cannot balloon the console.
#[cfg(test)]
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
    let recognized = model_list::visit_model_ids(value, &mut |id| {
        push_model_id(id, &mut seen, &mut models);
        Result::<bool, std::convert::Infallible>::Ok(models.len() < MAX_MODELS)
    })
    .unwrap_or_else(|never| match never {});
    (recognized, models)
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
    responses_payload(&config.model, &config.instruction, text)
}

fn effective_instruction(instruction: &str) -> &str {
    let instruction = instruction.trim();
    if instruction.is_empty() {
        DEFAULT_OPTIMIZER_INSTRUCTION
    } else {
        instruction
    }
}

fn responses_user_input(text: &str) -> Value {
    json!([{
        "type": "message",
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": text,
        }],
    }])
}

fn responses_payload(model: &str, instruction: &str, text: &str) -> Value {
    json!({
        "model": model.trim(),
        "instructions": effective_instruction(instruction),
        "input": responses_user_input(text),
        "max_output_tokens": MAX_TOKENS,
        "stream": false,
    })
}

fn configuration_test_payload(model: &str) -> Value {
    json!({
        "model": model,
        "input": responses_user_input("hi"),
        "max_output_tokens": 16,
        "stream": false,
    })
}

/// Optimizes a user prompt through a Responses API and returns the rewritten
/// prompt. All returned error messages are
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
    let endpoint = request_endpoint(&config.base_url)?;
    if config.model.trim().is_empty() {
        return Err("请先配置优化模型".to_string());
    }

    let payload = responses_payload(&config.model, &config.instruction, text);
    let response = post_optimization_request(client, &endpoint, config, &payload).await?;
    let status = response.status().as_u16();

    // 404 且用户填的是缺少 /v1 前缀的纯 base 地址时自动补 /v1 重试，
    // 与线路测试一致；已带 /v1 或已填完整端点时不再追加。
    let base_url = config.base_url.trim().trim_end_matches('/');
    if status == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = v1_request_endpoint(base_url);
        let v1_response = post_optimization_request(client, &v1_endpoint, config, &payload).await?;
        return parse_optimized_response(v1_response, &v1_endpoint, config)
            .await
            .map_err(|error| error.message);
    }

    match parse_optimized_response(response, &endpoint, config).await {
        Ok(optimized) => Ok(optimized),
        Err(error) if error.retryable_with_v1 && should_retry_with_v1(base_url) => {
            let v1_endpoint = v1_request_endpoint(base_url);
            let v1_response =
                post_optimization_request(client, &v1_endpoint, config, &payload).await?;
            parse_optimized_response(v1_response, &v1_endpoint, config)
                .await
                .map_err(|error| error.message)
        }
        Err(error) => Err(error.message),
    }
}

/// Sends a minimal Responses request to verify connectivity and
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
    let endpoint = request_endpoint(&config.base_url)?;
    let model = config.model.trim();
    if model.is_empty() {
        return Err("请先配置优化模型".to_string());
    }
    let payload = configuration_test_payload(model);
    let response = post_optimization_request(client, &endpoint, config, &payload).await?;
    let status = response.status().as_u16();

    let base_url = config.base_url.trim().trim_end_matches('/');
    if status == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = v1_request_endpoint(base_url);
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
    let optimized = extract_responses_optimized_text(&value).ok_or_else(|| {
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
            request_endpoint(&config.base_url).unwrap(),
            "https://api.example.com/v1/responses"
        );
        config.base_url = "https://api.example.com/v1/".to_string();
        assert_eq!(
            request_endpoint(&config.base_url).unwrap(),
            "https://api.example.com/v1/responses"
        );
        config.base_url = "http://127.0.0.1:11434".to_string();
        assert_eq!(
            request_endpoint(&config.base_url).unwrap(),
            "http://127.0.0.1:11434/responses"
        );
        // 直接填写完整端点时不得重复拼接后缀。
        config.base_url = "https://opencode.ai/zen/v1/responses".to_string();
        assert_eq!(
            request_endpoint(&config.base_url).unwrap(),
            "https://opencode.ai/zen/v1/responses"
        );
        config.base_url = "https://opencode.ai/zen/v1/responses/".to_string();
        assert_eq!(
            request_endpoint(&config.base_url).unwrap(),
            "https://opencode.ai/zen/v1/responses"
        );
        config.base_url = "  ".to_string();
        assert!(
            request_endpoint(&config.base_url)
                .unwrap_err()
                .contains("配置")
        );
        config.base_url = "ftp://api.example.com".to_string();
        assert!(
            request_endpoint(&config.base_url)
                .unwrap_err()
                .contains("HTTP")
        );
        config.base_url = "not a url".to_string();
        assert!(
            request_endpoint(&config.base_url)
                .unwrap_err()
                .contains("HTTP")
        );
        assert!(
            request_endpoint("https://api.example.com/v1/chat/completions")
                .unwrap_err()
                .contains("第三方网关")
        );
    }

    #[test]
    fn v1_retry_applies_only_to_bare_base_urls() {
        assert!(should_retry_with_v1("https://api.example.com/zen"));
        assert!(should_retry_with_v1("https://api.example.com"));
        assert!(!should_retry_with_v1("https://api.example.com/zen/v1"));
        assert!(!should_retry_with_v1("https://api.example.com/v1"));
        assert!(!should_retry_with_v1(
            "https://opencode.ai/zen/v1/responses"
        ));
        assert!(!should_retry_with_v1("https://api.example.com/responses"));
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
        config.base_url = "https://opencode.ai/zen/v1/responses".to_string();
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
        assert_eq!(
            models_endpoints_from_base("https://api.example.com#").unwrap(),
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
            extract_model_ids(&json!({"models": ["Provider-A", "provider-a"]})),
            ["Provider-A", "provider-a"]
        );

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
        assert!(error.contains("/v1/responses"), "{error}");
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
        assert_eq!(payload["instructions"], DEFAULT_OPTIMIZER_INSTRUCTION);
        assert_eq!(payload["input"][0]["content"][0]["text"], " 你好 ");
        assert_eq!(payload["model"], "gpt-test");

        config.instruction = " 简短回复 ".to_string();
        let payload = optimizer_payload(&config, "你好");
        assert_eq!(payload["instructions"], "简短回复");
        assert_eq!(payload["input"][0]["type"], "message");
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["input"][0]["content"][0]["text"], "你好");
        assert_eq!(payload["max_output_tokens"], MAX_TOKENS);
        assert_eq!(payload["stream"], false);
        assert!(payload.get("messages").is_none());
    }

    #[test]
    fn configuration_test_payload_uses_a_responses_input_list() {
        let payload = configuration_test_payload("gpt-test");
        assert!(payload["input"].is_array());
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["input"][0]["content"][0]["text"], "hi");
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
            assert!(request.starts_with("POST /responses "), "{request}");
            assert!(request.contains("\"input\":"), "{request}");
            assert!(!request.contains("\"messages\""), "{request}");
            let body = r#"{"output_text":"优化后的提示词"}"#;
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
                ("/responses", "<html>missing API prefix</html>"),
                ("/v1/responses", r#"{"output_text":"回退后的优化结果"}"#),
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
    async fn resolved_responses_request_uses_provider_headers() {
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
            assert!(
                request_lower
                    .contains("x-codex-installation-id: 49a95816-9ead-4f14-b008-1d0cbaa3c328"),
                "{request}"
            );
            assert!(!request_lower.contains("bearer ignored-key"), "{request}");
            assert!(
                request.contains(r#""instructions":"保持原意""#),
                "{request}"
            );
            let (_, body) = request.split_once("\r\n\r\n").unwrap();
            let body: Value = serde_json::from_str(body).unwrap();
            assert!(body["input"].is_array(), "{body}");
            assert_eq!(body["input"][0]["content"][0]["text"], "写个博客");

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
        request_headers.insert(
            "x-codex-installation-id".to_string(),
            "49a95816-9ead-4f14-b008-1d0cbaa3c328".to_string(),
        );
        let config = ResolvedPromptOptimizationConfig {
            base_url: format!("http://{address}/v1"),
            api_key: "ignored-key".to_string(),
            request_headers,
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
