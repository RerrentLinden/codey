use chrono::Local;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::formatting::DISPLAY_TIMESTAMP_FORMAT;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NotificationEvent {
    pub event_id: String,
    pub event: String,
    pub timestamp: String,
    pub session_id: String,
    pub session_name: String,
    pub profile_id: String,
    pub model: String,
    pub reasoning_effort: String,
    pub duration_ms: u128,
    pub error: Option<String>,
}

impl NotificationEvent {
    pub fn new(
        event: impl Into<String>,
        session_id: impl Into<String>,
        profile_id: impl Into<String>,
        model: impl Into<String>,
        duration_ms: u128,
        error: Option<String>,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            event: event.into(),
            timestamp: Local::now().format(DISPLAY_TIMESTAMP_FORMAT).to_string(),
            session_id: session_id.into(),
            session_name: "未命名会话".to_string(),
            profile_id: profile_id.into(),
            model: model.into(),
            reasoning_effort: "默认".to_string(),
            duration_ms,
            error,
        }
    }

    pub fn with_session_name(mut self, session_name: impl Into<String>) -> Self {
        let session_name = session_name.into();
        self.session_name = if session_name.trim().is_empty() {
            "未命名会话".to_string()
        } else {
            session_name
        };
        self
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: impl Into<String>) -> Self {
        let reasoning_effort = reasoning_effort.into();
        self.reasoning_effort = if reasoning_effort.trim().is_empty() {
            "默认".to_string()
        } else {
            reasoning_effort
        };
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_have_unique_ids_and_do_not_include_prompt_fields() {
        let first = NotificationEvent::new("session.completed", "s1", "p1", "gpt-5", 10, None);
        let second = NotificationEvent::new("session.completed", "s1", "p1", "gpt-5", 10, None);
        assert_ne!(first.event_id, second.event_id);
        let value = serde_json::to_value(first).unwrap();
        assert!(value.get("prompt").is_none());
        assert!(value.get("messages").is_none());
    }
}
