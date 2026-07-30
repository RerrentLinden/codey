use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, StatusCode, redirect};
use serde_json::{Value, json};

use super::channels::{NotificationChannelAdapter, adapter_for};
use super::{NotificationChannelConfig, NotificationEvent};

#[derive(Debug)]
pub struct NotificationDeliveryError {
    message: String,
    uncertain: bool,
}

impl NotificationDeliveryError {
    fn definitive(message: String) -> Self {
        Self {
            message,
            uncertain: false,
        }
    }

    fn uncertain(message: String) -> Self {
        Self {
            message,
            uncertain: true,
        }
    }

    pub fn is_uncertain(&self) -> bool {
        self.uncertain
    }
}

impl std::fmt::Display for NotificationDeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NotificationDeliveryError {}

#[derive(Clone)]
pub struct NotificationDispatcher {
    client: Client,
    config: NotificationChannelConfig,
}

impl NotificationDispatcher {
    #[cfg(test)]
    pub fn new(config: NotificationChannelConfig) -> Result<Self> {
        let client = notification_http_client()?;
        Ok(Self::with_client(client, config))
    }

    pub fn with_client(client: Client, config: NotificationChannelConfig) -> Self {
        Self { client, config }
    }

    pub async fn send(
        &self,
        event: &NotificationEvent,
    ) -> std::result::Result<(), NotificationDeliveryError> {
        self.send_with_attempts(event, 2).await
    }

    async fn send_with_attempts(
        &self,
        event: &NotificationEvent,
        attempts: u32,
    ) -> std::result::Result<(), NotificationDeliveryError> {
        if !self.config.enabled || !self.config.is_configured() {
            return Ok(());
        }
        let adapter = adapter_for(&self.config);
        let mut last_error = None;
        for attempt in 0..attempts.max(1) {
            let request = adapter
                .build_request(&self.client, event)
                .map_err(|error| {
                    NotificationDeliveryError::definitive(format!(
                        "{}消息发送失败：{}",
                        adapter.display_name(),
                        adapter.sanitize_error(&error.to_string())
                    ))
                })?;
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    match response.text().await {
                        Ok(response_body) => {
                            match validate_http_response(adapter.as_ref(), status, &response_body) {
                                Ok(()) => return Ok(()),
                                Err(error) => last_error = Some(error),
                            }
                        }
                        Err(error) => {
                            return Err(NotificationDeliveryError::uncertain(format!(
                                "{}消息发送结果不确定，已停止自动重试：{}",
                                adapter.display_name(),
                                adapter.sanitize_error(&format!(
                                    "{}响应读取失败：{error}",
                                    adapter.display_name()
                                ))
                            )));
                        }
                    }
                }
                Err(error) => {
                    if error.is_timeout() || !error.is_connect() {
                        return Err(NotificationDeliveryError::uncertain(format!(
                            "{}消息发送结果不确定，已停止自动重试：{}",
                            adapter.display_name(),
                            adapter.sanitize_error(&error.to_string())
                        )));
                    }
                    last_error = Some(adapter.sanitize_error(&error.to_string()));
                }
            }
            if attempt + 1 < attempts.max(1) {
                tokio::time::sleep(Duration::from_millis(250 * 2u64.pow(attempt))).await;
            }
        }
        let error = last_error.unwrap_or_else(|| "未知错误".to_string());
        Err(NotificationDeliveryError::definitive(format!(
            "{}消息发送失败：{}",
            adapter.display_name(),
            adapter.sanitize_error(&error)
        )))
    }

    pub async fn test(&self) -> Result<Value> {
        let adapter = adapter_for(&self.config);
        if let Some(error) = adapter.configuration_error() {
            anyhow::bail!(error);
        }
        drop(adapter);

        let event = NotificationEvent::new(
            "codey.test",
            "test-session",
            "test-profile",
            "Codex",
            0,
            None,
        )
        .with_session_name("通知渠道测试")
        .with_reasoning_effort("high");
        let mut tester = self.clone();
        tester.config.enabled = true;
        // A test click must finish promptly and report the real first error.
        // Background notifications only retry failures known not to be timeouts.
        tester
            .send_with_attempts(&event, 1)
            .await
            .map_err(anyhow::Error::new)?;
        Ok(json!({"status":"ok", "eventId": event.event_id}))
    }
}

