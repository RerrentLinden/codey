use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(crate) const HOOK_ARGUMENT: &str = "--codey-subagent-gate-hook";
pub(crate) const RUNTIME_ACTIVE_ENV: &str = "CODEY_SUBAGENT_GATE_ACTIVE";
pub(crate) const HOOK_TIMEOUT_SECONDS: u64 = 5;
pub(crate) const SESSION_END_HOOK_TIMEOUT_SECONDS: u64 = 3;
const MAX_HOOK_INPUT_BYTES: u64 = 1024 * 1024;
const STATE_DIRECTORY: &str = "codey-subagent-gate-v2";

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
    #[serde(default)]
    tool_response: Option<Value>,
}

pub fn run_hook_if_requested() -> Result<bool> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(HOOK_ARGUMENT)) {
        return Ok(false);
    }
    if !runtime_gate_is_active(std::env::var_os(RUNTIME_ACTIVE_ENV).as_deref()) {
        write_hook_output(&json!({}))?;
        return Ok(true);
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
    write_hook_output(&output)?;
    Ok(true)
}

fn runtime_gate_is_active(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

fn write_hook_output(output: &Value) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, output).context("序列化 Codex 子代理门禁 Hook 输出失败")?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub(crate) fn hook_commands() -> Result<HookCommands> {
    hook_commands_for(HOOK_ARGUMENT)
}

pub(crate) fn hook_commands_for(argument: &str) -> Result<HookCommands> {
    let executable = std::env::current_exe().context("定位 Codey 子代理门禁程序失败")?;
    Ok(HookCommands {
        command: format!("{} {argument}", quote_posix(&executable)),
        command_windows: format!(
            "{} {argument}",
            powershell_executable_invocation(&executable)
        ),
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
        "PostToolUse" => post_tool_use_output(input, state_root),
        "Stop" => stop_output(input, state_root),
        _ => Ok(json!({})),
    }
}

fn pre_tool_use_output(input: &HookInput, state_root: &Path) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some() {
        if input.tool_name.as_deref().is_some_and(is_spawn_agent_tool) {
            return Ok(subagent_spawn_denial());
        }
        return Ok(json!({}));
    }
    if input
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

fn post_tool_use_output(input: &HookInput, state_root: &Path) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some()
        || !input.tool_name.as_deref().is_some_and(is_wait_agent_tool)
    {
        return Ok(json!({}));
    }
    if wait_was_interrupted_by_user(input.tool_response.as_ref()) {
        remove_session_state(state_root, &input.session_id)?;
        return Ok(json!({}));
    }
    let active = active_agent_count(state_root, &input.session_id)?;
    if active == 0 {
        return Ok(json!({}));
    }
    Ok(post_wait_continuation(active, input.tool_response.as_ref()))
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
        "PostToolUse"
            if nonempty(input.agent_id.as_deref()).is_none()
                && input.tool_name.as_deref().is_some_and(is_wait_agent_tool) =>
        {
            json!({
                "decision": "block",
                "reason": reason,
            })
        }
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
                "Codey 子代理门禁：仍有 {active} 个子代理在运行。现在只可调用 agents.* 协作工具做必要的查看、转向或停止，随后调用 agents.wait_agent；请持续等待到每个子代理都返回 FINAL_ANSWER 或 task_complete。"
            ),
        }
    })
}

fn subagent_spawn_denial() -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": "Codey 子代理门禁：子代理不能继续派生子代理。请停止调用 Agent 或 agents.spawn_agent；如需进一步拆分，请把建议返回给主代理。",
        }
    })
}

