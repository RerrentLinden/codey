use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_NOTIFICATION_CHANNELS: usize = 32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum NotificationChannelKind {
    #[default]
    Feishu,
    Wecom,
    Telegram,
    WechatClaw,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationChannelConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: NotificationChannelKind,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub url_configured: bool,
    #[serde(default, skip_serializing)]
    pub clear_url: bool,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub bot_token_configured: bool,
    #[serde(default, skip_serializing)]
    pub clear_bot_token: bool,
    #[serde(default)]
    pub chat_id: String,
    #[cfg(test)]
    #[serde(skip)]
    pub allow_insecure_test_url: bool,
}

impl Default for NotificationChannelConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            kind: NotificationChannelKind::Feishu,
            enabled: true,
            url: String::new(),
            url_configured: false,
            clear_url: false,
            bot_token: String::new(),
            bot_token_configured: false,
            clear_bot_token: false,
            chat_id: String::new(),
            #[cfg(test)]
            allow_insecure_test_url: false,
        }
    }
}

impl NotificationChannelConfig {
    pub fn is_configured(&self) -> bool {
        match self.kind {
            NotificationChannelKind::Feishu => !self.url.trim().is_empty(),
            NotificationChannelKind::Wecom => !self.url.trim().is_empty(),
            NotificationChannelKind::Telegram => {
                !self.bot_token.trim().is_empty() && !self.chat_id.trim().is_empty()
            }
            NotificationChannelKind::WechatClaw => {
                !self.bot_token.trim().is_empty()
                    && !self.chat_id.trim().is_empty()
                    && self.wechat_claw_base_url().is_ok()
            }
        }
    }

    pub(crate) fn feishu_webhook_url(&self) -> Result<reqwest::Url, &'static str> {
        const INVALID_URL: &str =
            "飞书机器人 Webhook 必须使用 HTTPS 地址和 /open-apis/bot/v2/hook/... 路径";
        let value = self.url.trim();
        if value.is_empty() {
            return Err("请先填写飞书机器人 Webhook 地址");
        }
        let url = reqwest::Url::parse(value).map_err(|_| INVALID_URL)?;
        #[cfg(test)]
        if self.allow_insecure_test_url {
            return Ok(url);
        }
        let hook = url
            .path()
            .strip_prefix("/open-apis/bot/v2/hook/")
            .filter(|hook| !hook.is_empty() && !hook.contains('/'));
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || hook.is_none()
        {
            return Err(INVALID_URL);
        }
        Ok(url)
    }

    pub(crate) fn wecom_webhook_url(&self) -> Result<reqwest::Url, &'static str> {
        const INVALID_URL: &str = "企业微信机器人 Webhook 必须使用 https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=... 地址";
        let value = self.url.trim();
        if value.is_empty() {
            return Err("请先填写企业微信机器人 Webhook 地址");
        }
        let url = reqwest::Url::parse(value).map_err(|_| INVALID_URL)?;
        #[cfg(test)]
        if self.allow_insecure_test_url {
            return Ok(url);
        }
        let query = url.query_pairs().collect::<Vec<_>>();
        let valid_key = query.len() == 1 && query[0].0 == "key" && !query[0].1.trim().is_empty();
        if url.scheme() != "https"
            || url.host_str() != Some("qyapi.weixin.qq.com")
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/cgi-bin/webhook/send"
            || url.fragment().is_some()
            || !valid_key
        {
            return Err(INVALID_URL);
        }
        Ok(url)
    }

    pub(crate) fn wechat_claw_base_url(&self) -> Result<reqwest::Url, &'static str> {
        const INVALID_URL: &str = "微信 ClawBot 服务地址必须是腾讯 iLink 的 HTTPS 根地址";
        let value = self.url.trim();
        if value.is_empty() {
            return Err("请先通过扫码登录微信 ClawBot");
        }
        let url = reqwest::Url::parse(value).map_err(|_| INVALID_URL)?;
        let host = url.host_str().unwrap_or_default();
        let official_host = host == "ilinkai.weixin.qq.com" || host.ends_with(".weixin.qq.com");
        if url.scheme() != "https"
            || !official_host
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(INVALID_URL);
        }
        Ok(url)
    }
}

/// Notification settings retain the historic `webhook` wire name in
/// `CodeyConfig` so existing installations and renderer calls remain valid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebhookConfig {
    // Read the pre-channel-list format and migrate it in `normalize`. These
    // fields are deliberately omitted once the new format is serialized.
    #[serde(default)]
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(default)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default)]
    pub channels: Vec<NotificationChannelConfig>,
}

