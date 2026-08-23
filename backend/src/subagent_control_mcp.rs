use std::ffi::OsStr;
use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) const ARGUMENT: &str = "--codey-subagent-control-mcp";
pub(crate) const SERVER_ID: &str = "codey_subagent_control";
pub(crate) const NAMESPACE: &str = "mcp__codey_subagent_control";
pub(crate) const RESOLVE_BATCH_TOOL_NAME: &str = "resolve_batch";
pub(crate) const PREPARE_DELEGATION_TOOL_NAME: &str = "prepare_delegation";
pub(crate) const QUALIFIED_TOOL_NAME: &str = "mcp__codey_subagent_control__resolve_batch";
pub(crate) const PREPARE_DELEGATION_QUALIFIED_TOOL_NAME: &str =
    "mcp__codey_subagent_control__prepare_delegation";
pub(crate) const STARTUP_TIMEOUT_SECONDS: i64 = 30;
pub(crate) const TOOL_TIMEOUT_SECONDS: i64 = 30;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

pub fn run_if_requested() -> Result<bool> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(ARGUMENT)) {
        return Ok(false);
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.context("读取 Codey 子代理批次控制 MCP 请求失败")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_message(&line) {
            serde_json::to_writer(&mut stdout, &response)
                .context("序列化 Codey 子代理批次控制 MCP 响应失败")?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(true)
}

fn handle_message(line: &str) -> Option<Value> {
    let request = match serde_json::from_str::<JsonRpcRequest>(line) {
        Ok(request) => request,
        Err(error) => {
            return Some(json_rpc_error(
                Value::Null,
                -32700,
                format!("invalid JSON-RPC request: {error}"),
            ));
        }
    };
    let id = request.id?;
    let result = match request.method.as_str() {
        "initialize" => json!({
            "protocolVersion": request
                .params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "Codey subagent batch control", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => {
            json!({ "tools": [resolve_batch_tool_definition(), prepare_delegation_tool_definition()] })
        }
        "tools/call" => return Some(handle_tool_call(id, &request.params)),
        _ => {
            return Some(json_rpc_error(
                id,
                -32601,
                format!("method not found: {}", request.method),
            ));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn resolve_batch_tool_definition() -> Value {
    json!({
        "name": RESOLVE_BATCH_TOOL_NAME,
        "description": "Record the root agent's explicit decision after the current subagent batch has settled. The Codey Hook validates and commits the decision; this tool never spawns agents itself.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "decision": {
                    "type": "string",
                    "enum": ["spawn_next_batch", "continue_root", "complete", "blocked"]
                },
                "batch_number": { "type": "integer", "minimum": 1, "maximum": 65535 },
                "decision_id": { "type": "string", "minLength": 1, "maxLength": 128 },
                "reason": { "type": "string", "minLength": 1, "maxLength": 512 }
            },
            "required": ["decision", "batch_number", "decision_id", "reason"],
            "additionalProperties": false
        }
    })
}

fn prepare_delegation_tool_definition() -> Value {
    json!({
        "name": PREPARE_DELEGATION_TOOL_NAME,
        "description": "Authorize one encrypted writable spawn_agent call from the same root turn by staging its plaintext Codey delegation policy. The contract must declare an explicit absolute root. The permit binds the normalized root/read/write scope, is single-use, and expires on turn, batch, runtime, or TTL boundaries; this tool never spawns agents itself.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_name": { "type": "string", "minLength": 1, "maxLength": 64 },
                "agent_type": {
                    "type": "string",
                    "enum": ["codey_worker", "codey_visual_worker"]
                },
                "preparation_id": { "type": "string", "minLength": 1, "maxLength": 128 },
                "contract": {
                    "type": "object",
                    "description": "The exact CODEY_DELEGATION_V2 JSON object whose id equals task_name and whose root is explicit and absolute.",
                    "properties": {
                        "root": { "type": "string", "minLength": 1 }
                    },
                    "required": ["root"]
                }
            },
            "required": ["task_name", "agent_type", "preparation_id", "contract"],
            "additionalProperties": false
        }
    })
}

fn handle_tool_call(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str);
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let result = match name {
        Some(RESOLVE_BATCH_TOOL_NAME) => validated_tool_result(
            arguments,
            validate_batch_decision_arguments,
            "Batch decision accepted for Codey Hook validation.",
        ),
        Some(PREPARE_DELEGATION_TOOL_NAME) => validated_tool_result(
            arguments,
            validate_prepare_delegation_arguments,
            "Delegation sidecar accepted for Codey Hook validation.",
        ),
        _ => tool_error("unknown tool"),
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn validated_tool_result(
    arguments: Value,
    validator: fn(&Value) -> std::result::Result<(), String>,
    message: &str,
) -> Value {
    if let Err(reason) = validator(&arguments) {
        return tool_error(&reason);
    }
    let mut structured = arguments;
    structured
        .as_object_mut()
        .expect("validated Codey subagent control arguments are an object")
        .insert("accepted".to_string(), Value::Bool(true));
    json!({
        "content": [{
            "type": "text",
            "text": message
        }],
        "structuredContent": structured,
        "isError": false
    })
}

fn validate_batch_decision_arguments(arguments: &Value) -> std::result::Result<(), String> {
    let Some(object) = arguments.as_object() else {
        return Err("arguments must be an object".to_string());
    };
    const REQUIRED: [&str; 4] = ["decision", "batch_number", "decision_id", "reason"];
    if object.len() != REQUIRED.len() || REQUIRED.iter().any(|key| !object.contains_key(*key)) {
        return Err(
            "arguments must contain only decision, batch_number, decision_id, and reason"
                .to_string(),
        );
    }
    if !matches!(
        object.get("decision").and_then(Value::as_str),
        Some("spawn_next_batch" | "continue_root" | "complete" | "blocked")
    ) {
        return Err("decision is invalid".to_string());
    }
    if !object
        .get("batch_number")
        .and_then(Value::as_u64)
        .is_some_and(|value| (1..=u16::MAX as u64).contains(&value))
    {
        return Err("batch_number is invalid".to_string());
    }
    let decision_id = object
        .get("decision_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if decision_id.is_empty()
        || decision_id.len() > 128
        || !decision_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err("decision_id is invalid".to_string());
    }
    let reason = object
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if reason.is_empty() || reason.chars().count() > 512 {
        return Err("reason is invalid".to_string());
    }
    Ok(())
}

fn validate_prepare_delegation_arguments(arguments: &Value) -> std::result::Result<(), String> {
    let Some(object) = arguments.as_object() else {
        return Err("arguments must be an object".to_string());
    };
    const REQUIRED: [&str; 4] = ["task_name", "agent_type", "preparation_id", "contract"];
    if object.len() != REQUIRED.len() || REQUIRED.iter().any(|key| !object.contains_key(*key)) {
        return Err(
            "arguments must contain only task_name, agent_type, preparation_id, and contract"
                .to_string(),
        );
    }
    let task_name = object
        .get("task_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_task_name(task_name) {
        return Err("task_name is invalid".to_string());
    }
    if !matches!(
        object.get("agent_type").and_then(Value::as_str),
        Some("codey_worker" | "codey_visual_worker")
    ) {
        return Err("agent_type is invalid".to_string());
    }
    let preparation_id = object
        .get("preparation_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if preparation_id.is_empty()
        || preparation_id.len() > 128
        || !preparation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err("preparation_id is invalid".to_string());
    }
    let Some(contract) = object.get("contract").and_then(Value::as_object) else {
        return Err("contract must be an object".to_string());
    };
    if contract.get("id").and_then(Value::as_str) != Some(task_name) {
        return Err("contract.id must equal task_name".to_string());
    }
    let root = contract
        .get("root")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !valid_absolute_root(root) {
        return Err("contract.root must be an explicit absolute path".to_string());
    }
    Ok(())
}

fn valid_task_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_absolute_root(value: &str) -> bool {
    let value = value.trim().replace('\\', "/");
    value.starts_with('/')
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && value.as_bytes()[2] == b'/')
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

fn json_rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_batch_decision_tool() {
        let response =
            handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
                .unwrap();
        assert_eq!(
            response["result"]["tools"][0]["name"],
            RESOLVE_BATCH_TOOL_NAME
        );
        assert_eq!(
            response["result"]["tools"][1]["name"],
            PREPARE_DELEGATION_TOOL_NAME
        );
        assert_eq!(
            response["result"]["tools"][0]["inputSchema"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn echoes_only_valid_decisions_as_accepted() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"resolve_batch","arguments":{"decision":"spawn_next_batch","batch_number":1,"decision_id":"batch-1-next","reason":"more independent work remains"}}}"#,
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(response["result"]["structuredContent"]["accepted"], true);

        let invalid = handle_message(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"resolve_batch","arguments":{"decision":"auto_spawn","batch_number":1,"decision_id":"x","reason":"x"}}}"#,
        )
        .unwrap();
        assert_eq!(invalid["result"]["isError"], true);
    }

    #[test]
    fn echoes_only_valid_delegation_preparations_as_accepted() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"prepare_delegation","arguments":{"task_name":"worker_a","agent_type":"codey_worker","preparation_id":"prep-1","contract":{"id":"worker_a","why":"implementation","visual":false,"root":"/repo","read":[],"write":["backend/src"],"capabilities":["files.read","workspace.write"],"checks":[{"id":"tests","cmd":"cargo test"}]}}}}"#,
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(response["result"]["structuredContent"]["accepted"], true);

        let invalid = handle_message(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"prepare_delegation","arguments":{"task_name":"worker_a","agent_type":"codey_worker","preparation_id":"prep-1","contract":{"id":"other"}}}}"#,
        )
        .unwrap();
        assert_eq!(invalid["result"]["isError"], true);

        let relative_root = handle_message(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"prepare_delegation","arguments":{"task_name":"worker_a","agent_type":"codey_worker","preparation_id":"prep-2","contract":{"id":"worker_a","root":"repo"}}}}"#,
        )
        .unwrap();
        assert_eq!(relative_root["result"]["isError"], true);
        assert_eq!(
            relative_root["result"]["content"][0]["text"],
            "contract.root must be an explicit absolute path"
        );
    }

    #[test]
    fn notifications_do_not_write_a_response() {
        assert!(
            handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
                .is_none()
        );
    }
}
