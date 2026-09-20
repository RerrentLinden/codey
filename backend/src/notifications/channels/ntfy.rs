use anyhow::Result;
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use super::{NotificationChannelAdapter, bounded_remote_message, redact_secret};
use crate::notifications::formatting::{format_duration, format_timestamp, plain_text_value};
use crate::notifications::{NotificationChannelConfig, NotificationEvent};

pub(super) struct NtfyChannel<'a> {
    config: &'a NotificationChannelConfig,
}

impl<'a> NtfyChannel<'a> {
    pub(super) fn new(config: &'a NotificationChannelConfig) -> Self {
        Self { config }
    }
}

impl NotificationChannelAdapter for NtfyChannel<'_> {
    fn display_name(&self) -> &'static str {
        "ntfy"
    }

    fn configuration_error(&self) -> Option<&'static str> {
        if let Err(error) = self.config.ntfy_base_url() {
            return Some(error);
        }
        self.config.ntfy_topic().err()
    }

    fn build_request(&self, client: &Client, event: &NotificationEvent) -> Result<RequestBuilder> {
        let url = self.config.ntfy_base_url().map_err(anyhow::Error::msg)?;
        let topic = self.config.ntfy_topic().map_err(anyhow::Error::msg)?;
        let mut request = client
            .post(url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&ntfy_body(event, topic));
        let token = self.config.bot_token.trim();
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        Ok(request)
    }

    fn validate_response(&self, body: &str) -> std::result::Result<(), String> {
        validate_ntfy_response(body)
    }

    fn sanitize_error(&self, error: &str) -> String {
        redact_secret(
            &redact_secret(error, &self.config.url),
            &self.config.bot_token,
        )
    }
}

fn ntfy_body(event: &NotificationEvent, topic: &str) -> Value {
    let (icon, title, priority, tag) = match event.event.as_str() {
        "session.completed" => ("✅", "Codex 会话完成", 3, "white_check_mark"),
        "session.failed" => ("❌", "Codex 会话失败", 4, "warning"),
        "session.waiting" => ("⏳", "Codex 会话等待介入", 4, "hourglass"),
        "codey.test" => ("🔔", "Codey 通知测试", 3, "bell"),
        _ => ("🔔", "Codex 会话通知", 3, "bell"),
    };
    let session_name = plain_text_value(&event.session_name, "未命名会话");
    let model = plain_text_value(&event.model, "Codex");
    let reasoning_effort = plain_text_value(&event.reasoning_effort, "默认");
    let sent_at = plain_text_value(&format_timestamp(&event.timestamp), "未知");
    json!({
        "topic": topic,
        "title": format!("{icon} {title}"),
        "message": format!(
            "会话标题：{session_name}\n使用模型：{model}\n推理深度：{reasoning_effort}\n发送时间：{sent_at}\n耗时：{}",
            format_duration(event.duration_ms)
        ),
        "priority": priority,
        "tags": [tag],
    })
}