impl WebhookConfig {
    pub(crate) fn normalize(&mut self) {
        if self.channels.is_empty() && (self.enabled || !self.url.trim().is_empty()) {
            self.channels.push(NotificationChannelConfig {
                id: "legacy-feishu".to_string(),
                kind: NotificationChannelKind::Feishu,
                enabled: self.enabled,
                url: self.url.trim().to_string(),
                ..NotificationChannelConfig::default()
            });
        }
        self.enabled = false;
        self.url.clear();

        let mut ids = BTreeSet::new();
        for channel in &mut self.channels {
            channel.id = channel.id.trim().to_string();
            if channel.id.is_empty() || !ids.insert(channel.id.clone()) {
                channel.id = Uuid::new_v4().to_string();
                ids.insert(channel.id.clone());
            }
            channel.url = channel.url.trim().to_string();
            channel.url_configured = !channel.url.is_empty();
            channel.clear_url = false;
            channel.bot_token = channel.bot_token.trim().to_string();
            channel.chat_id = channel.chat_id.trim().to_string();
            channel.bot_token_configured = !channel.bot_token.is_empty();
            channel.clear_bot_token = false;
        }
    }

    pub fn enabled_channel_count(&self) -> usize {
        self.channels
            .iter()
            .filter(|channel| channel.enabled && channel.is_configured())
            .count()
    }

