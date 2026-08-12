use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(crate) const HOOK_ARGUMENT: &str = "--codey-subagent-gate-hook";
pub(crate) const HOOK_TIMEOUT_SECONDS: u64 = 5;
pub(crate) const SESSION_END_HOOK_TIMEOUT_SECONDS: u64 = 3;
const MAX_HOOK_INPUT_BYTES: u64 = 1024 * 1024;
const STATE_DIRECTORY: &str = "codey-subagent-gate-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HookCommands {
    pub command: String,
    pub command_windows: String,
}

#[derive(Debug, Deserialize)]
struct HookInput {
    hook_event_name: String,
    session_id: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
}

pub fn run_hook_if_requested() -> Result<bool> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(HOOK_ARGUMENT)) {
        return Ok(false);
    }

    let mut raw = Vec::new();
    std::io::stdin()
        .take(MAX_HOOK_INPUT_BYTES + 1)
        .read_to_end(&mut raw)
        .context("读取 Codex 子代理门禁 Hook 输入失败")?;
    if raw.len() as u64 > MAX_HOOK_INPUT_BYTES {
        bail!("Codex 子代理门禁 Hook 输入超过 1 MiB 上限");
    }
    let input: HookInput =
        serde_json::from_slice(&raw).context("解析 Codex 子代理门禁 Hook 输入失败")?;
    let state_root = std::env::temp_dir().join(STATE_DIRECTORY);
    let output = handle_hook(&input, &state_root).unwrap_or_else(|error| {
        eprintln!("Codey 子代理门禁 Hook 失败：{error:#}");
        fail_closed_output(&input, &error)
    });
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &output).context("序列化 Codex 子代理门禁 Hook 输出失败")?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(true)
}

pub(crate) fn hook_commands() -> Result<HookCommands> {
    let executable = std::env::current_exe().context("定位 Codey 子代理门禁程序失败")?;
    Ok(HookCommands {
        command: format!("{} {HOOK_ARGUMENT}", quote_posix(&executable)),
        command_windows: format!("{} {HOOK_ARGUMENT}", quote_windows(&executable)),
    })
}

pub(crate) fn hook_trust_hash(
    event_name: &str,
    matcher: Option<&str>,
    command: &str,
    timeout_seconds: u64,
) -> String {
    let mut handler = Map::new();
    handler.insert("async".to_string(), Value::Bool(false));
    handler.insert("command".to_string(), Value::String(command.to_string()));
    handler.insert("timeout".to_string(), Value::Number(timeout_seconds.into()));
    handler.insert("type".to_string(), Value::String("command".to_string()));

    let mut identity = Map::new();
    identity.insert(
        "event_name".to_string(),
        Value::String(event_name.to_string()),
    );
    identity.insert(
        "hooks".to_string(),
        Value::Array(vec![Value::Object(handler)]),
    );
    if let Some(matcher) = matcher {
        identity.insert("matcher".to_string(), Value::String(matcher.to_string()));
    }
    let canonical = canonical_json(&Value::Object(identity));
    let serialized = serde_json::to_vec(&canonical).unwrap_or_default();
    let digest = Sha256::digest(serialized);
    format!("sha256:{digest:x}")
}

fn handle_hook(input: &HookInput, state_root: &Path) -> Result<Value> {
    match input.hook_event_name.as_str() {
        "SubagentStart" => {
            if let Some(agent_id) = nonempty(input.agent_id.as_deref()) {
                create_active_marker(state_root, &input.session_id, agent_id)?;
            }
            Ok(json!({}))
        }
        "SubagentStop" => {
            if let Some(agent_id) = nonempty(input.agent_id.as_deref()) {
                remove_active_marker(state_root, &input.session_id, agent_id)?;
            }
            Ok(json!({}))
        }
        "SessionEnd" => {
            remove_session_state(state_root, &input.session_id)?;
            Ok(json!({}))
        }
        "PreToolUse" => pre_tool_use_output(input, state_root),
        "Stop" => stop_output(input, state_root),
        _ => Ok(json!({})),
    }
}

