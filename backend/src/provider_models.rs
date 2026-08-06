use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, header::ACCEPT};
use serde_json::Value;

use crate::config::ProviderProfile;

const PROVIDER_MODEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_PROVIDER_MODEL_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_PROVIDER_MODELS: usize = 10_000;
pub(crate) const MAX_PROVIDER_MODEL_ID_BYTES: usize = 512;

#[derive(Debug)]
enum ModelListError {
    InvalidJson(serde_json::Error),
    UnsupportedFormat,
    TooManyModels { limit: usize },
    ModelIdTooLong { limit: usize },
}

impl ModelListError {
    fn allows_endpoint_fallback(&self) -> bool {
        matches!(self, Self::InvalidJson(_) | Self::UnsupportedFormat)
    }
}

impl fmt::Display for ModelListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "模型列表不是有效 JSON：{error}"),
            Self::UnsupportedFormat => formatter.write_str("上游模型列表格式不受支持"),
            Self::TooManyModels { limit } => {
                write!(formatter, "上游模型数量超过安全上限 {limit}")
            }
            Self::ModelIdTooLong { limit } => {
                write!(formatter, "上游模型 ID 超过安全上限 {limit} 字节")
            }
        }
    }
}

impl std::error::Error for ModelListError {}

pub async fn fetch(profile: &ProviderProfile, client: &Client) -> Result<Vec<String>> {
    let base = profile.normalized_base_url();
    if base.is_empty() {
        anyhow::bail!("API 地址不能为空");
    }
    let endpoints = model_endpoints(&base)?;
    for (index, endpoint) in endpoints.iter().enumerate() {
        let mut request = client.get(endpoint).header(ACCEPT, "application/json");
        if !profile.api_key.trim().is_empty() {
            request = request.bearer_auth(profile.api_key.trim());
        }
        let response = request
            .timeout(PROVIDER_MODEL_REQUEST_TIMEOUT)
            .send()
            .await
            .with_context(|| format!("获取上游模型失败：{endpoint}"))?;
        let status = response.status();
        let has_fallback = index + 1 < endpoints.len();
        if matches!(
            status,
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
        ) && has_fallback
        {
            continue;
        }
        if !status.is_success() {
            anyhow::bail!("获取上游模型失败：{endpoint} 返回 HTTP {status}");
        }
        let body = read_bounded_body(response, endpoint).await?;
        match model_ids(&body) {
            Ok(models) => return Ok(models),
            Err(error) if has_fallback && error.allows_endpoint_fallback() => continue,
            Err(error) => {
                return Err(anyhow::Error::new(error))
                    .with_context(|| format!("解析上游模型列表失败：{endpoint}"));
            }
        }
    }
    anyhow::bail!("上游没有返回可用的模型列表")
}

async fn read_bounded_body(mut response: reqwest::Response, endpoint: &str) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROVIDER_MODEL_RESPONSE_BYTES as u64)
    {
        anyhow::bail!(
            "上游模型列表响应超过安全上限 {} 字节：{endpoint}",
            MAX_PROVIDER_MODEL_RESPONSE_BYTES
        );
    }

    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(MAX_PROVIDER_MODEL_RESPONSE_BYTES);
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("读取上游模型列表失败：{endpoint}"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_MODEL_RESPONSE_BYTES {
            anyhow::bail!(
                "上游模型列表响应超过安全上限 {} 字节：{endpoint}",
                MAX_PROVIDER_MODEL_RESPONSE_BYTES
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn model_endpoints(base: &str) -> Result<Vec<String>> {
    let mut url = reqwest::Url::parse(base).context("API 地址格式无效")?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("API 地址仅支持 HTTP 或 HTTPS");
    }
    url.set_query(None);
    url.set_fragment(None);
    let last_segment = url
        .path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let base = url.as_str().trim_end_matches('/');
    Ok(match last_segment {
        "models" => vec![base.to_string()],
        "v1" => vec![format!("{base}/models")],
        _ => vec![format!("{base}/v1/models"), format!("{base}/models")],
    })
}

fn model_ids(body: &[u8]) -> std::result::Result<Vec<String>, ModelListError> {
    model_ids_with_limits(body, MAX_PROVIDER_MODELS, MAX_PROVIDER_MODEL_ID_BYTES)
}

fn model_ids_with_limits(
    body: &[u8],
    max_models: usize,
    max_model_id_bytes: usize,
) -> std::result::Result<Vec<String>, ModelListError> {
    let value = serde_json::from_slice::<Value>(body).map_err(ModelListError::InvalidJson)?;
    let items = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or(ModelListError::UnsupportedFormat)?;
    let capacity = items.len().min(max_models);
    let mut models = Vec::with_capacity(capacity);
    let mut seen = HashSet::<&str>::with_capacity(capacity);
    for item in items {
        let Some(model) = item.as_str().or_else(|| {
            item.get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("name").and_then(Value::as_str))
                .or_else(|| item.get("slug").and_then(Value::as_str))
                .or_else(|| item.get("model").and_then(Value::as_str))
        }) else {
            continue;
        };
        let model = model.trim();
        if model.is_empty() || !seen.insert(model) {
            continue;
        }
        if model.len() > max_model_id_bytes {
            return Err(ModelListError::ModelIdTooLong {
                limit: max_model_id_bytes,
            });
        }
        if models.len() >= max_models {
            return Err(ModelListError::TooManyModels { limit: max_models });
        }
        models.push(model.to_string());
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn builds_compatible_model_endpoints() {
        assert_eq!(
            model_endpoints("https://relay.example/v1").unwrap(),
            vec!["https://relay.example/v1/models"]
        );
        assert_eq!(
            model_endpoints("https://relay.example/api").unwrap(),
            vec![
                "https://relay.example/api/v1/models",
                "https://relay.example/api/models"
            ]
        );
    }

    #[test]
    fn parses_common_model_list_shapes() {
        let models = model_ids(br#"{"data":[{"id":"a"},{"name":"b"},{"id":"a"}]}"#).unwrap();
        assert_eq!(models, vec!["a", "b"]);
    }

    #[test]
    fn enforces_unique_model_count_without_counting_duplicates() {
        let models =
            model_ids_with_limits(br#"{"data":[{"id":"a"},{"id":"a"},{"id":"b"}]}"#, 2, 16)
                .unwrap();
        assert_eq!(models, vec!["a", "b"]);

        let error = model_ids_with_limits(br#"{"data":[{"id":"a"},{"id":"b"},{"id":"c"}]}"#, 2, 16)
            .unwrap_err();
        assert!(matches!(error, ModelListError::TooManyModels { limit: 2 }));
    }

    #[test]
    fn rejects_model_ids_over_the_byte_limit() {
        let error = model_ids_with_limits(br#"{"models":["abcd"]}"#, 4, 3).unwrap_err();
        assert!(matches!(error, ModelListError::ModelIdTooLong { limit: 3 }));
    }

    #[tokio::test]
    async fn rejects_declared_oversized_responses_before_reading_the_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_PROVIDER_MODEL_RESPONSE_BYTES + 1
            )
            .unwrap();
        });
        let mut profile = ProviderProfile::new("test");
        profile.base_url = format!("http://{address}/v1");
        let client = Client::builder().no_proxy().build().unwrap();

        let error = fetch(&profile, &client).await.unwrap_err();

        assert!(error.to_string().contains("响应超过安全上限"));
        server.join().unwrap();
    }
}