pub(crate) fn notification_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .redirect(redirect::Policy::none())
        .build()
        .context("创建通知 HTTP 客户端失败")
}

fn validate_http_response(
    adapter: &dyn NotificationChannelAdapter,
    status: StatusCode,
    body: &str,
) -> std::result::Result<(), String> {
    let channel_result = adapter.validate_response(body);
    if status.is_success() {
        return channel_result.map_err(|error| adapter.sanitize_error(&error));
    }

    let error = match channel_result {
        Ok(()) => format!("{}返回 HTTP {status}", adapter.display_name()),
        Err(detail) => format!("{}返回 HTTP {status}：{detail}", adapter.display_name()),
    };
    Err(adapter.sanitize_error(&error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::NotificationChannelKind;

    #[tokio::test]
    async fn test_requires_a_configured_channel() {
        let dispatcher = NotificationDispatcher::new(NotificationChannelConfig::default()).unwrap();
        assert!(
            dispatcher
                .test()
                .await
                .unwrap_err()
                .to_string()
                .contains("Webhook")
        );
    }

    #[tokio::test]
    async fn an_ambiguous_http_timeout_is_never_retried() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));
        let server_request_count = Arc::clone(&request_count);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let bytes_read = socket.read(&mut request).await.unwrap();
            assert!(bytes_read > 0);
            server_request_count.fetch_add(1, Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(150)).await;
            drop(socket);

            if let Ok(Ok((_retry, _))) =
                tokio::time::timeout(Duration::from_millis(400), listener.accept()).await
            {
                server_request_count.fetch_add(1, Ordering::AcqRel);
            }
        });
        let client = Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let config = NotificationChannelConfig {
            id: "timeout-feishu".to_string(),
            kind: NotificationChannelKind::Feishu,
            enabled: true,
            url: format!("http://{address}"),
            allow_insecure_test_url: true,
            ..NotificationChannelConfig::default()
        };
        let dispatcher = NotificationDispatcher::with_client(client, config);
        let event = NotificationEvent::new(
            "session.waiting",
            "session-timeout",
            "profile-timeout",
            "Codex",
            0,
            None,
        );

        let error = dispatcher.send(&event).await.unwrap_err();

        assert!(error.is_uncertain());
        assert!(error.to_string().contains("已停止自动重试"));
        server.await.unwrap();
        assert_eq!(request_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn notification_client_never_follows_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let redirect_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_address = redirect_listener.local_addr().unwrap();
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_address = target_listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = redirect_listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_address}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let response = notification_http_client()
            .unwrap()
            .post(format!("http://{redirect_address}/webhook"))
            .body("notification-secret")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        server.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), target_listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn successful_http_status_still_requires_valid_channel_confirmation() {
        let config = NotificationChannelConfig {
            kind: NotificationChannelKind::Feishu,
            url: "https://open.feishu.cn/open-apis/bot/v2/hook/secret".to_string(),
            ..NotificationChannelConfig::default()
        };
        let adapter = adapter_for(&config);

        let error =
            validate_http_response(adapter.as_ref(), StatusCode::OK, "not json").unwrap_err();

        assert!(error.contains("无法解析"));
    }

    #[test]
    fn response_errors_are_bounded_and_do_not_expose_credentials() {
        let secret_url = "https://open.feishu.cn/open-apis/bot/v2/hook/private-secret";
        let config = NotificationChannelConfig {
            kind: NotificationChannelKind::Feishu,
            url: secret_url.to_string(),
            ..NotificationChannelConfig::default()
        };
        let adapter = adapter_for(&config);
        let response = serde_json::json!({
            "code": 19021,
            "msg": format!("{secret_url} {}", "x".repeat(500)),
        })
        .to_string();

        let error = validate_http_response(adapter.as_ref(), StatusCode::BAD_REQUEST, &response)
            .unwrap_err();

        assert!(!error.contains(secret_url));
        assert!(error.contains("***"));
        assert!(error.chars().count() < 300);
    }
}