fn pre_tool_use_output(input: &HookInput, state_root: &Path) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some()
        || input
            .tool_name
            .as_deref()
            .is_some_and(is_collaboration_tool)
    {
        return Ok(json!({}));
    }
    let active = active_agent_count(state_root, &input.session_id)?;
    if active == 0 {
        return Ok(json!({}));
    }
    Ok(pre_tool_denial(active))
}

fn stop_output(input: &HookInput, state_root: &Path) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some() {
        return Ok(json!({}));
    }
    let active = active_agent_count(state_root, &input.session_id)?;
    if active == 0 {
        return Ok(json!({}));
    }
    Ok(stop_continuation(active))
}

fn fail_closed_output(input: &HookInput, error: &anyhow::Error) -> Value {
    let reason = format!(
        "Codey 无法确认子代理运行状态，已暂停主代理继续操作：{error:#}。请调用 agents.wait_agent 或 agents.list_agents 核对状态。"
    );
    match input.hook_event_name.as_str() {
        "PreToolUse" if nonempty(input.agent_id.as_deref()).is_none() => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        "Stop" if nonempty(input.agent_id.as_deref()).is_none() => json!({
            "decision": "block",
            "reason": reason,
        }),
        _ => json!({}),
    }
}

fn pre_tool_denial(active: usize) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!(
                "Codey 子代理门禁：仍有 {active} 个子代理在运行。现在只可调用 agents.* 协作工具；请立即调用 agents.wait_agent，并在 MESSAGE 后继续等待，直到收到 FINAL_ANSWER 或 task_complete。"
            ),
        }
    })
}

fn stop_continuation(active: usize) -> Value {
    json!({
        "decision": "block",
        "reason": format!(
            "Codey 子代理门禁：仍有 {active} 个子代理在运行，当前任务不能结束。请调用 agents.wait_agent，并持续等待到所有子代理返回 FINAL_ANSWER 或 task_complete。"
        ),
    })
}

fn is_collaboration_tool(tool_name: &str) -> bool {
    let normalized = tool_name.trim().to_ascii_lowercase();
    let leaf = normalized
        .rsplit(['.', '/', ':'])
        .next()
        .unwrap_or(normalized.as_str())
        .rsplit("__")
        .next()
        .unwrap_or(normalized.as_str());
    let flattened_leaf = normalized
        .strip_prefix("agents")
        .map(|name| name.trim_start_matches(['.', '/', ':', '_']))
        .unwrap_or(leaf);
    matches!(
        flattened_leaf,
        "agent"
            | "spawn_agent"
            | "wait_agent"
            | "list_agents"
            | "interrupt_agent"
            | "send_message"
            | "followup_task"
    )
}

fn create_active_marker(state_root: &Path, session_id: &str, agent_id: &str) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    fs::create_dir_all(&session_dir).with_context(|| {
        format!(
            "创建 Codex 子代理门禁状态目录失败：{}",
            session_dir.display()
        )
    })?;
    let marker = agent_marker_path(&session_dir, agent_id);
    fs::write(&marker, b"active\n")
        .with_context(|| format!("写入 Codex 子代理门禁状态失败：{}", marker.display()))
}

fn remove_active_marker(state_root: &Path, session_id: &str, agent_id: &str) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    let marker = agent_marker_path(&session_dir, agent_id);
    match fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("移除 Codex 子代理门禁状态失败：{}", marker.display()));
        }
    }
    match fs::remove_dir(&session_dir) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "清理 Codex 子代理门禁状态目录失败：{}",
                    session_dir.display()
                )
            });
        }
    }
    Ok(())
}

fn remove_session_state(state_root: &Path, session_id: &str) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    match fs::remove_dir_all(&session_dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "清理 Codex 子代理门禁会话状态失败：{}",
                session_dir.display()
            )
        }),
    }
}