fn post_wait_continuation(active: usize, tool_response: Option<&Value>) -> Value {
    let returned_update = render_wait_result(tool_response);
    json!({
        "decision": "block",
        "reason": format!(
            "Codey 子代理汇合门禁：本次 agents.wait_agent 返回了局部更新，仍有 {active} 个子代理在运行。保留下方内容；可读取它并仅使用 agents.send_message、agents.followup_task、agents.interrupt_agent 或 agents.list_agents 做必要协调，随后继续调用 agents.wait_agent。在每个已派生子代理都返回 FINAL_ANSWER 或 task_complete 前，不得恢复非协作本地工作、形成最终结论或结束当前任务。\n\n本次 wait_agent 已返回内容：\n{returned_update}"
        ),
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

fn render_wait_result(tool_response: Option<&Value>) -> String {
    match tool_response {
        Some(Value::String(response)) => response.clone(),
        Some(response) => serde_json::to_string(response)
            .unwrap_or_else(|_| "（wait_agent 返回内容无法序列化）".to_string()),
        None => "（wait_agent 未提供返回内容）".to_string(),
    }
}

fn wait_was_interrupted_by_user(tool_response: Option<&Value>) -> bool {
    tool_response.is_some_and(value_reports_user_interrupt)
}

fn value_reports_user_interrupt(value: &Value) -> bool {
    match value {
        Value::String(value) => text_reports_user_interrupt(value),
        Value::Array(values) => values.iter().any(value_reports_user_interrupt),
        Value::Object(values) => values.iter().any(|(key, value)| {
            let normalized_key = key
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>();
            (matches!(
                normalized_key.as_str(),
                "interruptedbynewinput"
                    | "interruptedbyuserinput"
                    | "interruptedbyuser"
                    | "cancelledbyuser"
                    | "canceledbyuser"
                    | "abortedbyuser"
                    | "stoppedbyuser"
                    | "usercancelled"
                    | "usercanceled"
                    | "useraborted"
                    | "userstopped"
                    | "newuserinput"
                    | "steeredinput"
                    | "steereduserinput"
            ) && !matches!(value, Value::Bool(false) | Value::Null))
                || value_reports_user_interrupt(value)
        }),
        _ => false,
    }
}

fn text_reports_user_interrupt(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    let interrupted = normalized.contains("interrupt")
        || normalized.contains("steer")
        || normalized.contains("cancel")
        || normalized.contains("abort")
        || normalized.contains("stop");
    let user_action = [
        "new input",
        "new_input",
        "user input",
        "user_input",
        "steered input",
        "steered_input",
        "user message",
        "user_message",
        "new message",
        "new_message",
        "by user",
        "manual",
        "user cancel",
        "user abort",
        "user stop",
    ]
    .iter()
    .any(|needle| normalized.contains(needle));
    interrupted && user_action
}

fn is_collaboration_tool(tool_name: &str) -> bool {
    matches!(
        normalized_collaboration_tool(tool_name).as_str(),
        "agent"
            | "spawn_agent"
            | "wait_agent"
            | "list_agents"
            | "interrupt_agent"
            | "send_message"
            | "followup_task"
    )
}

fn is_wait_agent_tool(tool_name: &str) -> bool {
    normalized_collaboration_tool(tool_name) == "wait_agent"
}

fn is_spawn_agent_tool(tool_name: &str) -> bool {
    matches!(
        normalized_collaboration_tool(tool_name).as_str(),
        "agent" | "spawn_agent"
    )
}

fn normalized_collaboration_tool(tool_name: &str) -> String {
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
    flattened_leaf.to_string()
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
    let raw_path = path.to_string_lossy();
    #[cfg(windows)]
    let path = windows_path_to_wsl(&raw_path).unwrap_or_else(|| raw_path.into_owned());
    #[cfg(not(windows))]
    let path = raw_path.into_owned();
    format!("'{}'", path.replace('\'', "'\"'\"'"))
}

#[cfg(any(windows, test))]
fn windows_path_to_wsl(path: &str) -> Option<String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let bytes = path.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return None;
    }
    Some(format!(
        "/mnt/{}/{}",
        (bytes[0] as char).to_ascii_lowercase(),
        path[3..].replace('\\', "/")
    ))
}

fn powershell_executable_invocation(path: &Path) -> String {
    format!("& '{}'", path.to_string_lossy().replace('\'', "''"))
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
            tool_response: None,
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
    fn child_cannot_spawn_nested_subagents_through_any_supported_alias() {
        let temp = tempfile::tempdir().unwrap();
        for tool in [
            "Agent",
            "agents.Agent",
            "spawn_agent",
            "agents.spawn_agent",
            "agents__spawn_agent",
            "agentsspawn_agent",
        ] {
            let mut child_spawn = input("PreToolUse", "session-a");
            child_spawn.agent_id = Some("agent-a".to_string());
            child_spawn.tool_name = Some(tool.to_string());

            let denied = handle_hook(&child_spawn, temp.path()).unwrap();
            assert_eq!(
                denied["hookSpecificOutput"]["permissionDecision"].as_str(),
                Some("deny"),
                "{tool}"
            );
            assert!(
                denied["hookSpecificOutput"]["permissionDecisionReason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("子代理不能继续派生子代理")),
                "{tool}"
            );
        }
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
    fn partial_wait_updates_keep_root_blocked_until_every_subagent_stops() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for agent_id in ["agent-a", "agent-b", "agent-c"] {
            let mut start = input("SubagentStart", "session-a");
            start.agent_id = Some(agent_id.to_string());
            handle_hook(&start, root).unwrap();
        }

        let mut stop_a = input("SubagentStop", "session-a");
        stop_a.agent_id = Some("agent-a".to_string());
        handle_hook(&stop_a, root).unwrap();

        let mut first_wait = input("PostToolUse", "session-a");
        first_wait.tool_name = Some("agents.wait_agent".to_string());
        first_wait.tool_response = Some(json!({
            "status": "FINAL_ANSWER",
            "agent_id": "agent-a",
            "message": "first result"
        }));
        let blocked_after_first = handle_hook(&first_wait, root).unwrap();
        assert_eq!(blocked_after_first["decision"].as_str(), Some("block"));
        let first_reason = blocked_after_first["reason"].as_str().unwrap();
        assert!(first_reason.contains("仍有 2 个子代理"));
        assert!(first_reason.contains("first result"));
        assert!(first_reason.contains("可读取它并仅使用 agents.send_message"));
        assert!(first_reason.contains("不得恢复非协作本地工作"));

        let mut root_steer = input("PreToolUse", "session-a");
        root_steer.tool_name = Some("agents.send_message".to_string());
        assert_eq!(handle_hook(&root_steer, root).unwrap(), json!({}));

        let mut root_patch = input("PreToolUse", "session-a");
        root_patch.tool_name = Some("apply_patch".to_string());
        assert_eq!(
            handle_hook(&root_patch, root).unwrap()["hookSpecificOutput"]["permissionDecision"]
                .as_str(),
            Some("deny")
        );
        assert_eq!(
            handle_hook(&input("Stop", "session-a"), root).unwrap()["decision"].as_str(),
            Some("block")
        );

        let mut stop_b = input("SubagentStop", "session-a");
        stop_b.agent_id = Some("agent-b".to_string());
        handle_hook(&stop_b, root).unwrap();
        let blocked_after_second = handle_hook(&first_wait, root).unwrap();
        assert!(
            blocked_after_second["reason"]
                .as_str()
                .unwrap()
                .contains("仍有 1 个子代理")
        );

        let mut stop_c = input("SubagentStop", "session-a");
        stop_c.agent_id = Some("agent-c".to_string());
        handle_hook(&stop_c, root).unwrap();
        assert_eq!(handle_hook(&first_wait, root).unwrap(), json!({}));
        assert_eq!(handle_hook(&root_patch, root).unwrap(), json!({}));
        assert_eq!(
            handle_hook(&input("Stop", "session-a"), root).unwrap(),
            json!({})
        );
    }

    #[test]
    fn interrupted_root_wait_clears_session_gate_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        let mut child_wait = input("PostToolUse", "child-session");
        child_wait.agent_id = Some("agent-a".to_string());
        child_wait.tool_name = Some("agentswait_agent".to_string());
        assert_eq!(handle_hook(&child_wait, root).unwrap(), json!({}));

        for (index, tool_response) in [
            json!({ "output": "Wait interrupted by new input" }),
            json!({ "output": "Wait cancelled by user" }),
            json!({ "message": "Wait manually stopped" }),
            json!({ "kind": "steered_input" }),
            json!({ "interrupted_by_user_input": true }),
            json!({ "canceled_by_user": true }),
        ]
        .into_iter()
        .enumerate()
        {
            let session_id = format!("interrupted-session-{index}");
            for agent_id in ["agent-a", "agent-b"] {
                let mut start = input("SubagentStart", &session_id);
                start.agent_id = Some(agent_id.to_string());
                handle_hook(&start, root).unwrap();
            }

            let mut interrupted_wait = input("PostToolUse", &session_id);
            interrupted_wait.tool_name = Some("agents__wait_agent".to_string());
            interrupted_wait.tool_response = Some(tool_response);
            assert_eq!(handle_hook(&interrupted_wait, root).unwrap(), json!({}));
            assert_eq!(active_agent_count(root, &session_id).unwrap(), 0);

            let mut root_patch = input("PreToolUse", &session_id);
            root_patch.tool_name = Some("apply_patch".to_string());
            assert_eq!(handle_hook(&root_patch, root).unwrap(), json!({}));
            assert_eq!(
                handle_hook(&input("Stop", &session_id), root).unwrap(),
                json!({})
            );
        }

        let mut start = input("SubagentStart", "active-session");
        start.agent_id = Some("agent-a".to_string());
        handle_hook(&start, root).unwrap();
        let mut completed_wait = input("PostToolUse", "active-session");
        completed_wait.tool_name = Some("agents__wait_agent".to_string());
        completed_wait.tool_response = Some(json!({
            "message": "Wait completed after an agent update"
        }));
        assert_eq!(
            handle_hook(&completed_wait, root).unwrap()["decision"].as_str(),
            Some("block")
        );
    }

    #[test]
    fn late_subagent_stop_after_interrupted_wait_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());
        handle_hook(&start, root).unwrap();

        let mut interrupted_wait = input("PostToolUse", "session-a");
        interrupted_wait.tool_name = Some("agents.wait_agent".to_string());
        interrupted_wait.tool_response = Some(json!({
            "output": "Wait interrupted by new user input"
        }));
        handle_hook(&interrupted_wait, root).unwrap();

        let mut late_stop = input("SubagentStop", "session-a");
        late_stop.agent_id = Some("agent-a".to_string());
        assert_eq!(handle_hook(&late_stop, root).unwrap(), json!({}));
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 0);
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
        assert!(is_spawn_agent_tool("Agent"));
        assert!(is_spawn_agent_tool("agents.spawn_agent"));
        assert!(is_spawn_agent_tool("agents__spawn_agent"));
        assert!(is_spawn_agent_tool("agentsspawn_agent"));
        assert!(!is_spawn_agent_tool("agents.wait_agent"));
    }

    #[test]
    fn gate_only_activates_for_a_codey_runtime() {
        assert!(!runtime_gate_is_active(None));
        assert!(!runtime_gate_is_active(Some(OsStr::new("0"))));
        assert!(!runtime_gate_is_active(Some(OsStr::new("true"))));
        assert!(runtime_gate_is_active(Some(OsStr::new("1"))));
    }

    #[test]
    fn windows_hook_executable_paths_translate_to_wsl_mounts() {
        assert_eq!(
            windows_path_to_wsl(r"C:\Program Files\Codey\codey.exe").as_deref(),
            Some("/mnt/c/Program Files/Codey/codey.exe")
        );
        assert_eq!(
            windows_path_to_wsl(r"\\?\D:\Apps\Codey.exe").as_deref(),
            Some("/mnt/d/Apps/Codey.exe")
        );
        assert_eq!(windows_path_to_wsl("/Applications/Codey"), None);
    }

    #[test]
    fn windows_hook_executable_paths_are_powershell_invocations() {
        assert_eq!(
            powershell_executable_invocation(Path::new(r"C:\Program Files\Codey\codey.exe")),
            r#"& 'C:\Program Files\Codey\codey.exe'"#
        );
        assert_eq!(
            powershell_executable_invocation(Path::new(
                r"C:\Users\O'Brien\$Codey` Preview\codey.exe"
            )),
            r#"& 'C:\Users\O''Brien\$Codey` Preview\codey.exe'"#
        );
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
