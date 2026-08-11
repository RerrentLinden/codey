use std::time::Duration;

use reqwest::Client;
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
pub fn chat_completions_endpoint(config: &PromptOptimizationConfig) -> Result<String, String> {
    let base_url = config.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("请先配置 OpenAI 兼容 API 地址".to_string());
    }
    let url =
        reqwest::Url::parse(base_url).map_err(|_| "API 地址不是有效的 HTTP(S) 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("API 地址必须是有效的 HTTP(S) 地址".to_string());
    }
    if base_url.ends_with("/chat/completions") {
        return Ok(base_url.to_string());
    }
    Ok(format!("{base_url}/chat/completions"))
}

/// Whether a 404 on the built endpoint should trigger the `/v1` retry. The
/// retry only applies when the user supplied a bare base URL that does not
/// already carry the `/v1` prefix or the complete endpoint.
fn should_retry_with_v1(base_url: &str) -> bool {
    !base_url.ends_with("/v1") && !base_url.ends_with("/chat/completions")
}

/// Builds the model-list endpoint from the same base URL rules as
/// `chat_completions_endpoint`: a complete `/chat/completions` endpoint is
/// reduced back to the models path, a bare base keeps its shape.
fn models_endpoint(config: &PromptOptimizationConfig) -> Result<String, String> {
    let base_url = config.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("请先配置 OpenAI 兼容 API 地址".to_string());
    }
    let url =
        reqwest::Url::parse(base_url).map_err(|_| "API 地址不是有效的 HTTP(S) 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("API 地址必须是有效的 HTTP(S) 地址".to_string());
    }
    if let Some(prefix) = base_url.strip_suffix("/chat/completions") {
        Ok(format!("{prefix}/models"))
    } else {
        Ok(format!("{base_url}/models"))
    }
}

/// Fetches the model IDs advertised by the configured OpenAI-compatible
/// service (`GET /models`), with the same `/v1` retry and error sanitization
/// as the completion requests. The result keeps upstream order, is deduped
/// and bounded so a misbehaving service cannot balloon the console.
pub async fn fetch_models(
    client: &Client,
    config: &PromptOptimizationConfig,
) -> Result<Vec<String>, String> {
    let endpoint = models_endpoint(config)?;
    let mut effective_endpoint = endpoint.clone();
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    let mut response = client
        .get(&endpoint)
        .bearer_auth(api_key)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(MODELS_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            sanitize_error(
                &format!("获取模型列表失败：{}", format_error_chain(&error)),
                api_key,
            )
        })?;

    let base_url = config.base_url.trim().trim_end_matches('/');
    if response.status().as_u16() == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = if let Some(prefix) = base_url.strip_suffix("/chat/completions") {
            format!("{prefix}/v1/models")
        } else {
            format!("{base_url}/v1/models")
        };
        let v1_response = client
            .get(&v1_endpoint)
            .bearer_auth(api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(MODELS_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                sanitize_error(
                    &format!("获取模型列表失败：{}", format_error_chain(&error)),
                    api_key,
                )
            })?;
        response = v1_response;
        effective_endpoint = v1_endpoint;
    }

    let status = response.status().as_u16();
    if status >= 400 {
        let detail = sanitize_error(&response_body_preview(response, api_key).await?, api_key);
        let detail: String = detail.chars().take(200).collect();
        return Err(format!(
            "获取模型列表失败（HTTP {status}，{effective_endpoint}）：{detail}"
        ));
    }
    let body = read_bounded_body(response, MAX_RESPONSE_BYTES, "模型列表响应", api_key).await?;
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| "模型列表返回的不是有效的 JSON".to_string())?;
    Ok(extract_model_ids(&value))
}

fn extract_model_ids(value: &Value) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    let Some(data) = value.get("data").and_then(Value::as_array) else {
        return models;
    };
    for entry in data {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || id.chars().count() > MAX_MODEL_ID_CHARS || !seen.insert(id.to_string())
        {
            continue;
        }
        models.push(id.to_string());
        if models.len() >= MAX_MODELS {
            break;
        }
    }
    models
}

pub fn optimizer_payload(config: &PromptOptimizationConfig, text: &str) -> Value {
    let instruction = config.instruction.trim();
    let instruction = if instruction.is_empty() {
        DEFAULT_OPTIMIZER_INSTRUCTION
    } else {
        instruction
    };
    json!({
        "model": config.model.trim(),
        "messages": [
            {"role": "system", "content": instruction},
            {"role": "user", "content": text},
        ],
        "max_tokens": MAX_TOKENS,
        "temperature": 0.7,
    })
}