fn active_agent_count(state_root: &Path, session_id: &str) -> Result<usize> {
    let session_dir = session_state_dir(state_root, session_id);
    let entries = match fs::read_dir(&session_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("读取 Codex 子代理门禁状态失败：{}", session_dir.display())
            });
        }
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(OsStr::to_str) == Some("active")
        {
            count += 1;
        }
    }
    Ok(count)
}

fn session_state_dir(state_root: &Path, session_id: &str) -> PathBuf {
    state_root.join(hash_component(session_id))
}

fn agent_marker_path(session_dir: &Path, agent_id: &str) -> PathBuf {
    session_dir.join(format!("{}.active", hash_component(agent_id)))
}

fn hash_component(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn quote_posix(path: &Path) -> String {
    let path = path.to_string_lossy();
    format!("'{}'", path.replace('\'', "'\"'\"'"))
}

fn quote_windows(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('"', "\\\""))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&map[key]));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(event: &str, session: &str) -> HookInput {
        HookInput {
            hook_event_name: event.to_string(),
            session_id: session.to_string(),
            agent_id: None,
            tool_name: None,
        }
    }

    #[test]
    fn active_subagent_blocks_only_root_non_collaboration_tools() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());
        handle_hook(&start, root).unwrap();

        let mut root_bash = input("PreToolUse", "session-a");
        root_bash.tool_name = Some("Bash".to_string());
        let denied = handle_hook(&root_bash, root).unwrap();
        assert_eq!(
            denied["hookSpecificOutput"]["permissionDecision"].as_str(),
            Some("deny")
        );

        let mut child_bash = input("PreToolUse", "session-a");
        child_bash.agent_id = Some("agent-a".to_string());
        child_bash.tool_name = Some("Bash".to_string());
        assert_eq!(handle_hook(&child_bash, root).unwrap(), json!({}));

        let mut wait = input("PreToolUse", "session-a");
        wait.tool_name = Some("agents.wait_agent".to_string());
        assert_eq!(handle_hook(&wait, root).unwrap(), json!({}));
    }

    #[test]
    fn subagent_stop_releases_root_and_stop_hook_cannot_finish_early() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());
        handle_hook(&start, root).unwrap();

        let blocked = handle_hook(&input("Stop", "session-a"), root).unwrap();
        assert_eq!(blocked["decision"].as_str(), Some("block"));

        let mut stop = input("SubagentStop", "session-a");
        stop.agent_id = Some("agent-a".to_string());
        handle_hook(&stop, root).unwrap();
        assert_eq!(
            handle_hook(&input("Stop", "session-a"), root).unwrap(),
            json!({})
        );
    }

    #[test]
    fn gate_state_is_isolated_by_session() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());
        handle_hook(&start, root).unwrap();

        let mut other = input("PreToolUse", "session-b");
        other.tool_name = Some("apply_patch".to_string());
        assert_eq!(handle_hook(&other, root).unwrap(), json!({}));
    }

    #[test]
    fn collaboration_tool_aliases_are_allowed() {
        for tool in [
            "Agent",
            "spawn_agent",
            "agents__wait_agent",
            "agentswait_agent",
            "agents.list_agents",
            "agents/interrupt_agent",
            "agents::send_message",
            "followup_task",
        ] {
            assert!(is_collaboration_tool(tool), "{tool}");
        }
        assert!(!is_collaboration_tool("functions.exec"));
        assert!(!is_collaboration_tool("update_plan"));
    }

    #[test]
    fn trust_hash_is_canonical_and_definition_sensitive() {
        let command = "'/tmp/codey' --codey-subagent-gate-hook";
        let first = hook_trust_hash("pre_tool_use", Some("*"), command, 5);
        let same = hook_trust_hash("pre_tool_use", Some("*"), command, 5);
        let changed = hook_trust_hash("stop", None, "codey --gate", 5);

        assert_eq!(first, same);
        assert_eq!(
            first,
            "sha256:55551dee38305185b5687a38eac9f0301b5e77da84abe693bc6c905fcfd767a5"
        );
        assert!(first.starts_with("sha256:"));
        assert_eq!(first.len(), "sha256:".len() + 64);
        assert_ne!(first, changed);
    }
}