fn validate_ntfy_response(body: &str) -> std::result::Result<(), String> {
    let value =
        serde_json::from_str::<Value>(body).map_err(|_| "ntfy 返回了无法解析的响应".to_string())?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(format!("ntfy 返回错误：{}", bounded_remote_message(error)));
    }
    let published = value
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty());
    if published {
        return Ok(());
    }
    Err("ntfy 响应缺少消息编号".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::NotificationChannelKind;

    fn channel_config() -> NotificationChannelConfig {
        NotificationChannelConfig {
            kind: NotificationChannelKind::Ntfy,
            url: "https://ntfy.sh".to_string(),
            chat_id: "codey-topic".to_string(),
            ..NotificationChannelConfig::default()
        }
    }

    #[test]
    fn message_uses_ntfy_publish_schema_without_internal_ids() {
        let mut event =
            NotificationEvent::new("session.completed", "s1", "p1", "gpt-5.4", 61_000, None)
                .with_session_name("发布 Codey 版本")
                .with_reasoning_effort("high");
        event.timestamp = "2026-07-21 20:30:00".to_string();

        let body = ntfy_body(&event, "codey-topic");
        assert_eq!(body["topic"], "codey-topic");
        assert_eq!(body["priority"], 3);
        assert_eq!(body["tags"][0], "white_check_mark");
        let title = body["title"].as_str().unwrap();
        assert!(title.contains("Codex 会话完成"));
        let message = body["message"].as_str().unwrap();
        assert!(message.contains("发布 Codey 版本"));
        assert!(message.contains("gpt-5.4"));
        assert!(message.contains("1 分 1 秒"));
        assert!(!message.contains("s1"));
        assert!(!message.contains("p1"));
    }

    #[test]
    fn failure_and_waiting_events_raise_the_priority() {
        for kind in ["session.failed", "session.waiting"] {
            let event = NotificationEvent::new(kind, "s1", "p1", "Codex", 0, None);
            assert_eq!(ntfy_body(&event, "codey-topic")["priority"], 4);
        }
    }

    #[test]
    fn request_posts_json_to_the_server_root() {
        let config = channel_config();
        let event = NotificationEvent::new("codey.test", "s1", "p1", "Codex", 0, None);
        let request = NtfyChannel::new(&config)
            .build_request(&Client::new(), &event)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().host_str(), Some("ntfy.sh"));
        assert_eq!(request.url().path(), "/");
        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .is_none()
        );
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .unwrap();
        assert_eq!(body["topic"], "codey-topic");
        assert!(body["title"].as_str().unwrap().contains("Codey 通知测试"));
    }

    #[test]
    fn access_token_is_sent_as_a_bearer_credential() {
        let config = NotificationChannelConfig {
            bot_token: "tk_secret".to_string(),
            ..channel_config()
        };
        let event = NotificationEvent::new("codey.test", "s1", "p1", "Codex", 0, None);
        let request = NtfyChannel::new(&config)
            .build_request(&Client::new(), &event)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer tk_secret"
        );
    }

    #[test]
    fn response_requires_a_message_id() {
        assert!(validate_ntfy_response(r#"{"id":"UOJfAmgB7MLk","event":"message"}"#).is_ok());
        assert!(
            validate_ntfy_response(r#"{"code":40101,"http":401,"error":"unauthorized"}"#)
                .unwrap_err()
                .contains("unauthorized")
        );
        assert!(validate_ntfy_response("not json").is_err());
        assert!(validate_ntfy_response(r#"{"event":"message"}"#).is_err());
    }

    #[test]
    fn configuration_requires_a_server_url_and_topic() {
        let empty = NotificationChannelConfig {
            kind: NotificationChannelKind::Ntfy,
            ..NotificationChannelConfig::default()
        };
        assert_eq!(
            NtfyChannel::new(&empty).configuration_error(),
            Some("请先填写 ntfy 服务器地址")
        );

        let without_topic = NotificationChannelConfig {
            url: "https://ntfy.sh".to_string(),
            ..empty
        };
        assert_eq!(
            NtfyChannel::new(&without_topic).configuration_error(),
            Some("请先填写 ntfy 主题")
        );

        let invalid_topic = NotificationChannelConfig {
            chat_id: "bad topic!".to_string(),
            ..without_topic
        };
        assert!(
            NtfyChannel::new(&invalid_topic)
                .configuration_error()
                .unwrap()
                .contains("字母、数字")
        );
    }

    #[test]
    fn transport_errors_do_not_expose_the_access_token() {
        let config = NotificationChannelConfig {
            bot_token: "tk_private_value".to_string(),
            ..channel_config()
        };
        let channel = NtfyChannel::new(&config);

        let error = channel.sanitize_error(
            "request to https://ntfy.sh failed with Authorization: Bearer tk_private_value",
        );

        assert!(!error.contains("tk_private_value"));
        assert!(error.contains("***"));
    }
}