    pub fn has_enabled_channel(&self) -> bool {
        self.enabled_channel_count() > 0
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.channels.len() > MAX_NOTIFICATION_CHANNELS {
            return Err(format!(
                "通知渠道最多只能配置 {MAX_NOTIFICATION_CHANNELS} 个"
            ));
        }
        for channel in &self.channels {
            if channel.url.trim().is_empty() {
                continue;
            }
            match channel.kind {
                NotificationChannelKind::Feishu => {
                    channel.feishu_webhook_url().map_err(ToString::to_string)?;
                }
                NotificationChannelKind::Wecom => {
                    channel.wecom_webhook_url().map_err(ToString::to_string)?;
                }
                NotificationChannelKind::Telegram => {}
                NotificationChannelKind::WechatClaw => {
                    if !channel.url.trim().is_empty() {
                        channel
                            .wechat_claw_base_url()
                            .map_err(ToString::to_string)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn merge_redacted_secrets(&mut self, previous: &Self) {
        for channel in &mut self.channels {
            match channel.kind {
                NotificationChannelKind::Feishu | NotificationChannelKind::Wecom => {
                    let kind = channel.kind;
                    if channel.clear_url {
                        channel.url.clear();
                        channel.url_configured = false;
                        continue;
                    }
                    if !channel.url.trim().is_empty() || !channel.url_configured {
                        continue;
                    }
                    if let Some(existing) = previous
                        .channels
                        .iter()
                        .find(|existing| existing.id == channel.id && existing.kind == kind)
                    {
                        channel.url = existing.url.clone();
                    }
                }
                NotificationChannelKind::Telegram | NotificationChannelKind::WechatClaw => {
                    let kind = channel.kind;
                    if kind == NotificationChannelKind::WechatClaw
                        && channel.url.trim().is_empty()
                        && channel.url_configured
                        && let Some(existing) = previous
                            .channels
                            .iter()
                            .find(|existing| existing.id == channel.id && existing.kind == kind)
                    {
                        channel.url = existing.url.clone();
                    }
                    if channel.clear_bot_token {
                        channel.bot_token.clear();
                        channel.bot_token_configured = false;
                        continue;
                    }
                    if !channel.bot_token.trim().is_empty() || !channel.bot_token_configured {
                        continue;
                    }
                    if let Some(existing) = previous
                        .channels
                        .iter()
                        .find(|existing| existing.id == channel.id && existing.kind == kind)
                    {
                        channel.bot_token = existing.bot_token.clone();
                    }
                }
            }
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_multiple_channel_kinds() {
        let mut config = serde_json::from_str::<WebhookConfig>(
            r#"{
                "channels":[
                    {"id":"feishu-1","kind":"feishu","enabled":true,"url":"https://open.feishu.cn/example"},
                    {"id":"wecom-1","kind":"wecom","enabled":true,"url":"https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=preview"},
                    {"id":"telegram-1","kind":"telegram","enabled":true,"botToken":"123:token","chatId":"-100123"}
                ]
            }"#,
        )
        .unwrap();
        config.normalize();

        assert_eq!(config.channels.len(), 3);
        assert_eq!(config.enabled_channel_count(), 3);
        assert!(config.has_enabled_channel());
        assert!(config.channels[0].url_configured);
        assert_eq!(config.channels[1].kind, NotificationChannelKind::Wecom);
        assert!(config.channels[1].url_configured);
        assert_eq!(config.channels[2].kind, NotificationChannelKind::Telegram);
        assert!(config.channels[2].bot_token_configured);
    }

    #[test]
    fn rejects_channel_lists_above_the_resource_limit() {
        let config = WebhookConfig {
            channels: (0..=MAX_NOTIFICATION_CHANNELS)
                .map(|_| NotificationChannelConfig::default())
                .collect(),
            ..WebhookConfig::default()
        };

        assert_eq!(
            config.validate().unwrap_err(),
            format!("通知渠道最多只能配置 {MAX_NOTIFICATION_CHANNELS} 个")
        );
    }

    #[test]
    fn feishu_webhooks_accept_custom_https_hosts() {
        for accepted in [
            "https://open.feishu.cn/open-apis/bot/v2/hook/secret",
            "https://open.larksuite.com/open-apis/bot/v2/hook/secret",
            "https://open.feishu.cn:443/open-apis/bot/v2/hook/secret",
            "https://feishu.corp.example/open-apis/bot/v2/hook/secret",
        ] {
            let config = NotificationChannelConfig {
                url: accepted.to_string(),
                ..NotificationChannelConfig::default()
            };
            assert!(config.feishu_webhook_url().is_ok(), "{accepted}");
        }

        for rejected in [
            "http://open.feishu.cn/open-apis/bot/v2/hook/secret",
            "https://feishu.corp.example:8443/open-apis/bot/v2/hook/secret",
            "https://open.feishu.cn/other/path",
            "https://open.feishu.cn/open-apis/bot/v2/hook/secret?redirect=1",
            "https://user@open.feishu.cn/open-apis/bot/v2/hook/secret",
        ] {
            let config = NotificationChannelConfig {
                url: rejected.to_string(),
                ..NotificationChannelConfig::default()
            };
            assert!(config.feishu_webhook_url().is_err(), "{rejected}");
        }
    }

    #[test]
    fn wecom_webhooks_require_the_official_robot_endpoint() {
        for accepted in [
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret",
            "https://qyapi.weixin.qq.com:443/cgi-bin/webhook/send?key=secret-value",
        ] {
            let config = NotificationChannelConfig {
                kind: NotificationChannelKind::Wecom,
                url: accepted.to_string(),
                ..NotificationChannelConfig::default()
            };
            assert!(config.wecom_webhook_url().is_ok(), "{accepted}");
        }

        for rejected in [
            "http://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret",
            "https://qyapi.weixin.qq.com:8443/cgi-bin/webhook/send?key=secret",
            "https://example.com/cgi-bin/webhook/send?key=secret",
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/upload_media?key=secret",
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send",
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=",
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret&debug=1",
            "https://user@qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret",
        ] {
            let config = NotificationChannelConfig {
                kind: NotificationChannelKind::Wecom,
                url: rejected.to_string(),
                ..NotificationChannelConfig::default()
            };
            assert!(config.wecom_webhook_url().is_err(), "{rejected}");
        }
    }

    #[test]
    fn redacted_feishu_url_is_restored_when_other_settings_are_saved() {
        let previous = WebhookConfig {
            channels: vec![NotificationChannelConfig {
                id: "feishu-1".to_string(),
                kind: NotificationChannelKind::Feishu,
                enabled: true,
                url: "https://open.feishu.cn/open-apis/bot/v2/hook/secret".to_string(),
                url_configured: true,
                ..NotificationChannelConfig::default()
            }],
            ..WebhookConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.channels[0].url.clear();
        incoming.merge_redacted_secrets(&previous);

        assert_eq!(
            incoming.channels[0].url,
            "https://open.feishu.cn/open-apis/bot/v2/hook/secret"
        );
    }

    #[test]
    fn explicit_feishu_url_clear_does_not_restore_the_previous_secret() {
        let previous = WebhookConfig {
            channels: vec![NotificationChannelConfig {
                id: "feishu-1".to_string(),
                kind: NotificationChannelKind::Feishu,
                url: "https://open.feishu.cn/open-apis/bot/v2/hook/secret".to_string(),
                url_configured: true,
                ..NotificationChannelConfig::default()
            }],
            ..WebhookConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.channels[0].url.clear();
        incoming.channels[0].clear_url = true;
        incoming.merge_redacted_secrets(&previous);

        assert!(incoming.channels[0].url.is_empty());
        assert!(!incoming.channels[0].url_configured);
        assert!(
            serde_json::to_value(&incoming).unwrap()["channels"][0]
                .get("clearUrl")
                .is_none()
        );
    }

    #[test]
    fn redacted_wecom_url_is_restored_unless_explicitly_cleared() {
        let previous = WebhookConfig {
            channels: vec![NotificationChannelConfig {
                id: "wecom-1".to_string(),
                kind: NotificationChannelKind::Wecom,
                url: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=secret".to_string(),
                url_configured: true,
                ..NotificationChannelConfig::default()
            }],
            ..WebhookConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.channels[0].url.clear();
        incoming.merge_redacted_secrets(&previous);
        assert_eq!(incoming.channels[0].url, previous.channels[0].url);

        incoming.channels[0].url.clear();
        incoming.channels[0].clear_url = true;
        incoming.merge_redacted_secrets(&previous);
        assert!(incoming.channels[0].url.is_empty());
        assert!(!incoming.channels[0].url_configured);
    }

    #[test]
    fn redacted_telegram_token_is_restored_when_other_settings_are_saved() {
        let previous = WebhookConfig {
            channels: vec![NotificationChannelConfig {
                id: "telegram-1".to_string(),
                kind: NotificationChannelKind::Telegram,
                enabled: true,
                bot_token: "123:secret".to_string(),
                bot_token_configured: true,
                chat_id: "-100123".to_string(),
                ..NotificationChannelConfig::default()
            }],
            ..WebhookConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.channels[0].bot_token.clear();
        incoming.merge_redacted_secrets(&previous);

        assert_eq!(incoming.channels[0].bot_token, "123:secret");
    }

    #[test]
    fn explicit_telegram_token_clear_does_not_restore_the_previous_secret() {
        let previous = WebhookConfig {
            channels: vec![NotificationChannelConfig {
                id: "telegram-1".to_string(),
                kind: NotificationChannelKind::Telegram,
                bot_token: "123:secret".to_string(),
                bot_token_configured: true,
                chat_id: "-100123".to_string(),
                ..NotificationChannelConfig::default()
            }],
            ..WebhookConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.channels[0].bot_token.clear();
        incoming.channels[0].clear_bot_token = true;
        incoming.merge_redacted_secrets(&previous);

        assert!(incoming.channels[0].bot_token.is_empty());
        assert!(!incoming.channels[0].bot_token_configured);
        assert!(
            serde_json::to_value(&incoming).unwrap()["channels"][0]
                .get("clearBotToken")
                .is_none()
        );
    }

    #[test]
    fn wechat_claw_requires_an_official_ilink_https_root_url() {
        for accepted in [
            "https://ilinkai.weixin.qq.com",
            "https://region.weixin.qq.com/",
        ] {
            let config = NotificationChannelConfig {
                kind: NotificationChannelKind::WechatClaw,
                url: accepted.to_string(),
                ..NotificationChannelConfig::default()
            };
            assert!(config.wechat_claw_base_url().is_ok(), "{accepted}");
        }

        for rejected in [
            "http://ilinkai.weixin.qq.com",
            "https://ilinkai.weixin.qq.com:8443",
            "https://ilinkai.weixin.qq.com/ilink/bot/sendmessage",
            "https://ilinkai.weixin.qq.com?redirect=1",
            "https://weixin.qq.com.evil.example",
            "https://user@ilinkai.weixin.qq.com",
        ] {
            let config = NotificationChannelConfig {
                kind: NotificationChannelKind::WechatClaw,
                url: rejected.to_string(),
                ..NotificationChannelConfig::default()
            };
            assert!(config.wechat_claw_base_url().is_err(), "{rejected}");
        }
    }

    #[test]
    fn redacted_wechat_claw_token_is_restored_when_other_settings_are_saved() {
        let previous = WebhookConfig {
            channels: vec![NotificationChannelConfig {
                id: "wechat-claw-1".to_string(),
                kind: NotificationChannelKind::WechatClaw,
                url: "https://ilinkai.weixin.qq.com".to_string(),
                url_configured: true,
                bot_token: "ilink-secret".to_string(),
                bot_token_configured: true,
                chat_id: "user@im.wechat".to_string(),
                ..NotificationChannelConfig::default()
            }],
            ..WebhookConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.channels[0].url.clear();
        incoming.channels[0].bot_token.clear();
        incoming.merge_redacted_secrets(&previous);

        assert_eq!(incoming.channels[0].bot_token, "ilink-secret");
        assert_eq!(incoming.channels[0].url, "https://ilinkai.weixin.qq.com");
    }
}
