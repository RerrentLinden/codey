use std::ffi::OsStr;
use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) const ARGUMENT: &str = "--codey-subagent-control-mcp";
pub(crate) const SERVER_ID: &str = "codey_subagent_control";
pub(crate) const NAMESPACE: &str = "mcp__codey_subagent_control";
pub(crate) const TOOL_NAME: &str = "resolve_batch";
pub(crate) const QUALIFIED_TOOL_NAME: &str = "mcp__codey_subagent_control__resolve_batch";
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
        "tools/list" => json!({ "tools": [tool_definition()] }),
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

fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
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

fn handle_tool_call(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str);
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let result = if name != Some(TOOL_NAME) {
        tool_error("unknown tool")
    } else if let Err(reason) = validate_arguments(&arguments) {
        tool_error(&reason)
    } else {
        let mut structured = arguments;
        structured
            .as_object_mut()
            .expect("validated batch decision arguments are an object")
            .insert("accepted".to_string(), Value::Bool(true));
        json!({
            "content": [{
                "type": "text",
                "text": "Batch decision accepted for Codey Hook validation."
            }],
            "structuredContent": structured,
            "isError": false
        })
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn validate_arguments(arguments: &Value) -> std::result::Result<(), String> {
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
        assert_eq!(response["result"]["tools"][0]["name"], TOOL_NAME);
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
    fn notifications_do_not_write_a_response() {
        assert!(
            handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
                .is_none()
        );
    }
}