/// Optimizes a user prompt through the configured OpenAI-compatible
/// Chat Completions API and returns the rewritten prompt. All returned error
/// messages are sanitized so the API key never reaches the renderer or logs.
pub async fn optimize_prompt(
    client: &Client,
    config: &PromptOptimizationConfig,
    text: &str,
) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("请输入要优化的提示词".to_string());
    }
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err(format!("提示词过长，最多支持 {MAX_INPUT_CHARS} 个字符"));
    }
    let endpoint = chat_completions_endpoint(config)?;
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    if config.model.trim().is_empty() {
        return Err("请先配置优化模型".to_string());
    }

    let payload = optimizer_payload(config, text);
    let response = post_chat_completions(client, &endpoint, api_key, &payload).await?;
    let status = response.status().as_u16();

    // 404 且用户填的是缺少 /v1 前缀的纯 base 地址时自动补 /v1 重试，
    // 与线路测试一致；已带 /v1 或已填完整端点时不再追加。
    let base_url = config.base_url.trim().trim_end_matches('/');
    if status == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = format!("{base_url}/v1/chat/completions");
        let v1_response = post_chat_completions(client, &v1_endpoint, api_key, &payload).await?;
        return parse_optimized_response(v1_response, &v1_endpoint, api_key).await;
    }

    parse_optimized_response(response, &endpoint, api_key).await
}

/// Sends a minimal Chat Completions request to verify connectivity and
/// credentials. Returns the HTTP status, endpoint and a sanitized response
/// preview so the console can show the outcome without a full optimization.
pub async fn test_configuration(
    client: &Client,
    config: &PromptOptimizationConfig,
) -> Result<Value, String> {
    let endpoint = chat_completions_endpoint(config)?;
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    let model = config.model.trim();
    if model.is_empty() {
        return Err("请先配置优化模型".to_string());
    }
    let payload = json!({
        "model": model,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 16,
    });
    let response = post_chat_completions(client, &endpoint, api_key, &payload).await?;
    let status = response.status().as_u16();

    let base_url = config.base_url.trim().trim_end_matches('/');
    if status == 404 && should_retry_with_v1(base_url) {
        let v1_endpoint = format!("{base_url}/v1/chat/completions");
        let v1_response = post_chat_completions(client, &v1_endpoint, api_key, &payload).await?;
        return configuration_test_result(v1_response, &v1_endpoint, api_key).await;
    }

    configuration_test_result(response, &endpoint, api_key).await
}

async fn configuration_test_result(
    response: reqwest::Response,
    endpoint: &str,
    api_key: &str,
) -> Result<Value, String> {
    let status = response.status().as_u16();
    let preview = response_body_preview(response, api_key).await?;
    let preview = sanitize_error(&preview, api_key)
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

async fn post_chat_completions(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    payload: &Value,
) -> Result<reqwest::Response, String> {
    client
        .post(endpoint)
        .bearer_auth(api_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(payload)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            sanitize_error(
                &format!("请求优化 API 失败：{}", format_error_chain(&error)),
                api_key,
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
    api_key: &str,
) -> Result<String, String> {
    let status = response.status().as_u16();
    if status >= 400 {
        let detail = sanitize_error(&response_body_preview(response, api_key).await?, api_key);
        let detail: String = detail.chars().take(200).collect();
        return Err(format!(
            "优化 API 请求失败（HTTP {status}，{endpoint}）：{detail}"
        ));
    }
    let body = read_bounded_body(response, MAX_RESPONSE_BYTES, "优化 API 响应", api_key).await?;
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| "优化 API 返回的不是有效的 JSON".to_string())?;
    let optimized =
        extract_optimized_text(&value).ok_or_else(|| "优化 API 响应中缺少优化结果".to_string())?;
    let optimized = optimized.trim();
    if optimized.is_empty() {
        return Err("优化 API 返回了空的优化结果".to_string());
    }
    Ok(optimized.chars().take(MAX_OUTPUT_CHARS).collect())
}

/// Extracts `choices[0].message.content`, accepting both the plain string
/// form and the newer part-array form (`{type:"text",text:...}`).
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
            "https://api.example.com/models"
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
    fn sanitize_error_hides_the_api_key() {
        assert_eq!(
            sanitize_error("401 unauthorized for sk-test-key", "sk-test-key"),
            "401 unauthorized for ***"
        );
        assert_eq!(sanitize_error("boom", ""), "boom");
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
            let mut request = [0_u8; 8192];
            let bytes_read = socket.read(&mut request).await.unwrap();
            assert!(bytes_read > 0);
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
