use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(crate) const HOOK_ARGUMENT: &str = "--codey-subagent-gate-hook";
pub(crate) const COMBINED_HOOK_ARGUMENT: &str = "--codey-subagent-gate-hook-with-fastctx";
pub(crate) const RUNTIME_ACTIVE_ENV: &str = "CODEY_SUBAGENT_GATE_ACTIVE";
pub(crate) const RUNTIME_ID_ENV: &str = "CODEY_SUBAGENT_GATE_RUNTIME_ID";
pub(crate) const HOOK_TIMEOUT_SECONDS: u64 = 5;
pub(crate) const SESSION_END_HOOK_TIMEOUT_SECONDS: u64 = 3;
const MAX_HOOK_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_RENDERED_TOOL_RESULT_CHARS: usize = 8 * 1024;
const STATE_DIRECTORY: &str = "codey-subagent-gate-v3";
const ACTIVE_MARKER_SCHEMA_VERSION: u32 = 1;
const LEGACY_RUNTIME_ID: &str = "legacy-runtime";
const PENDING_INIT_GRACE_MILLIS: u64 = 10 * 60 * 1000;
const STOP_STALL_GRACE_MILLIS: u64 = 10 * 60 * 1000;
const PENDING_INIT_OBSERVED_FILE: &str = "pending-init-observed.state";
const STOP_BLOCKED_SINCE_FILE: &str = "stop-blocked-since.state";
const STATE_ERROR_SINCE_FILE: &str = "state-error-since.state";

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
    tool_input: Option<Value>,
    #[serde(default)]
    tool_response: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ActiveMarker {
    schema_version: u32,
    runtime_id_hash: String,
    started_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookMode {
    SubagentOnly,
    WithFastctx,
}

pub fn run_hook_if_requested() -> Result<bool> {
    let mode = match std::env::args_os().nth(1).as_deref() {
        Some(argument) if argument == OsStr::new(HOOK_ARGUMENT) => HookMode::SubagentOnly,
        Some(argument) if argument == OsStr::new(COMBINED_HOOK_ARGUMENT) => HookMode::WithFastctx,
        _ => return Ok(false),
    };
    let gate_active = runtime_gate_is_active(std::env::var_os(RUNTIME_ACTIVE_ENV).as_deref());
    if mode == HookMode::SubagentOnly && !gate_active {
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
    let state_root = crate::codex_config::codex_home().join(STATE_DIRECTORY);
    let runtime_id = current_runtime_id();
    let output = match mode {
        HookMode::SubagentOnly => handle_hook_for_runtime(&input, &state_root, &runtime_id),
        HookMode::WithFastctx => {
            combined_hook_output_for_runtime(&input, &state_root, &runtime_id, gate_active)
        }
    }
    .unwrap_or_else(|error| {
        eprintln!("Codey 子代理门禁 Hook 失败：{error:#}");
        fail_closed_output(&input, &error)
    });
    write_hook_output(&output)?;
    Ok(true)
}

fn runtime_gate_is_active(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

fn current_runtime_id() -> String {
    std::env::var(RUNTIME_ID_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| LEGACY_RUNTIME_ID.to_string())
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

#[cfg(test)]
fn handle_hook(input: &HookInput, state_root: &Path) -> Result<Value> {
    handle_hook_for_runtime(input, state_root, &current_runtime_id())
}

fn handle_hook_for_runtime(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
) -> Result<Value> {
    handle_hook_for_runtime_at(input, state_root, runtime_id, current_timestamp_millis())
}

fn combined_hook_output_for_runtime(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
    gate_active: bool,
) -> Result<Value> {
    let gate_output = if gate_active {
        handle_hook_for_runtime(input, state_root, runtime_id)?
    } else {
        json!({})
    };
    if !gate_output.as_object().is_some_and(Map::is_empty) {
        return Ok(gate_output);
    }
    Ok(crate::fastctx_route_gate::hook_output(
        &input.hook_event_name,
        input.tool_name.as_deref(),
        input.tool_input.as_ref(),
    ))
}

fn handle_hook_for_runtime_at(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
    now_ms: u64,
) -> Result<Value> {
    match input.hook_event_name.as_str() {
        "SubagentStart" => {
            if let Some(agent_id) = nonempty(input.agent_id.as_deref()) {
                create_active_marker(state_root, runtime_id, &input.session_id, agent_id)?;
                crate::subagent_orchestrator::subagent_started(
                    state_root,
                    runtime_id,
                    &input.session_id,
                    agent_id,
                    now_ms,
                )?;
            }
            Ok(json!({}))
        }
        "SubagentStop" => {
            if let Some(agent_id) = nonempty(input.agent_id.as_deref()) {
                crate::subagent_orchestrator::subagent_stopped(
                    state_root,
                    runtime_id,
                    &input.session_id,
                    agent_id,
                    now_ms,
                )?;
                remove_active_marker(state_root, runtime_id, &input.session_id, agent_id)?;
                if active_agent_count_for_runtime(state_root, runtime_id, &input.session_id)? == 0 {
                    remove_session_state(state_root, runtime_id, &input.session_id)?;
                }
            }
            Ok(json!({}))
        }
        "SessionEnd" => {
            crate::subagent_orchestrator::end_session(state_root, &input.session_id)?;
            remove_session_state(state_root, runtime_id, &input.session_id)?;
            Ok(json!({}))
        }
        "PreToolUse" => pre_tool_use_output(input, state_root, runtime_id, now_ms),
        "PostToolUse" => post_tool_use_output(input, state_root, runtime_id, now_ms),
        "Stop" => stop_output(input, state_root, runtime_id, now_ms),
        _ => Ok(json!({})),
    }
}

fn pre_tool_use_output(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
    now_ms: u64,
) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some() {
        if input.tool_name.as_deref().is_some_and(is_spawn_agent_tool) {
            return Ok(subagent_spawn_denial());
        }
        if let (Some(agent_id), Some(tool_name)) =
            (input.agent_id.as_deref(), input.tool_name.as_deref())
            && let Some(reason) = crate::subagent_orchestrator::authorize_child_tool(
                state_root,
                runtime_id,
                &input.session_id,
                agent_id,
                tool_name,
                input.tool_input.as_ref(),
                now_ms,
            )?
        {
            return Ok(pre_tool_reason_denial(reason));
        }
        return Ok(json!({}));
    }
    if input
        .tool_name
        .as_deref()
        .is_some_and(is_contract_spawn_tool)
    {
        let active = active_agent_count_for_runtime(state_root, runtime_id, &input.session_id)?;
        if let Some(reason) = crate::subagent_orchestrator::pre_spawn(
            state_root,
            runtime_id,
            &input.session_id,
            input.tool_input.as_ref(),
            active,
            now_ms,
        )? {
            return Ok(pre_tool_reason_denial(reason));
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
    let active = active_agent_count_for_runtime(state_root, runtime_id, &input.session_id)?;
    if active == 0 {
        if let Some(reason) = crate::subagent_orchestrator::pre_root_tool(
            state_root,
            runtime_id,
            &input.session_id,
            input.tool_input.as_ref(),
            now_ms,
        )? {
            return Ok(pre_tool_reason_denial(reason));
        }
        return Ok(json!({}));
    }
    Ok(pre_tool_denial(active))
}

fn post_tool_use_output(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
    now_ms: u64,
) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some() {
        return Ok(json!({}));
    }
    let Some(tool_name) = input.tool_name.as_deref() else {
        return Ok(json!({}));
    };
    if is_contract_spawn_tool(tool_name) {
        crate::subagent_orchestrator::post_spawn(
            state_root,
            runtime_id,
            &input.session_id,
            input.tool_input.as_ref(),
            input.tool_response.as_ref(),
            now_ms,
        )?;
        return Ok(json!({}));
    }
    crate::subagent_orchestrator::post_root_tool(
        state_root,
        runtime_id,
        &input.session_id,
        input.tool_input.as_ref(),
        input.tool_response.as_ref(),
        now_ms,
    )?;
    if !is_agent_status_tool(tool_name) {
        return Ok(json!({}));
    }

    let response_is_usable = if is_wait_agent_tool(tool_name) {
        wait_agent_response_is_usable(input.tool_response.as_ref())
    } else {
        summarize_list_agents_response(input.tool_response.as_ref())
            != AgentListSnapshotState::Unknown
    };
    if response_is_usable {
        remove_session_auxiliary_file(
            state_root,
            runtime_id,
            &input.session_id,
            STOP_BLOCKED_SINCE_FILE,
        )?;
    }
    if is_wait_agent_tool(tool_name) {
        if wait_was_interrupted_by_user(input.tool_response.as_ref()) {
            remove_session_state(state_root, runtime_id, &input.session_id)?;
            crate::subagent_orchestrator::observe_status_response(
                state_root,
                runtime_id,
                &input.session_id,
                input.tool_response.as_ref(),
                true,
                now_ms,
            )?;
            return Ok(json!({}));
        }
        remove_completed_agents_from_wait_response(
            state_root,
            runtime_id,
            &input.session_id,
            input.tool_response.as_ref(),
        )?;
    } else if reconcile_list_agents_response(input, state_root, runtime_id, now_ms)? {
        crate::subagent_orchestrator::observe_status_response(
            state_root,
            runtime_id,
            &input.session_id,
            input.tool_response.as_ref(),
            true,
            now_ms,
        )?;
        return Ok(json!({}));
    }

    let Some(active) = active_agent_count_or_recover_corrupt_state(
        state_root,
        runtime_id,
        &input.session_id,
        now_ms,
    )?
    else {
        return Ok(json!({}));
    };
    if active == 0 {
        remove_session_state(state_root, runtime_id, &input.session_id)?;
        crate::subagent_orchestrator::observe_status_response(
            state_root,
            runtime_id,
            &input.session_id,
            input.tool_response.as_ref(),
            true,
            now_ms,
        )?;
        return Ok(json!({}));
    }
    crate::subagent_orchestrator::observe_status_response(
        state_root,
        runtime_id,
        &input.session_id,
        input.tool_response.as_ref(),
        false,
        now_ms,
    )?;
    if is_wait_agent_tool(tool_name) {
        Ok(post_wait_continuation(active, input.tool_response.as_ref()))
    } else {
        Ok(post_list_continuation(active, input.tool_response.as_ref()))
    }
}

fn stop_output(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
    now_ms: u64,
) -> Result<Value> {
    if nonempty(input.agent_id.as_deref()).is_some() {
        return Ok(json!({}));
    }
    let Some(active) = active_agent_count_or_recover_corrupt_state(
        state_root,
        runtime_id,
        &input.session_id,
        now_ms,
    )?
    else {
        return Ok(json!({}));
    };
    if active == 0 {
        return finalize_root_turn(state_root, runtime_id, &input.session_id, now_ms);
    }
    if observation_elapsed_if_present(
        state_root,
        runtime_id,
        &input.session_id,
        PENDING_INIT_OBSERVED_FILE,
        now_ms,
        PENDING_INIT_GRACE_MILLIS,
    )? || observe_and_check_elapsed(
        state_root,
        runtime_id,
        &input.session_id,
        STOP_BLOCKED_SINCE_FILE,
        now_ms,
        STOP_STALL_GRACE_MILLIS,
    )? {
        remove_session_state(state_root, runtime_id, &input.session_id)?;
        return finalize_root_turn(state_root, runtime_id, &input.session_id, now_ms);
    }
    Ok(stop_continuation(active))
}

fn finalize_root_turn(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<Value> {
    if let Some(reason) = crate::subagent_orchestrator::pending_acceptance_reason(
        state_root, runtime_id, session_id, now_ms,
    )? {
        return Ok(json!({
            "decision": "block",
            "reason": reason,
        }));
    }
    crate::subagent_orchestrator::settle_turn(state_root, runtime_id, session_id, now_ms)?;
    Ok(json!({}))
}

fn active_agent_count_or_recover_corrupt_state(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<Option<usize>> {
    match active_agent_count_for_runtime(state_root, runtime_id, session_id) {
        Ok(active) => {
            remove_session_auxiliary_file(
                state_root,
                runtime_id,
                session_id,
                STATE_ERROR_SINCE_FILE,
            )?;
            Ok(Some(active))
        }
        Err(error) => {
            if observe_and_check_elapsed(
                state_root,
                runtime_id,
                session_id,
                STATE_ERROR_SINCE_FILE,
                now_ms,
                STOP_STALL_GRACE_MILLIS,
            )? {
                remove_session_state(state_root, runtime_id, session_id)?;
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn fail_closed_output(input: &HookInput, error: &anyhow::Error) -> Value {
    let reason = format!(
        "Codey 无法确认子代理运行状态，已暂停主代理继续操作：{error:#}。请调用 agents.wait_agent 或 agents.list_agents 核对状态。若状态存储持续损坏，Stop 路径会在持续 10 分钟后回收当前运行代次；期间不得绕过门禁。"
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
                && input.tool_name.as_deref().is_some_and(is_agent_status_tool) =>
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
                "Codey 子代理门禁：仍有 {active} 个子代理尚未确认进入终态。现在只可调用 agents.* 协作工具；请先调用 agents.list_agents 核对 running、pending_init、completed、errored、shutdown 等状态，再对仍在运行的代理调用 agents.wait_agent。"
            ),
        }
    })
}

fn pre_tool_reason_denial(reason: String) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
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
    let returned_update = render_tool_result(tool_response, "wait_agent");
    json!({
        "decision": "block",
        "reason": format!(
            "Codey 子代理汇合门禁：本次 agents.wait_agent 返回后仍有 {active} 个子代理活动标记尚未核销。保留下方内容；可读取它并仅使用 agents.send_message、agents.followup_task、agents.interrupt_agent 或 agents.list_agents 做必要协调，并请先调用不带筛选的 agents.list_agents 对账。completed、errored、shutdown、not_found、FINAL_ANSWER 和 task_complete 都视为终态；只对 running、pending_init 或 interrupted 的代理继续等待或协调。累计 10 分钟仍无终态时只中断一次对应代理，再继续等待终态；不得无限 wait 或自动重派。在确认所有子代理进入终态前，不得恢复非协作本地工作、形成最终结论或结束当前任务。\n\n本次 wait_agent 已返回内容：\n{returned_update}"
        ),
    })
}

fn post_list_continuation(active: usize, tool_response: Option<&Value>) -> Value {
    let returned_update = render_tool_result(tool_response, "list_agents");
    json!({
        "decision": "block",
        "reason": format!(
            "Codey 子代理汇合门禁：agents.list_agents 核对后仍有 {active} 个子代理尚未确认进入终态。只对 running、pending_init 或 interrupted 的代理继续等待、转向或停止；completed、errored、shutdown 和 not_found 不再阻塞。累计 10 分钟仍无终态时只中断一次对应代理，再等待其进入终态；不得无限 wait 或自动重派。若 pending_init 实际已僵死，门禁会在持续 10 分钟无法进展后释放遗留状态。\n\n本次 list_agents 已返回内容：\n{returned_update}"
        ),
    })
}

fn stop_continuation(active: usize) -> Value {
    json!({
        "decision": "block",
        "reason": format!(
            "Codey 子代理门禁：仍有 {active} 个子代理尚未确认进入终态，当前任务不能结束。请先调用不带筛选的 agents.list_agents 对账，再对 running、pending_init 或 interrupted 的代理调用 agents.wait_agent；累计 10 分钟仍无终态时只中断一次对应代理并继续等待，不得无限重试或自动重派。若协作工具已经不可用，门禁会在持续 10 分钟无法进展后释放遗留状态。"
        ),
    })
}

fn render_tool_result(tool_response: Option<&Value>, tool_name: &str) -> String {
    let rendered = match tool_response {
        Some(Value::String(response)) => response.clone(),
        Some(response) => serde_json::to_string(response)
            .unwrap_or_else(|_| format!("（{tool_name} 返回内容无法序列化）")),
        None => format!("（{tool_name} 未提供返回内容）"),
    };
    let Some((cut_at, _)) = rendered.char_indices().nth(MAX_RENDERED_TOOL_RESULT_CHARS) else {
        return rendered;
    };
    let mut bounded = rendered;
    bounded.truncate(cut_at);
    bounded.push_str(
        "\n…（协作工具返回内容已截断；请调用不带筛选的 agents.list_agents 获取紧凑状态）",
    );
    bounded
}

fn wait_was_interrupted_by_user(tool_response: Option<&Value>) -> bool {
    tool_response.is_some_and(value_reports_user_interrupt)
}

fn wait_agent_response_is_usable(tool_response: Option<&Value>) -> bool {
    let Some(tool_response) = tool_response else {
        return false;
    };
    match tool_response {
        Value::Object(values) => {
            object_value(values, "timedout").is_some_and(Value::is_boolean)
                && (object_value(values, "message").is_some_and(Value::is_string)
                    || object_value(values, "status").is_some_and(Value::is_object))
        }
        Value::String(value) => serde_json::from_str::<Value>(value)
            .ok()
            .as_ref()
            .is_some_and(|value| wait_agent_response_is_usable(Some(value))),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentListSnapshotState {
    AllChildrenTerminal,
    OnlyPendingInit,
    HasLiveChildren,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservedAgentState {
    PendingInit,
    Live,
    Terminal,
    Unknown,
}

fn reconcile_list_agents_response(
    input: &HookInput,
    state_root: &Path,
    runtime_id: &str,
    now_ms: u64,
) -> Result<bool> {
    if !list_agents_query_is_full(input.tool_input.as_ref()) {
        return Ok(false);
    }
    match summarize_list_agents_response(input.tool_response.as_ref()) {
        AgentListSnapshotState::AllChildrenTerminal => {
            remove_session_state(state_root, runtime_id, &input.session_id)?;
            Ok(true)
        }
        AgentListSnapshotState::OnlyPendingInit => {
            if observe_and_check_elapsed(
                state_root,
                runtime_id,
                &input.session_id,
                PENDING_INIT_OBSERVED_FILE,
                now_ms,
                PENDING_INIT_GRACE_MILLIS,
            )? {
                remove_session_state(state_root, runtime_id, &input.session_id)?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        AgentListSnapshotState::HasLiveChildren => {
            remove_session_auxiliary_file(
                state_root,
                runtime_id,
                &input.session_id,
                PENDING_INIT_OBSERVED_FILE,
            )?;
            Ok(false)
        }
        AgentListSnapshotState::Unknown => Ok(false),
    }
}

fn list_agents_query_is_full(tool_input: Option<&Value>) -> bool {
    match tool_input {
        None | Some(Value::Null) => true,
        Some(Value::Object(values)) => values.iter().all(|(key, value)| {
            normalized_ascii_identifier(key) != "pathprefix"
                || matches!(value, Value::Null)
                || value.as_str().is_some_and(|value| value.trim().is_empty())
        }),
        Some(Value::String(value)) => serde_json::from_str::<Value>(value)
            .ok()
            .as_ref()
            .is_some_and(|value| list_agents_query_is_full(Some(value))),
        Some(_) => false,
    }
}

fn summarize_list_agents_response(tool_response: Option<&Value>) -> AgentListSnapshotState {
    let Some(agents) = tool_response.and_then(find_agents_array) else {
        return AgentListSnapshotState::Unknown;
    };
    let mut pending_init = 0;
    let mut live = 0;
    let mut unknown = 0;
    for agent in agents {
        let Value::Object(agent) = agent else {
            unknown += 1;
            continue;
        };
        let agent_name = object_value(&agent, "agentname").and_then(Value::as_str);
        if agent_name.is_some_and(is_root_agent_name) {
            continue;
        }
        let Some(status) = object_value(&agent, "agentstatus") else {
            unknown += 1;
            continue;
        };
        match classify_agent_status(status) {
            ObservedAgentState::PendingInit => pending_init += 1,
            ObservedAgentState::Live => live += 1,
            ObservedAgentState::Terminal => {}
            ObservedAgentState::Unknown => unknown += 1,
        }
    }
    if unknown > 0 {
        AgentListSnapshotState::Unknown
    } else if pending_init == 0 && live == 0 {
        AgentListSnapshotState::AllChildrenTerminal
    } else if pending_init > 0 && live == 0 {
        AgentListSnapshotState::OnlyPendingInit
    } else {
        AgentListSnapshotState::HasLiveChildren
    }
}

fn find_agents_array(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::Object(values) => {
            if let Some(Value::Array(agents)) = object_value(values, "agents") {
                return Some(agents.clone());
            }
            values.values().find_map(find_agents_array)
        }
        Value::Array(values) => values.iter().find_map(find_agents_array),
        Value::String(value) => {
            let parsed = serde_json::from_str::<Value>(value).ok()?;
            find_agents_array(&parsed)
        }
        _ => None,
    }
}

fn object_value<'a>(values: &'a Map<String, Value>, normalized_key: &str) -> Option<&'a Value> {
    values.iter().find_map(|(key, value)| {
        (normalized_ascii_identifier(key) == normalized_key).then_some(value)
    })
}

fn is_root_agent_name(value: &str) -> bool {
    matches!(value.trim().trim_end_matches('/'), "root" | "/root")
}

fn classify_agent_status(value: &Value) -> ObservedAgentState {
    match value {
        Value::String(value) => match normalized_ascii_identifier(value).as_str() {
            "pending" | "pendinginit" => ObservedAgentState::PendingInit,
            "running" | "live" | "interrupted" => ObservedAgentState::Live,
            value if is_agent_completion_value(value) => ObservedAgentState::Terminal,
            _ => ObservedAgentState::Unknown,
        },
        Value::Object(values) if object_status_reports_agent_completion(values) => {
            ObservedAgentState::Terminal
        }
        _ => ObservedAgentState::Unknown,
    }
}

fn remove_completed_agents_from_wait_response(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_response: Option<&Value>,
) -> Result<()> {
    let Some(tool_response) = tool_response else {
        return Ok(());
    };
    let mut completed_agent_ids = Vec::new();
    collect_completed_agent_ids(tool_response, &mut completed_agent_ids);
    completed_agent_ids.sort();
    completed_agent_ids.dedup();
    for agent_id in completed_agent_ids {
        remove_active_marker(state_root, runtime_id, session_id, &agent_id)?;
    }
    Ok(())
}

fn collect_completed_agent_ids(value: &Value, completed_agent_ids: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_completed_agent_ids(value, completed_agent_ids);
            }
        }
        Value::Object(values) => {
            if let Some(agent_id) = object_agent_id(values)
                && object_reports_agent_completion(values)
            {
                completed_agent_ids.push(agent_id.to_string());
            }
            for value in values.values() {
                collect_completed_agent_ids(value, completed_agent_ids);
            }
        }
        _ => {}
    }
}

fn object_agent_id(values: &Map<String, Value>) -> Option<&str> {
    values.iter().find_map(|(key, value)| {
        (normalized_ascii_identifier(key) == "agentid")
            .then(|| nonempty(value.as_str()))
            .flatten()
    })
}

fn object_reports_agent_completion(values: &Map<String, Value>) -> bool {
    values
        .iter()
        .any(|(key, value)| is_agent_completion_field(key) && value_reports_agent_completion(value))
}

fn value_reports_agent_completion(value: &Value) -> bool {
    match value {
        Value::String(value) => is_agent_completion_value(value),
        Value::Object(values) => object_status_reports_agent_completion(values),
        _ => false,
    }
}

fn object_status_reports_agent_completion(values: &Map<String, Value>) -> bool {
    values.iter().any(|(key, value)| {
        let normalized_key = normalized_ascii_identifier(key);
        (is_agent_completion_value(&normalized_key) && value != &Value::Bool(false))
            || (is_agent_completion_field(&normalized_key) && value_reports_agent_completion(value))
    })
}

fn is_agent_completion_field(key: &str) -> bool {
    matches!(
        normalized_ascii_identifier(key).as_str(),
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

fn is_agent_completion_value(value: &str) -> bool {
    matches!(
        normalized_ascii_identifier(value).as_str(),
        "finalanswer"
            | "taskcomplete"
            | "completed"
            | "errored"
            | "error"
            | "failed"
            | "shutdown"
            | "notfound"
    )
}

fn value_reports_user_interrupt(value: &Value) -> bool {
    match value {
        Value::String(value) => is_wait_interrupt_text(value),
        Value::Object(values) => object_reports_user_interrupt(values),
        _ => false,
    }
}

fn object_reports_user_interrupt(values: &Map<String, Value>) -> bool {
    values.iter().any(|(key, value)| {
        let normalized_key = normalized_ascii_identifier(key);
        is_user_interrupt_flag(&normalized_key) && value == &Value::Bool(true)
    }) || values.iter().any(|(key, value)| {
        let normalized_key = normalized_ascii_identifier(key);
        is_user_interrupt_discriminator(&normalized_key)
            && value
                .as_str()
                .is_some_and(is_user_interrupt_discriminator_value)
    }) || values.iter().any(|(key, value)| {
        let normalized_key = normalized_ascii_identifier(key);
        is_user_interrupt_text_field(&normalized_key)
            && value.as_str().is_some_and(is_wait_interrupt_text)
    }) || values.iter().any(|(key, value)| {
        let normalized_key = normalized_ascii_identifier(key);
        is_wait_response_envelope(&normalized_key)
            && !value.is_string()
            && value_reports_user_interrupt(value)
    })
}

fn is_user_interrupt_flag(key: &str) -> bool {
    matches!(
        key,
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
    )
}

fn is_user_interrupt_discriminator(key: &str) -> bool {
    matches!(key, "kind" | "type" | "status" | "event" | "eventname")
}

fn is_user_interrupt_discriminator_value(value: &str) -> bool {
    matches!(
        normalized_ascii_identifier(value).as_str(),
        "interruptedbynewinput"
            | "interruptedbyuserinput"
            | "interruptedbyuser"
            | "cancelledbyuser"
            | "canceledbyuser"
            | "abortedbyuser"
            | "stoppedbyuser"
            | "newuserinput"
            | "steeredinput"
            | "steereduserinput"
    )
}

fn is_user_interrupt_text_field(key: &str) -> bool {
    matches!(key, "message" | "output" | "reason")
}

fn is_wait_response_envelope(key: &str) -> bool {
    matches!(
        key,
        "data" | "output" | "response" | "result" | "toolresponse"
    )
}

fn normalized_ascii_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_wait_interrupt_text(value: &str) -> bool {
    matches!(
        normalized_ascii_identifier(value).as_str(),
        "waitinterruptedbynewinput"
            | "waitinterruptedbynewuserinput"
            | "waitinterruptedbyuserinput"
            | "waitcancelledbyuser"
            | "waitcanceledbyuser"
            | "waitabortedbyuser"
            | "waitstoppedbyuser"
            | "waitmanuallystopped"
    )
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

fn is_list_agents_tool(tool_name: &str) -> bool {
    normalized_collaboration_tool(tool_name) == "list_agents"
}

fn is_agent_status_tool(tool_name: &str) -> bool {
    is_wait_agent_tool(tool_name) || is_list_agents_tool(tool_name)
}

fn is_spawn_agent_tool(tool_name: &str) -> bool {
    matches!(
        normalized_collaboration_tool(tool_name).as_str(),
        "agent" | "spawn_agent"
    )
}

fn is_contract_spawn_tool(tool_name: &str) -> bool {
    normalized_collaboration_tool(tool_name) == "spawn_agent"
}

fn normalized_collaboration_tool(tool_name: &str) -> String {
    let normalized = tool_name.trim().to_ascii_lowercase();
    if is_functions_wait_alias(&normalized) {
        return "wait_agent".to_string();
    }
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

fn is_functions_wait_alias(normalized_tool_name: &str) -> bool {
    let Some(remainder) = normalized_tool_name.strip_prefix("functions") else {
        return false;
    };
    ["__", ".", "/", ":", "_"]
        .iter()
        .any(|separator| remainder.strip_prefix(separator) == Some("wait"))
}

fn current_timestamp_millis() -> u64 {
    u64::try_from(crate::fs_util::timestamp_millis()).unwrap_or(u64::MAX)
}

fn observe_and_check_elapsed(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    file_name: &str,
    now_ms: u64,
    grace_ms: u64,
) -> Result<bool> {
    let session_dir = session_state_dir(state_root, session_id);
    let path = session_auxiliary_path(&session_dir, runtime_id, file_name);
    match read_observation_timestamp(&path)? {
        Some(observed_at_ms) => Ok(now_ms.saturating_sub(observed_at_ms) >= grace_ms),
        None => {
            write_observation_timestamp(&session_dir, &path, now_ms)?;
            Ok(false)
        }
    }
}

fn observation_elapsed_if_present(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    file_name: &str,
    now_ms: u64,
    grace_ms: u64,
) -> Result<bool> {
    let path = session_auxiliary_path(
        &session_state_dir(state_root, session_id),
        runtime_id,
        file_name,
    );
    Ok(read_observation_timestamp(&path)?
        .is_some_and(|observed_at_ms| now_ms.saturating_sub(observed_at_ms) >= grace_ms))
}

fn read_observation_timestamp(path: &Path) -> Result<Option<u64>> {
    match fs::read_to_string(path) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map(Some)
            .with_context(|| format!("解析 Codex 子代理门禁观察时间失败：{}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("读取 Codex 子代理门禁观察时间失败：{}", path.display())),
    }
}

fn write_observation_timestamp(session_dir: &Path, path: &Path, now_ms: u64) -> Result<()> {
    fs::create_dir_all(session_dir).with_context(|| {
        format!(
            "创建 Codex 子代理门禁状态目录失败：{}",
            session_dir.display()
        )
    })?;
    let temp = crate::fs_util::unique_temp_path(path);
    fs::write(&temp, format!("{now_ms}\n"))
        .with_context(|| format!("写入 Codex 子代理门禁临时观察状态失败：{}", temp.display()))?;
    crate::fs_util::persist_temp_file(&temp, path)
        .with_context(|| format!("替换 Codex 子代理门禁观察状态失败：{}", path.display()))
}

fn remove_session_auxiliary_file(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    file_name: &str,
) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    let path = session_auxiliary_path(&session_dir, runtime_id, file_name);
    match fs::remove_file(&path) {
        Ok(()) => remove_empty_session_dir(&session_dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("移除 Codex 子代理门禁辅助状态失败：{}", path.display())),
    }
}

fn create_active_marker(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    fs::create_dir_all(&session_dir).with_context(|| {
        format!(
            "创建 Codex 子代理门禁状态目录失败：{}",
            session_dir.display()
        )
    })?;
    let marker = agent_marker_path(&session_dir, runtime_id, agent_id);
    let runtime_id_hash = hash_component(runtime_id);
    let state = ActiveMarker {
        schema_version: ACTIVE_MARKER_SCHEMA_VERSION,
        runtime_id_hash,
        started_at_ms: current_timestamp_millis(),
    };
    let bytes = serde_json::to_vec(&state).context("序列化 Codex 子代理门禁状态失败")?;
    let temp = crate::fs_util::unique_temp_path(&marker);
    fs::write(&temp, bytes)
        .with_context(|| format!("写入 Codex 子代理门禁临时状态失败：{}", temp.display()))?;
    crate::fs_util::persist_temp_file(&temp, &marker)
        .with_context(|| format!("替换 Codex 子代理门禁状态失败：{}", marker.display()))
}

fn remove_active_marker(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    let marker = agent_marker_path(&session_dir, runtime_id, agent_id);
    match fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("移除 Codex 子代理门禁状态失败：{}", marker.display()));
        }
    }
    remove_empty_session_dir(&session_dir)
}

fn remove_empty_session_dir(session_dir: &Path) -> Result<()> {
    match fs::remove_dir(session_dir) {
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

fn remove_session_state(state_root: &Path, runtime_id: &str, session_id: &str) -> Result<()> {
    let session_dir = session_state_dir(state_root, session_id);
    let entries = match fs::read_dir(&session_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "读取 Codex 子代理门禁会话状态失败：{}",
                    session_dir.display()
                )
            });
        }
    };
    let prefix = runtime_marker_prefix(runtime_id);
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() && runtime_file_has_prefix(&entry.path(), &prefix) {
            fs::remove_file(entry.path()).with_context(|| {
                format!(
                    "清理 Codex 子代理门禁会话状态失败：{}",
                    entry.path().display()
                )
            })?;
        }
    }
    remove_empty_session_dir(&session_dir)
}

#[cfg(test)]
fn active_agent_count(state_root: &Path, session_id: &str) -> Result<usize> {
    active_agent_count_for_runtime(state_root, &current_runtime_id(), session_id)
}

fn active_agent_count_for_runtime(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
) -> Result<usize> {
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
    let expected_runtime_id_hash = hash_component(runtime_id);
    let prefix = runtime_marker_prefix(runtime_id);
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() || !marker_name_has_prefix(&entry.path(), &prefix) {
            continue;
        }
        let path = entry.path();
        let bytes = fs::read(&path)
            .with_context(|| format!("读取 Codex 子代理门禁状态失败：{}", path.display()))?;
        let marker = serde_json::from_slice::<ActiveMarker>(&bytes)
            .with_context(|| format!("解析 Codex 子代理门禁状态失败：{}", path.display()))?;
        anyhow::ensure!(
            marker.schema_version == ACTIVE_MARKER_SCHEMA_VERSION,
            "Codex 子代理门禁状态版本不受支持：{}",
            path.display()
        );
        anyhow::ensure!(
            marker.runtime_id_hash == expected_runtime_id_hash,
            "Codex 子代理门禁状态代次不一致：{}",
            path.display()
        );
        count += 1;
    }
    Ok(count)
}

fn session_state_dir(state_root: &Path, session_id: &str) -> PathBuf {
    state_root.join(hash_component(session_id))
}

fn agent_marker_path(session_dir: &Path, runtime_id: &str, agent_id: &str) -> PathBuf {
    session_dir.join(format!(
        "{}{}.active",
        runtime_marker_prefix(runtime_id),
        hash_component(agent_id)
    ))
}

fn session_auxiliary_path(session_dir: &Path, runtime_id: &str, file_name: &str) -> PathBuf {
    session_dir.join(format!("{}{file_name}", runtime_marker_prefix(runtime_id)))
}

fn runtime_marker_prefix(runtime_id: &str) -> String {
    format!("{}-", hash_component(runtime_id))
}

fn marker_name_has_prefix(path: &Path, prefix: &str) -> bool {
    path.extension().and_then(OsStr::to_str) == Some("active")
        && runtime_file_has_prefix(path, prefix)
}

fn runtime_file_has_prefix(path: &Path, prefix: &str) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with(prefix))
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
            tool_input: None,
            tool_response: None,
        }
    }

    fn delegation_message(contract: Value) -> String {
        format!(
            "Do the bounded task.\n{}{}",
            crate::subagent_orchestrator::CONTRACT_PREFIX,
            serde_json::to_string(&contract).unwrap()
        )
    }

    #[test]
    fn runtime_gate_enforces_contract_ownership_and_mechanical_acceptance() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut spawn = input("PreToolUse", "contract-session");
        spawn.tool_name = Some("agents.spawn_agent".to_string());
        spawn.tool_input = Some(json!({
            "task_name": "worker_a",
            "agent_type": "codey_worker",
            "fork_turns": "none",
            "message": delegation_message(json!({
                "id": "worker_a",
                "why": "independent_work",
                "calls": 6,
                "visual": false,
                "root": "/repo",
                "read": [],
                "write": ["backend/src"],
                "checks": [{ "id": "tests", "cmd": "cargo test -p codey --lib" }]
            }))
        }));
        assert_eq!(handle_hook(&spawn, root).unwrap(), json!({}));

        let mut spawned = input("PostToolUse", "contract-session");
        spawned.tool_name = spawn.tool_name.clone();
        spawned.tool_input = spawn.tool_input.clone();
        spawned.tool_response = Some(json!({ "agent_id": "agent-a" }));
        assert_eq!(handle_hook(&spawned, root).unwrap(), json!({}));

        let mut started = input("SubagentStart", "contract-session");
        started.agent_id = Some("agent-a".to_string());
        handle_hook(&started, root).unwrap();

        let mut owned_patch = input("PreToolUse", "contract-session");
        owned_patch.agent_id = Some("agent-a".to_string());
        owned_patch.tool_name = Some("apply_patch".to_string());
        owned_patch.tool_input = Some(json!({
            "patch": "*** Begin Patch\n*** Update File: backend/src/lib.rs\n*** End Patch"
        }));
        assert_eq!(handle_hook(&owned_patch, root).unwrap(), json!({}));

        let mut escaped_patch = owned_patch;
        escaped_patch.tool_input = Some(json!({
            "patch": "*** Begin Patch\n*** Update File: README.md\n*** End Patch"
        }));
        assert_eq!(
            handle_hook(&escaped_patch, root).unwrap()["hookSpecificOutput"]["permissionDecision"]
                .as_str(),
            Some("deny")
        );

        let mut stopped = input("SubagentStop", "contract-session");
        stopped.agent_id = Some("agent-a".to_string());
        handle_hook(&stopped, root).unwrap();
        let blocked = handle_hook(&input("Stop", "contract-session"), root).unwrap();
        assert_eq!(blocked["decision"].as_str(), Some("block"));
        assert!(
            blocked["reason"]
                .as_str()
                .unwrap()
                .contains("codey-accept:worker_a:tests")
        );

        let acceptance_command = "# codey-accept:worker_a:tests\ncargo test -p codey --lib";
        let mut pre_acceptance = input("PreToolUse", "contract-session");
        pre_acceptance.tool_name = Some("Bash".to_string());
        pre_acceptance.tool_input = Some(json!({ "command": acceptance_command }));
        assert_eq!(handle_hook(&pre_acceptance, root).unwrap(), json!({}));

        let mut post_acceptance = input("PostToolUse", "contract-session");
        post_acceptance.tool_name = pre_acceptance.tool_name;
        post_acceptance.tool_input = pre_acceptance.tool_input;
        post_acceptance.tool_response = Some(json!({ "exit_code": 0, "output": "ok" }));
        assert_eq!(handle_hook(&post_acceptance, root).unwrap(), json!({}));
        assert_eq!(
            handle_hook(&input("Stop", "contract-session"), root).unwrap(),
            json!({})
        );
    }

    #[test]
    fn runtime_gate_rejects_small_delegations_before_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let mut spawn = input("PreToolUse", "small-session");
        spawn.tool_name = Some("agents.spawn_agent".to_string());
        spawn.tool_input = Some(json!({
            "task_name": "tiny_scan",
            "agent_type": "codey_deep_research",
            "fork_turns": "none",
            "message": delegation_message(json!({
                "id": "tiny_scan",
                "why": "breadth",
                "calls": 1,
                "files": 1,
                "dirs": 1,
                "visual": false,
                "read": [],
                "write": [],
                "checks": []
            }))
        }));
        let denied = handle_hook(&spawn, temp.path()).unwrap();
        assert_eq!(
            denied["hookSpecificOutput"]["permissionDecision"].as_str(),
            Some("deny")
        );
        assert!(
            denied["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("规模阈值")
        );
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

        for tool in [
            "agents.wait_agent",
            "functions.wait",
            "functions/wait",
            "functions:wait",
            "functions__wait",
            "functions_wait",
        ] {
            let mut wait = input("PreToolUse", "session-a");
            wait.tool_name = Some(tool.to_string());
            assert_eq!(handle_hook(&wait, root).unwrap(), json!({}), "{tool}");
        }

        let mut functions_exec = input("PreToolUse", "session-a");
        functions_exec.tool_name = Some("functions.exec".to_string());
        assert_eq!(
            handle_hook(&functions_exec, root).unwrap()["hookSpecificOutput"]["permissionDecision"]
                .as_str(),
            Some("deny")
        );
    }

    #[test]
    fn combined_hook_keeps_fastctx_active_and_prioritizes_the_subagent_gate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime_id = "runtime-a";
        let mut root_bash = input("PreToolUse", "session-a");
        root_bash.tool_name = Some("Bash".to_string());
        root_bash.tool_input = Some(json!({ "command": "rg -n needle src" }));

        let routed = combined_hook_output_for_runtime(&root_bash, root, runtime_id, false).unwrap();
        assert!(
            routed["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Codey FastCtx"))
        );

        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());
        handle_hook_for_runtime(&start, root, runtime_id).unwrap();
        let gated = combined_hook_output_for_runtime(&root_bash, root, runtime_id, true).unwrap();
        assert!(
            gated["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .is_some_and(|reason| reason.contains("子代理门禁"))
        );

        let mut child_bash = root_bash;
        child_bash.agent_id = Some("agent-a".to_string());
        let child_routed =
            combined_hook_output_for_runtime(&child_bash, root, runtime_id, true).unwrap();
        assert!(
            child_routed["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Codey FastCtx"))
        );
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
    fn completed_wait_response_releases_matching_active_markers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for agent_id in ["agent-a", "agent-b"] {
            let mut start = input("SubagentStart", "session-a");
            start.agent_id = Some(agent_id.to_string());
            handle_hook(&start, root).unwrap();
        }

        let mut completed_wait = input("PostToolUse", "session-a");
        completed_wait.tool_name = Some("functions.wait".to_string());
        completed_wait.tool_response = Some(json!({
            "updates": [
                {
                    "agentId": "agent-a",
                    "status": "FINAL_ANSWER",
                    "message": "done"
                },
                {
                    "nested": {
                        "agent_id": "agent-b",
                        "kind": "task-complete"
                    }
                }
            ]
        }));

        assert_eq!(handle_hook(&completed_wait, root).unwrap(), json!({}));
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 0);

        for agent_id in ["agent-a", "agent-b"] {
            let mut late_stop = input("SubagentStop", "session-a");
            late_stop.agent_id = Some(agent_id.to_string());
            assert_eq!(handle_hook(&late_stop, root).unwrap(), json!({}));
        }
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 0);
    }

    #[test]
    fn errored_and_other_terminal_wait_statuses_release_markers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for agent_id in ["agent-a", "agent-b", "agent-c", "agent-d"] {
            let mut start = input("SubagentStart", "session-a");
            start.agent_id = Some(agent_id.to_string());
            handle_hook(&start, root).unwrap();
        }

        let mut terminal_wait = input("PostToolUse", "session-a");
        terminal_wait.tool_name = Some("agents.wait_agent".to_string());
        terminal_wait.tool_response = Some(json!({
            "updates": [
                { "agent_id": "agent-a", "status": "completed" },
                { "agent_id": "agent-b", "state": "errored" },
                { "agent_id": "agent-c", "agent_status": { "errored": "429 Too Many Requests" } },
                { "agent_id": "agent-d", "status": "shutdown" }
            ]
        }));

        assert_eq!(handle_hook(&terminal_wait, root).unwrap(), json!({}));
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 0);
    }

    #[test]
    fn full_agent_list_snapshot_reconciles_terminal_children() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for agent_id in ["agent-a", "agent-b"] {
            let mut start = input("SubagentStart", "session-a");
            start.agent_id = Some(agent_id.to_string());
            handle_hook(&start, root).unwrap();
        }

        let mut list = input("PostToolUse", "session-a");
        list.tool_name = Some("agents.list_agents".to_string());
        list.tool_input = Some(json!({}));
        list.tool_response = Some(json!({
            "agents": [
                { "agent_name": "/root", "agent_status": "running" },
                { "agent_name": "/root/agent-a", "agent_status": { "completed": "done" } },
                { "agent_name": "/root/agent-b", "agent_status": { "errored": "503 Service Unavailable" } }
            ]
        }));

        assert_eq!(handle_hook(&list, root).unwrap(), json!({}));
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 0);
    }

    #[test]
    fn filtered_or_mixed_agent_lists_do_not_clear_live_markers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for agent_id in ["agent-a", "agent-b"] {
            let mut start = input("SubagentStart", "session-a");
            start.agent_id = Some(agent_id.to_string());
            handle_hook(&start, root).unwrap();
        }

        let mut filtered = input("PostToolUse", "session-a");
        filtered.tool_name = Some("agents.list_agents".to_string());
        filtered.tool_input = Some(json!({ "path_prefix": "/root/agent-a" }));
        filtered.tool_response = Some(json!({
            "agents": [{
                "agent_name": "/root/agent-a",
                "agent_status": { "errored": "429 Too Many Requests" }
            }]
        }));
        assert_eq!(
            handle_hook(&filtered, root).unwrap()["decision"].as_str(),
            Some("block")
        );
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 2);

        let mut mixed = input("PostToolUse", "session-a");
        mixed.tool_name = Some("agents.list_agents".to_string());
        mixed.tool_input = Some(json!({}));
        mixed.tool_response = Some(json!({
            "agents": [
                { "agent_name": "/root", "agent_status": "running" },
                { "agent_name": "/root/agent-a", "agent_status": { "errored": "429 Too Many Requests" } },
                { "agent_name": "/root/agent-b", "agent_status": "running" }
            ]
        }));
        assert_eq!(
            handle_hook(&mixed, root).unwrap()["decision"].as_str(),
            Some("block")
        );
        assert_eq!(active_agent_count(root, "session-a").unwrap(), 2);
    }

    #[test]
    fn stale_pending_init_and_unusable_collaboration_paths_release_after_grace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime_id = "runtime-a";

        let mut start = input("SubagentStart", "pending-session");
        start.agent_id = Some("agent-a".to_string());
        handle_hook_for_runtime_at(&start, root, runtime_id, 1_000).unwrap();
        let mut list = input("PostToolUse", "pending-session");
        list.tool_name = Some("agents.list_agents".to_string());
        list.tool_input = Some(json!({}));
        list.tool_response = Some(json!({
            "agents": [
                { "agent_name": "/root", "agent_status": "running" },
                { "agent_name": "/root/agent-a", "agent_status": "pending_init" }
            ]
        }));
        assert_eq!(
            handle_hook_for_runtime_at(&list, root, runtime_id, 1_000).unwrap()["decision"]
                .as_str(),
            Some("block")
        );
        assert_eq!(
            handle_hook_for_runtime_at(
                &input("Stop", "pending-session"),
                root,
                runtime_id,
                1_000 + PENDING_INIT_GRACE_MILLIS - 1,
            )
            .unwrap()["decision"]
                .as_str(),
            Some("block")
        );
        assert_eq!(
            handle_hook_for_runtime_at(
                &input("Stop", "pending-session"),
                root,
                runtime_id,
                1_000 + PENDING_INIT_GRACE_MILLIS,
            )
            .unwrap(),
            json!({})
        );

        let mut stalled_start = input("SubagentStart", "stalled-session");
        stalled_start.agent_id = Some("agent-b".to_string());
        handle_hook_for_runtime_at(&stalled_start, root, runtime_id, 2_000).unwrap();
        assert_eq!(
            handle_hook_for_runtime_at(&input("Stop", "stalled-session"), root, runtime_id, 2_000,)
                .unwrap()["decision"]
                .as_str(),
            Some("block")
        );
        let mut unavailable_wait = input("PostToolUse", "stalled-session");
        unavailable_wait.tool_name = Some("agents.wait_agent".to_string());
        unavailable_wait.tool_response = Some(Value::String(
            "该工具未在当前线程注册，无法执行 agents.wait_agent".to_string(),
        ));
        assert_eq!(
            handle_hook_for_runtime_at(
                &unavailable_wait,
                root,
                runtime_id,
                2_000 + STOP_STALL_GRACE_MILLIS - 1,
            )
            .unwrap()["decision"]
                .as_str(),
            Some("block")
        );
        assert_eq!(
            handle_hook_for_runtime_at(
                &input("Stop", "stalled-session"),
                root,
                runtime_id,
                2_000 + STOP_STALL_GRACE_MILLIS,
            )
            .unwrap(),
            json!({})
        );
    }

    #[test]
    fn collaboration_tool_output_is_bounded_on_unicode_boundaries() {
        let payload = "界".repeat(MAX_RENDERED_TOOL_RESULT_CHARS + 32);
        let rendered = render_tool_result(Some(&Value::String(payload)), "wait_agent");
        let (body, suffix) = rendered.split_once('\n').unwrap();

        assert_eq!(body.chars().count(), MAX_RENDERED_TOOL_RESULT_CHARS);
        assert!(body.chars().all(|character| character == '界'));
        assert!(suffix.contains("协作工具返回内容已截断"));
        assert!(suffix.contains("agents.list_agents"));
    }

    #[test]
    fn corrupted_active_state_fails_closed_then_recovers_after_grace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime_id = "runtime-a";
        let session_id = "corrupt-session";
        let mut start = input("SubagentStart", session_id);
        start.agent_id = Some("agent-a".to_string());
        handle_hook_for_runtime_at(&start, root, runtime_id, 1_000).unwrap();

        let session_dir = session_state_dir(root, session_id);
        let marker = agent_marker_path(&session_dir, runtime_id, "agent-a");
        fs::write(&marker, b"{").unwrap();

        let first_error =
            handle_hook_for_runtime_at(&input("Stop", session_id), root, runtime_id, 2_000)
                .unwrap_err();
        assert!(format!("{first_error:#}").contains("解析 Codex 子代理门禁状态失败"));

        let observed = session_auxiliary_path(&session_dir, runtime_id, STATE_ERROR_SINCE_FILE);
        assert_eq!(fs::read_to_string(&observed).unwrap(), "2000\n");
        assert!(marker.exists());

        let recovered = handle_hook_for_runtime_at(
            &input("Stop", session_id),
            root,
            runtime_id,
            2_000 + STOP_STALL_GRACE_MILLIS,
        )
        .unwrap();
        assert_eq!(recovered, json!({}));
        assert!(!marker.exists());
        assert!(!observed.exists());
    }

    #[test]
    fn healthy_active_state_clears_a_stale_corruption_observation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime_id = "runtime-a";
        let session_id = "healthy-session";
        let mut start = input("SubagentStart", session_id);
        start.agent_id = Some("agent-a".to_string());
        handle_hook_for_runtime_at(&start, root, runtime_id, 1_000).unwrap();

        let session_dir = session_state_dir(root, session_id);
        let observed = session_auxiliary_path(&session_dir, runtime_id, STATE_ERROR_SINCE_FILE);
        write_observation_timestamp(&session_dir, &observed, 1_000).unwrap();

        let blocked = handle_hook_for_runtime_at(
            &input("Stop", session_id),
            root,
            runtime_id,
            1_000 + STOP_STALL_GRACE_MILLIS,
        )
        .unwrap();
        assert_eq!(blocked["decision"].as_str(), Some("block"));
        assert!(!observed.exists());
        assert_eq!(
            active_agent_count_for_runtime(root, runtime_id, session_id).unwrap(),
            1
        );
    }

    #[test]
    fn non_terminal_or_unattributed_wait_updates_do_not_release_markers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (index, tool_response) in [
            json!({ "agent_id": "agent-a", "status": "partial" }),
            json!({ "agentId": "agent-a", "type": "MESSAGE" }),
            json!({ "status": "FINAL_ANSWER", "message": "done" }),
            json!({ "agent_id": "agent-a", "message": "FINAL_ANSWER" }),
        ]
        .into_iter()
        .enumerate()
        {
            let session_id = format!("session-{index}");
            let mut start = input("SubagentStart", &session_id);
            start.agent_id = Some("agent-a".to_string());
            handle_hook(&start, root).unwrap();

            let mut wait = input("PostToolUse", &session_id);
            wait.tool_name = Some("agents.wait_agent".to_string());
            wait.tool_response = Some(tool_response);
            let blocked = handle_hook(&wait, root).unwrap();

            assert_eq!(blocked["decision"].as_str(), Some("block"));
            assert_eq!(active_agent_count(root, &session_id).unwrap(), 1);
        }
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
            json!({ "result": { "interrupted_by_user": true } }),
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
    fn ordinary_agent_messages_cannot_clear_the_session_gate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime_id = "runtime-a";
        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());
        handle_hook_for_runtime(&start, root, runtime_id).unwrap();

        let mut wait = input("PostToolUse", "session-a");
        wait.tool_name = Some("agents.wait_agent".to_string());
        wait.tool_response = Some(json!({
            "updates": [{
                "agent_id": "agent-a",
                "type": "MESSAGE",
                "message": "Document the manual stop procedure before continuing",
                "details": { "interrupted_by_user_input": true }
            }]
        }));

        let blocked = handle_hook_for_runtime(&wait, root, runtime_id).unwrap();
        assert_eq!(blocked["decision"].as_str(), Some("block"));
        assert_eq!(
            active_agent_count_for_runtime(root, runtime_id, "session-a").unwrap(),
            1
        );
    }

    #[test]
    fn runtime_generations_fence_stale_markers_and_late_events() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut start = input("SubagentStart", "session-a");
        start.agent_id = Some("agent-a".to_string());

        handle_hook_for_runtime(&start, root, "runtime-old").unwrap();
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-old", "session-a").unwrap(),
            1
        );
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-new", "session-a").unwrap(),
            0
        );

        handle_hook_for_runtime(&start, root, "runtime-new").unwrap();
        let session_dir = session_state_dir(root, "session-a");
        let marker_path = agent_marker_path(&session_dir, "runtime-new", "agent-a");
        let marker: ActiveMarker =
            serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
        assert_eq!(marker.schema_version, ACTIVE_MARKER_SCHEMA_VERSION);
        assert_eq!(marker.runtime_id_hash, hash_component("runtime-new"));
        assert!(marker.started_at_ms > 0);

        let mut late_old_stop = input("SubagentStop", "session-a");
        late_old_stop.agent_id = Some("agent-a".to_string());
        handle_hook_for_runtime(&late_old_stop, root, "runtime-old").unwrap();
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-new", "session-a").unwrap(),
            1
        );

        handle_hook_for_runtime(&input("SessionEnd", "session-a"), root, "runtime-old").unwrap();
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-new", "session-a").unwrap(),
            1
        );

        let mut root_patch = input("PreToolUse", "session-a");
        root_patch.tool_name = Some("apply_patch".to_string());
        assert_eq!(
            handle_hook_for_runtime(&root_patch, root, "runtime-new").unwrap()
                ["hookSpecificOutput"]["permissionDecision"]
                .as_str(),
            Some("deny")
        );
    }

    #[test]
    fn unverifiable_legacy_markers_do_not_block_a_versioned_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session_dir = session_state_dir(root, "session-a");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(format!("{}.active", hash_component("agent-a"))),
            b"active\n",
        )
        .unwrap();

        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-new", "session-a").unwrap(),
            0
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
            "functions.wait",
            "functions/wait",
            "functions:wait",
            "functions__wait",
            "functions_wait",
        ] {
            assert!(is_collaboration_tool(tool), "{tool}");
        }
        assert!(!is_collaboration_tool("functions.exec"));
        assert!(!is_collaboration_tool("update_plan"));
        assert!(is_wait_agent_tool("functions.wait"));
        assert!(is_wait_agent_tool("functions__wait"));
        assert!(!is_wait_agent_tool("functions.exec"));
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
