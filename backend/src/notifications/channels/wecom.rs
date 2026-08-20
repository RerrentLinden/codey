use anyhow::Result;
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use super::{NotificationChannelAdapter, bounded_remote_message};
use crate::notifications::formatting::{format_duration, format_timestamp, markdown_text_value};
use crate::notifications::{NotificationChannelConfig, NotificationEvent};

pub(super) struct WecomChannel<'a> {
    config: &'a NotificationChannelConfig,
}

impl<'a> WecomChannel<'a> {
    pub(super) fn new(config: &'a NotificationChannelConfig) -> Self {
        Self { config }
    }
}

impl NotificationChannelAdapter for WecomChannel<'_> {
    fn display_name(&self) -> &'static str {
        "企业微信"
    }

    fn configuration_error(&self) -> Option<&'static str> {
        self.config.wecom_webhook_url().err()
    }

    fn build_request(&self, client: &Client, event: &NotificationEvent) -> Result<RequestBuilder> {
        let url = self
            .config
            .wecom_webhook_url()
            .map_err(anyhow::Error::msg)?;
        Ok(client
            .post(url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&wecom_body(event)))
    }

    fn validate_response(&self, body: &str) -> std::result::Result<(), String> {
        validate_wecom_response(body)
    }

    fn sanitize_error(&self, error: &str) -> String {
        let url = self.config.url.trim();
        if url.is_empty() {
            return error.to_string();
        }

        let mut sanitized = error.replace(url, "***");
        if let Ok(normalized) = reqwest::Url::parse(url) {
            sanitized = sanitized.replace(normalized.as_str(), "***");
            for (name, value) in normalized.query_pairs() {
                if name == "key" && !value.is_empty() {
                    sanitized = sanitized.replace(value.as_ref(), "***");
                }
            }
        }
        sanitized
    }
}

fn wecom_body(event: &NotificationEvent) -> Value {
    json!({
        "msgtype": "markdown",
        "markdown": {
            "content": wecom_markdown(event),
        },
    })
}

fn wecom_markdown(event: &NotificationEvent) -> String {
    let (icon, title, color) = match event.event.as_str() {
        "session.completed" => ("✅", "Codex 会话完成", "info"),
        "session.failed" => ("❌", "Codex 会话失败", "warning"),
        "session.waiting" => ("⏳", "Codex 会话等待介入", "warning"),
        "codey.test" => ("🔔", "Codey 通知测试", "info"),
        _ => ("🔔", "Codex 会话通知", "comment"),
    };
    let session_name = markdown_text_value(&event.session_name, "未命名会话");
    let model = markdown_text_value(&event.model, "Codex");
    let reasoning_effort = markdown_text_value(&event.reasoning_effort, "默认");
    let sent_at = markdown_text_value(&format_timestamp(&event.timestamp), "未知");
    format!(
        "{icon} **{title}**\n>会话标题：<font color=\"{color}\">{session_name}</font>\n>使用模型：{model}\n>推理深度：{reasoning_effort}\n>发送时间：{sent_at}\n>耗时：{}",
        format_duration(event.duration_ms)
    )
}

fn validate_wecom_response(body: &str) -> std::result::Result<(), String> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|_| "企业微信机器人返回了无法解析的响应".to_string())?;
    let code = value
        .get("errcode")
        .and_then(Value::as_i64)
        .ok_or_else(|| "企业微信机器人响应缺少状态码".to_string())?;
    if code == 0 {
        return Ok(());
    }
    let message = value
        .get("errmsg")
        .and_then(Value::as_str)
        .unwrap_or("未知错误");
    Err(format!(
        "企业微信机器人返回错误 {code}：{}",
        bounded_remote_message(message)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::{NotificationChannelKind, NotificationEvent};

    #[test]
    fn message_uses_wecom_markdown_schema_without_internal_ids() {
        let mut event =
            NotificationEvent::new("session.completed", "s1", "p1", "gpt-5.4", 61_000, None)
                .with_session_name("发布 <@all> **Codey**")
                .with_reasoning_effort("high");
        event.timestamp = "2026-07-21 20:30:00".to_string();

        let body = wecom_body(&event);
        assert_eq!(body["msgtype"], "markdown");
        let content = body["markdown"]["content"].as_str().unwrap();
        assert!(content.contains("Codex 会话完成"));
        assert!(content.contains("发布 ＜@all＞ \\*\\*Codey\\*\\*"));
        assert!(content.contains("gpt-5.4"));
        assert!(content.contains("1 分 1 秒"));
        assert!(!content.contains("s1"));
        assert!(!content.contains("p1"));
    }

    #[test]
    fn request_posts_json_to_the_configured_webhook() {
        let config = NotificationChannelConfig {
            kind: NotificationChannelKind::Wecom,
            url: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=request-secret".to_string(),
            ..NotificationChannelConfig::default()
        };
        let event = NotificationEvent::new("codey.test", "s1", "p1", "Codex", 0, None);
        let request = WecomChannel::new(&config)
            .build_request(&Client::new(), &event)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().host_str(), Some("qyapi.weixin.qq.com"));
        assert_eq!(request.url().path(), "/cgi-bin/webhook/send");
        assert_eq!(request.url().query(), Some("key=request-secret"));
        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .unwrap();
        assert_eq!(body["msgtype"], "markdown");
        assert!(
            body["markdown"]["content"]
                .as_str()
                .unwrap()
                .contains("Codey 通知测试")
        );
    }

    #[test]
    fn event_kinds_have_distinct_titles() {
        for (kind, title) in [
            ("session.failed", "Codex 会话失败"),
            ("session.waiting", "Codex 会话等待介入"),
            ("codey.test", "Codey 通知测试"),
        ] {
            let event = NotificationEvent::new(kind, "s1", "p1", "Codex", 0, None);
            assert!(wecom_markdown(&event).contains(title));
        }
    }

    #[test]
    fn response_requires_an_explicit_success_code() {
        assert!(validate_wecom_response(r#"{"errcode":0,"errmsg":"ok"}"#).is_ok());
        assert!(
            validate_wecom_response(r#"{"errcode":93000,"errmsg":"invalid webhook url"}"#)
                .unwrap_err()
                .contains("93000")
        );
        assert!(validate_wecom_response("not json").is_err());
        assert!(validate_wecom_response(r#"{"errmsg":"ok"}"#).is_err());
    }

    #[test]
    fn configuration_requires_a_valid_webhook_url() {
        let config = NotificationChannelConfig {
            kind: NotificationChannelKind::Wecom,
            ..NotificationChannelConfig::default()
        };
        let channel = WecomChannel::new(&config);
        assert_eq!(
            channel.configuration_error(),
            Some("请先填写企业微信机器人 Webhook 地址")
        );
    }

    #[test]
    fn transport_errors_do_not_expose_the_webhook_key() {
        let secret = "wecom-secret-key";
        let config = NotificationChannelConfig {
            kind: NotificationChannelKind::Wecom,
            url: format!("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key={secret}"),
            ..NotificationChannelConfig::default()
        };
        let channel = WecomChannel::new(&config);
        let error = channel.sanitize_error(&format!(
            "request to {} failed; remote echoed {secret}",
            config.url
        ));

        assert!(!error.contains(secret));
        assert!(!error.contains("qyapi.weixin.qq.com"));
        assert!(error.contains("***"));
    }
}
