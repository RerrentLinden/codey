//! Compatibility parsing for collaboration-tool responses.
//!
//! Tool providers have used several casing conventions and envelope shapes over
//! time.  Keeping that tolerance here gives the gate and lifecycle ledger one
//! definition of terminal state, interruption and spawn failure.

use serde_json::{Map, Value};

const MAX_JSON_ENCODED_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentState {
    PendingInit,
    Live,
    Terminal,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalOutcome {
    Succeeded,
    Failed,
    Lost,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalObservation {
    pub identifier: String,
    pub outcome: TerminalOutcome,
}

pub(crate) fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn classify_agent_status(value: &Value) -> AgentState {
    match value {
        Value::String(value) => match normalize_identifier(value).as_str() {
            "pending" | "pendinginit" => AgentState::PendingInit,
            "running" | "live" | "interrupted" => AgentState::Live,
            value if is_terminal_value(value) => AgentState::Terminal,
            _ => AgentState::Unknown,
        },
        Value::Object(values) if object_reports_terminal(values) => AgentState::Terminal,
        _ => AgentState::Unknown,
    }
}

pub(crate) fn is_terminal_value(value: &str) -> bool {
    terminal_outcome_from_identifier(&normalize_identifier(value)).is_some()
}

pub(crate) fn value_reports_terminal(value: &Value) -> bool {
    match value {
        Value::String(value) => is_terminal_value(value),
        Value::Object(values) => object_reports_terminal(values),
        _ => false,
    }
}

pub(crate) fn object_has_terminal_status(values: &Map<String, Value>) -> bool {
    values
        .iter()
        .any(|(key, value)| is_terminal_field(key) && value_reports_terminal(value))
}

pub(crate) fn object_reports_terminal(values: &Map<String, Value>) -> bool {
    object_terminal_outcome(values).is_some()
}

pub(crate) fn collect_terminal_agent_ids(value: &Value, target: &mut Vec<String>) {
    let decoded = decode_json_encoded_response(value);
    let value = decoded.as_ref().unwrap_or(value);
    collect_terminal_identifiers_with(
        value,
        target,
        |key| normalize_identifier(key) == "agentid",
        0,
        true,
    );
}

pub(crate) fn collect_terminal_observations(value: &Value, target: &mut Vec<TerminalObservation>) {
    let decoded = decode_json_encoded_response(value);
    let value = decoded.as_ref().unwrap_or(value);
    collect_terminal_observations_in_envelope(value, target, 0, true);
}

fn decode_json_encoded_response(value: &Value) -> Option<Value> {
    let encoded = value.as_str()?.trim();
    if encoded.is_empty() || encoded.len() > MAX_JSON_ENCODED_RESPONSE_BYTES {
        return None;
    }
    serde_json::from_str(encoded).ok()
}

fn collect_terminal_observations_in_envelope(
    value: &Value,
    target: &mut Vec<TerminalObservation>,
    depth: usize,
    entry_allowed: bool,
) {
    if depth > 8 {
        return;
    }
    match value {
        Value::Array(values) if entry_allowed => {
            for value in values {
                collect_terminal_observations_in_envelope(value, target, depth + 1, true);
            }
        }
        Value::Object(values) => {
            if entry_allowed && let Some(outcome) = object_terminal_outcome(values) {
                target.extend(values.iter().filter_map(|(key, value)| {
                    matches!(
                        normalize_identifier(key).as_str(),
                        "taskname" | "agentname" | "agentid" | "subagentid"
                    )
                    .then(|| value.as_str().map(str::trim))
                    .flatten()
                    .filter(|value| !value.is_empty())
                    .map(|identifier| TerminalObservation {
                        identifier: identifier.to_owned(),
                        outcome,
                    })
                }));
            }
            for (key, value) in values {
                if is_agent_collection_field(key) || is_provider_envelope_field(key) {
                    collect_terminal_observations_in_envelope(value, target, depth + 1, true);
                }
            }
        }
        _ => {}
    }
}

fn collect_terminal_identifiers_with(
    value: &Value,
    target: &mut Vec<String>,
    is_identifier: impl Fn(&str) -> bool + Copy,
    depth: usize,
    entry_allowed: bool,
) {
    if depth > 8 {
        return;
    }
    match value {
        Value::Array(values) if entry_allowed => {
            for value in values {
                collect_terminal_identifiers_with(value, target, is_identifier, depth + 1, true);
            }
        }
        Value::Object(values) => {
            if entry_allowed && object_has_terminal_status(values) {
                target.extend(values.iter().filter_map(|(key, value)| {
                    (is_identifier(key))
                        .then(|| value.as_str().map(str::trim))
                        .flatten()
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                }));
            }
            for (key, value) in values {
                if is_agent_collection_field(key) || is_provider_envelope_field(key) {
                    collect_terminal_identifiers_with(
                        value,
                        target,
                        is_identifier,
                        depth + 1,
                        true,
                    );
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn is_agent_collection_field(key: &str) -> bool {
    matches!(
        normalize_identifier(key).as_str(),
        "agents" | "subagents" | "children" | "updates"
    )
}

pub(crate) fn is_provider_envelope_field(key: &str) -> bool {
    matches!(
        normalize_identifier(key).as_str(),
        "result" | "structuredcontent" | "data"
    )
}

pub(crate) fn extract_agent_identifier(value: &Value) -> Option<&str> {
    match value {
        Value::Object(values) => {
            if let Some(identifier) = values.iter().find_map(|(key, value)| {
                matches!(
                    normalize_identifier(key).as_str(),
                    "agentid" | "agentname" | "subagentid"
                )
                .then(|| value.as_str().map(str::trim))
                .flatten()
                .filter(|value| !value.is_empty())
            }) {
                return Some(identifier);
            }
            // Only descend through provider envelopes. A task payload may contain
            // arbitrary `task_name`/`agent_id` fields and must not be mistaken for
            // the identity returned by the spawn provider.
            values.iter().find_map(|(key, value)| {
                is_provider_envelope_field(key)
                    .then(|| extract_agent_identifier(value))
                    .flatten()
            })
        }
        Value::Array(values) => values.iter().find_map(extract_agent_identifier),
        _ => None,
    }
}

pub(crate) fn response_is_explicit_spawn_failure(value: &Value) -> bool {
    // A concrete agent identifier is authoritative. Embedded task output may
    // legitimately contain an `error` field and must not roll back the spawn.
    if extract_agent_identifier(value).is_some() {
        return false;
    }
    response_has_structured_failure(value) || response_has_textual_spawn_failure(value)
}

fn response_has_structured_failure(value: &Value) -> bool {
    let Value::Object(values) = value else {
        return false;
    };
    values
        .iter()
        .any(|(key, value)| match normalize_identifier(key).as_str() {
            "iserror" => value.as_bool() == Some(true),
            "error" => value_reports_nonempty_error(value),
            _ => false,
        })
}

pub(crate) fn value_reports_nonempty_error(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::Bool(true) => true,
    }
}

fn response_has_textual_spawn_failure(value: &Value) -> bool {
    match value {
        Value::Object(values) => values.iter().any(|(key, value)| {
            matches!(
                normalize_identifier(key).as_str(),
                "content" | "message" | "output" | "result" | "text"
            ) && response_has_textual_spawn_failure(value)
        }),
        Value::Array(values) => values.iter().any(response_has_textual_spawn_failure),
        Value::String(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            [
                "collab spawn failed",
                "agent spawn failed",
                "spawn agent failed",
                "spawn_agent failed",
                "failed to spawn agent",
                "failed to spawn subagent",
            ]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
        }
        _ => false,
    }
}

fn object_terminal_outcome(values: &Map<String, Value>) -> Option<TerminalOutcome> {
    values
        .iter()
        .filter_map(|(key, value)| {
            let normalized_key = normalize_identifier(key);
            if is_terminal_marker_field(&normalized_key)
                && let Some(outcome) = terminal_outcome_from_identifier(&normalized_key)
                && !matches!(value, Value::Bool(false) | Value::Null)
            {
                return Some(outcome);
            }
            is_terminal_field(&normalized_key)
                .then(|| terminal_outcome_from_value(value))
                .flatten()
        })
        .fold(None, |current, outcome| {
            Some(match (current, outcome) {
                (Some(TerminalOutcome::Failed), _) | (_, TerminalOutcome::Failed) => {
                    TerminalOutcome::Failed
                }
                (Some(TerminalOutcome::Lost), _) | (_, TerminalOutcome::Lost) => {
                    TerminalOutcome::Lost
                }
                _ => TerminalOutcome::Succeeded,
            })
        })
}

fn is_terminal_marker_field(key: &str) -> bool {
    matches!(
        key,
        "finalanswer"
            | "taskcomplete"
            | "completed"
            | "errored"
            | "failed"
            | "shutdown"
            | "notfound"
    )
}

fn terminal_outcome_from_value(value: &Value) -> Option<TerminalOutcome> {
    match value {
        Value::String(value) => terminal_outcome_from_identifier(&normalize_identifier(value)),
        Value::Object(values) => object_terminal_outcome(values),
        _ => None,
    }
}

fn terminal_outcome_from_identifier(value: &str) -> Option<TerminalOutcome> {
    match value {
        "finalanswer" | "taskcomplete" | "completed" => Some(TerminalOutcome::Succeeded),
        "errored" | "error" | "failed" => Some(TerminalOutcome::Failed),
        "shutdown" | "notfound" => Some(TerminalOutcome::Lost),
        _ => None,
    }
}

fn is_terminal_field(key: &str) -> bool {
    matches!(
        normalize_identifier(key).as_str(),
        "status"
            | "state"
            | "agentstatus"
            | "type"
            | "kind"
            | "event"
            | "messagetype"
            | "messagekind"
            | "eventname"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn terminal_status_is_shared_across_envelope_shapes() {
        for value in [
            json!("FINAL_ANSWER"),
            json!({"status": "task_complete"}),
            json!({"event": {"failed": true}}),
        ] {
            assert_eq!(classify_agent_status(&value), AgentState::Terminal);
        }
        assert_eq!(
            classify_agent_status(&json!("interrupted")),
            AgentState::Live
        );
    }

    #[test]
    fn terminal_outcomes_do_not_conflate_failure_or_loss_with_success() {
        let value = json!({"updates": [
            {"agentId": "ok", "status": "completed"},
            {"agentId": "bad", "state": "errored"},
            {"agentId": "gone", "agentStatus": "not_found"}
        ]});
        let mut observations = Vec::new();
        collect_terminal_observations(&value, &mut observations);
        assert_eq!(
            observations,
            [
                TerminalObservation {
                    identifier: "ok".to_string(),
                    outcome: TerminalOutcome::Succeeded,
                },
                TerminalObservation {
                    identifier: "bad".to_string(),
                    outcome: TerminalOutcome::Failed,
                },
                TerminalObservation {
                    identifier: "gone".to_string(),
                    outcome: TerminalOutcome::Lost,
                },
            ]
        );
        assert_eq!(
            classify_agent_status(&json!("mystery")),
            AgentState::Unknown
        );
    }

    #[test]
    fn terminal_identifiers_are_collected_without_false_positive() {
        let value = json!({"updates": [
            {"agentId": "done", "agentStatus": "completed"},
            {"agentId": "live", "agentStatus": "running"}
        ]});
        let mut identifiers = Vec::new();
        collect_terminal_agent_ids(&value, &mut identifiers);
        assert_eq!(identifiers, ["done"]);
    }

    #[test]
    fn json_encoded_provider_responses_preserve_terminal_identity_and_outcome() {
        let encoded = Value::String(
            serde_json::to_string(&json!({
                "updates": [
                    {"agent_id": "done", "status": "completed"},
                    {"agent_id": "bad", "status": "failed"}
                ]
            }))
            .unwrap(),
        );
        let mut observations = Vec::new();
        collect_terminal_observations(&encoded, &mut observations);
        assert_eq!(
            observations,
            [
                TerminalObservation {
                    identifier: "done".to_string(),
                    outcome: TerminalOutcome::Succeeded,
                },
                TerminalObservation {
                    identifier: "bad".to_string(),
                    outcome: TerminalOutcome::Failed,
                }
            ]
        );

        let mut identifiers = Vec::new();
        collect_terminal_agent_ids(&encoded, &mut identifiers);
        assert_eq!(identifiers, ["done", "bad"]);
    }

    #[test]
    fn arbitrary_nested_business_payload_cannot_report_agent_terminal_state() {
        let value = json!({
            "updates": [{
                "agent_id": "live",
                "status": "running",
                "output": {
                    "agent_id": "live",
                    "status": "completed"
                },
                "details": {
                    "task_name": "unrelated",
                    "failed": true
                }
            }]
        });
        let mut observations = Vec::new();
        collect_terminal_observations(&value, &mut observations);
        assert!(observations.is_empty());

        let mut identifiers = Vec::new();
        collect_terminal_agent_ids(&value, &mut identifiers);
        assert!(identifiers.is_empty());

        let wrapped = json!({
            "result": {
                "structuredContent": {
                    "updates": [{"agent_id": "done", "status": "completed"}]
                }
            }
        });
        collect_terminal_observations(&wrapped, &mut observations);
        assert_eq!(observations[0].identifier, "done");
    }

    #[test]
    fn agent_identifier_overrides_nested_error_output() {
        assert!(!response_is_explicit_spawn_failure(
            &json!({"agent_id": "agent-1", "output": {"error": "task output"}})
        ));
        assert_eq!(
            extract_agent_identifier(&json!({"task_name": "/root/scan_auth"})),
            None
        );
        assert_eq!(
            extract_agent_identifier(&json!({"result": {"agent_id": "agent-1"}})),
            Some("agent-1")
        );
        assert!(response_is_explicit_spawn_failure(
            &json!({"isError": true, "message": "failed"})
        ));
        for empty_error in [json!(null), json!(false), json!(""), json!([]), json!({})] {
            assert!(!response_is_explicit_spawn_failure(
                &json!({"error": empty_error, "message": "accepted"})
            ));
        }
        assert!(response_is_explicit_spawn_failure(
            &json!({"error": {"code": "capacity"}})
        ));
    }

    #[test]
    fn payload_error_fields_are_not_terminal_markers() {
        assert!(!object_reports_terminal(
            json!({"agent_id": "live", "error": "task output"})
                .as_object()
                .unwrap()
        ));
        assert!(object_reports_terminal(
            json!({"agent_id": "bad", "status": "error"})
                .as_object()
                .unwrap()
        ));
        assert!(object_reports_terminal(
            json!({"agent_id": "bad", "failed": "capacity"})
                .as_object()
                .unwrap()
        ));
    }
}
