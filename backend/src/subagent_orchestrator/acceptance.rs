//! Parses mechanical-acceptance commands and classifies trusted exit evidence.

use serde_json::Value;

use crate::subagent::protocol;

use super::identity::normalized_identifier;
use super::{AcceptanceEntry, AcceptanceStatus, Reservation};

pub(super) fn extract_command(value: Option<&Value>) -> Option<&str> {
    let value = value?;
    match value {
        Value::String(command) => Some(command),
        Value::Object(values) => {
            for key in ["command", "cmd"] {
                if let Some(command) = values.get(key).and_then(Value::as_str) {
                    return Some(command);
                }
            }
            None
        }
        _ => None,
    }
}

pub(super) fn parse_acceptance_marker(command: &str) -> Option<(&str, &str, &str)> {
    let mut lines = command.lines();
    let first = lines.next()?.trim();
    let marker = first.strip_prefix("# codey-accept:")?;
    let (task_id, check_id) = marker.split_once(':')?;
    if task_id.is_empty() || check_id.is_empty() || check_id.contains(':') {
        return None;
    }
    let body_offset = command.find('\n').map_or(command.len(), |index| index + 1);
    Some((task_id, check_id, &command[body_offset..]))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AcceptanceEvidence {
    Passed,
    CommandFailed,
    MissingExitStatus,
}

impl AcceptanceEvidence {
    pub(super) fn failure_reason(self) -> &'static str {
        match self {
            Self::Passed => "验收已通过",
            Self::CommandFailed => "验收命令返回失败状态",
            Self::MissingExitStatus => "上游工具响应缺少可识别的退出状态",
        }
    }
}

pub(super) fn classify_acceptance_evidence(value: Option<&Value>) -> AcceptanceEvidence {
    let Some(value) = value else {
        return AcceptanceEvidence::MissingExitStatus;
    };
    let mut exit_codes = Vec::new();
    let mut error = false;
    collect_exit_status(value, &mut exit_codes, &mut error, 0, true);
    if error || exit_codes.iter().any(|exit_code| *exit_code != 0) {
        AcceptanceEvidence::CommandFailed
    } else if exit_codes.is_empty() {
        AcceptanceEvidence::MissingExitStatus
    } else {
        AcceptanceEvidence::Passed
    }
}

pub(super) fn collect_exit_status(
    value: &Value,
    exit_codes: &mut Vec<i64>,
    error: &mut bool,
    depth: usize,
    allow_plain_text_status: bool,
) {
    if depth > 12 {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect_exit_status(value, exit_codes, error, depth + 1, false);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let key = normalized_identifier(key);
                if key == "exitcode" {
                    if let Some(code) = value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
                    {
                        exit_codes.push(code);
                    }
                } else if (key == "iserror" && value.as_bool() == Some(true))
                    || (key == "error" && protocol::value_reports_nonempty_error(value))
                {
                    *error = true;
                }
                collect_exit_status(value, exit_codes, error, depth + 1, false);
            }
        }
        Value::String(value) if allow_plain_text_status && value.len() <= 64 * 1024 => {
            if let Ok(parsed) = serde_json::from_str::<Value>(value) {
                collect_exit_status(&parsed, exit_codes, error, depth + 1, false);
            } else if let Some(exit_code) = parse_plain_text_exit_code(value) {
                exit_codes.push(exit_code);
            }
        }
        _ => {}
    }
}

pub(super) fn parse_plain_text_exit_code(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 1024 {
        return None;
    }
    let first_line = trimmed.lines().next()?.trim().to_ascii_lowercase();
    for prefix in [
        "exit_code",
        "exit code",
        "exitcode",
        "process exited with code",
        "command exited with code",
        "script exited with code",
        "command finished with exit code",
    ] {
        if let Some(remainder) = first_line.strip_prefix(prefix) {
            let token = remainder
                .trim_start_matches(|character: char| {
                    character.is_ascii_whitespace() || matches!(character, ':' | '=')
                })
                .split_whitespace()
                .next()?;
            let token = token.trim_end_matches(|character: char| !character.is_ascii_digit());
            return token.parse::<i64>().ok();
        }
    }
    None
}

pub(super) fn reservation_has_pending_acceptance(reservation: &Reservation) -> bool {
    !reservation.spawn_failed
        && reservation.side_effect_authorized
        && reservation.acceptance.iter().any(acceptance_blocks_turn)
}

pub(super) fn acceptance_blocks_turn(check: &AcceptanceEntry) -> bool {
    match check.status {
        AcceptanceStatus::Passed => false,
        AcceptanceStatus::Pending | AcceptanceStatus::Failed => true,
        AcceptanceStatus::Unverifiable => check.release_notice_delivered_at_ms.is_none(),
    }
}
