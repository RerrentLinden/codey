use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvocationMode {
    Sync,
    #[default]
    Async,
    Stream,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraceContext {
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

impl TraceContext {
    pub(crate) fn new(parent_id: Option<String>) -> Self {
        Self {
            trace_id: format!("{:032x}", Uuid::new_v4().as_u128()),
            parent_id,
        }
    }

    pub(crate) fn normalized(
        trace_id: Option<&str>,
        parent_id: Option<&str>,
    ) -> Result<Self, String> {
        let trace_id = trace_id.map(str::trim).filter(|value| !value.is_empty());
        let parent_id = parent_id.map(str::trim).filter(|value| !value.is_empty());
        if let Some(trace_id) = trace_id {
            validate_identifier("trace_id", trace_id, 128)?;
        }
        if let Some(parent_id) = parent_id {
            validate_identifier("parent_id", parent_id, 128)?;
        }
        Ok(Self {
            trace_id: trace_id
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{:032x}", Uuid::new_v4().as_u128())),
            parent_id: parent_id.map(ToString::to_string),
        })
    }
}

fn validate_identifier(name: &str, value: &str, max_chars: usize) -> Result<(), String> {
    if value.chars().count() > max_chars
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!(
            "{name} 只能包含 ASCII 字母、数字、连字符或下划线，且不能超过 {max_chars} 个字符"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_context_rejects_opaque_or_oversized_identifiers() {
        assert!(TraceContext::normalized(Some("trace-123"), Some("parent_1")).is_ok());
        assert!(TraceContext::normalized(Some("trace/123"), None).is_err());
        assert!(TraceContext::normalized(Some(&"a".repeat(129)), None).is_err());
    }
}
