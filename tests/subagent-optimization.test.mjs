import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("subagent optimization exposes per-task model and reasoning controls", async () => {
  const [appSource, modelHookSource, featurePolicySource] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
  ]);
  const uiSource = `${appSource}\n${featurePolicySource}`;

  assert.match(uiSource, /checked=\{config\.subagentOptimization\}/);
  assert.match(
    uiSource,
    /onCheckedChange=\{\(checked\) =>\s*onSubagentOptimizationChange\(checked\)\s*\}/,
  );
  assert.match(uiSource, /Codey 子代理角色与调度增强/);
  assert.match(uiSource, /aria-label="启用 Codey 子代理角色与调度增强"/);
  assert.match(uiSource, /实际受父任务权限模式约束/);
  for (const role of [
    "codey_quick_scan",
    "codey_deep_research",
    "codey_visual_analysis",
    "codey_worker",
    "codey_visual_worker",
  ]) {
    assert.match(uiSource, new RegExp(`id: "${role}"`));
  }
  for (const label of [
    "快速定位",
    "深度检索",
    "视觉分析",
    "代码实施",
    "视觉实施",
  ]) {
    assert.match(uiSource, new RegExp(`name: "${label}"`));
  }
  assert.doesNotMatch(uiSource, /id: "default"/);
  assert.doesNotMatch(uiSource, /name: "通用兜底"/);
  assert.match(uiSource, /选择性委派/);
  assert.match(uiSource, /五类专用角色/);
  assert.match(uiSource, /config\.subagentOptimization \? \(/);
  assert.match(uiSource, /className="subagent-task-help"/);
  assert.match(uiSource, /content=\{task\.description\}/);
  assert.match(uiSource, /aria-labelledby=\{`\$\{task\.id\}-model-label`\}/);
  assert.match(uiSource, /aria-labelledby=\{`\$\{task\.id\}-effort-label`\}/);
  assert.match(uiSource, /config\.subagentRoles\[task\.id\]/);
  assert.match(uiSource, /\[task\.id\]: \{ model, reasoningEffort \}/);
  assert.match(
    uiSource,
    /subagentPolicyControlsDisabled \|\|\s*subagentModelOptions\.length === 0/,
  );
  assert.match(
    uiSource,
    /subagentPolicyControlsDisabled \|\|\s*reasoningEfforts\.length === 0/,
  );
  assert.match(
    modelHookSource,
    /modelState\.officialModels\s*\.filter\(\(model\) => model\.supported\)/,
  );
  assert.match(modelHookSource, /\.\.\.modelState\.thirdPartyModels\s*\.map/);
  assert.doesNotMatch(modelHookSource, /subagentModelIds|subagentModelKeys/);
  assert.doesNotMatch(
    uiSource,
    /check-subagent-model|当前线路没有 Codex 子代理工具可用的模型/,
  );
  assert.doesNotMatch(uiSource, /仅接受 Sol \/ Terra/);
});

test("leaf subagent models do not inherit coordinator capability markers", async () => {
  const catalogSource = await readFile(
    new URL("backend/src/model_catalog.rs", root),
    "utf8",
  );

  assert.doesNotMatch(catalogSource, /enable_subagents_for_all_models/);
  assert.match(catalogSource, /object\.remove\("multi_agent_version"\)/);
  assert.match(
    catalogSource,
    /generated_catalog_preserves_official_multi_agent_markers/,
  );
  assert.match(
    catalogSource,
    /generated_catalog_keeps_leaf_models_without_v2_coordinator_markers/,
  );
});

test("per-task subagent files are composed at startup and hot-refreshed after save", async () => {
  const [commandSource, configSource, launcherSource, rendererSource, vendorRendererSource] = await Promise.all([
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config.rs", root), "utf8"),
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
    readFile(new URL("public/renderer-inject.js", root), "utf8"),
    readFile(new URL("vendor/CodeyRuntime/assets/inject/renderer-inject.js", root), "utf8"),
  ]);

  assert.match(commandSource, /hot_reload_runtime_subagent_config/);
  assert.match(commandSource, /refresh_runtime_subagent_roles/);
  assert.doesNotMatch(commandSource, /let refresh_subagent_defaults = false/);
  assert.match(configSource, /fn prepare_runtime_agent_files/);
  assert.match(configSource, /document\["model"\] = value\(model\)/);
  assert.match(
    configSource,
    /document\["model_reasoning_effort"\] = value\(&reasoning_effort\)/,
  );
  assert.match(configSource, /register_runtime_agents/);
  assert.match(configSource, /agents\.\{\}\.config_file/);
  assert.match(configSource, /pub fn refresh_runtime_subagent_roles/);
  assert.match(configSource, /verify_runtime_agent_files/);
  assert.match(configSource, /restore_runtime_agent_files_and_lease/);
  assert.match(configSource, /atomic_write\(runtime_path, rendered\.as_bytes\(\)\)/);
  assert.doesNotMatch(configSource, /write_private_file\(runtime_path, rendered\.as_bytes\(\)\)/);
  assert.match(launcherSource, /subagent_roles: Some\(&subagent_roles\)/);
  assert.match(launcherSource, /supports_subagent_config_hot_reload/);
  assert.doesNotMatch(rendererSource, /__codeyApplySubagentDefaults/);
  assert.doesNotMatch(vendorRendererSource, /__codeyApplySubagentDefaults/);
  assert.doesNotMatch(vendorRendererSource, /patchAppServerSubagentRequestParams/);
});

test("subagent optimization installs recoverable orchestration and runtime gates", async () => {
  const [gateSource, orchestratorSource, protocolSource, controlSource, controlConfigSource, rulesSource, configSource, guidanceSource, mainSource] = await Promise.all([
    readFile(new URL("backend/src/subagent_gate.rs", root), "utf8"),
    readFile(new URL("backend/src/subagent_orchestrator.rs", root), "utf8"),
    readFile(new URL("backend/src/subagent/protocol.rs", root), "utf8"),
    readFile(new URL("backend/src/subagent_control_mcp.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config/subagent_control.rs", root), "utf8"),
    readFile(new URL("backend/resources/subagent-rules.default.json", root), "utf8"),
    readFile(new URL("backend/src/codex_config.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config_guidance.rs", root), "utf8"),
    readFile(new URL("backend/src/main.rs", root), "utf8"),
  ]);

  assert.match(mainSource, /run_subagent_gate_hook_if_requested/);
  assert.match(mainSource, /run_subagent_control_mcp_if_requested/);
  assert.match(
    configSource,
    /enable_subagent_gate_hooks\(doc, config_path, fastctx_namespace\.is_some\(\)\)/,
  );
  assert.match(configSource, /COMBINED_HOOK_ARGUMENT/);
  assert.match(configSource, /multi_agent_mode_hint_text/);
  assert.match(configSource, /ROOT_AGENT_MULTI_AGENT_MODE_HINT/);
  assert.match(configSource, /&SUBAGENT_GATE_HOOKS\[\.\.1\]/);
  assert.match(configSource, /&SUBAGENT_GATE_HOOKS\[1\.\.\]/);
  assert.match(configSource, /if !subagent_optimization/);
  assert.match(configSource, /toml_event: "PreToolUse"/);
  assert.match(configSource, /toml_event: "PostToolUse"/);
  assert.match(
    configSource,
    /matcher: Some\(crate::subagent_orchestrator::POST_TOOL_HOOK_MATCHER\)/,
  );
  assert.match(
    orchestratorSource,
    /pub\(crate\) const POST_TOOL_HOOK_MATCHER: &str = "\*"/,
  );
  assert.match(configSource, /toml_event: "SubagentStart"/);
  assert.match(configSource, /toml_event: "SubagentStop"/);
  assert.match(configSource, /toml_event: "Stop"/);
  assert.match(configSource, /trusted_hash/);
  assert.match(gateSource, /nonempty\(input\.agent_id\.as_deref\(\)\)\.is_some\(\)/);
  assert.match(gateSource, /input_has_subagent_context/);
  assert.match(gateSource, /SUBAGENT_CONTEXT_OBSERVED_FILE/);
  assert.match(gateSource, /missing_agent_id_has_classified_subagent_context/);
  assert.match(gateSource, /permissionDecision": "deny"/);
  assert.match(gateSource, /"decision": "block"/);
  assert.match(gateSource, /is_collaboration_tool/);
  assert.match(rulesSource, /deny-nested-spawn/);
  assert.doesNotMatch(gateSource, /fn subagent_spawn_denial/);
  assert.match(gateSource, /post_wait_continuation/);
  assert.match(gateSource, /只可使用 agents\.send_message/);
  assert.match(gateSource, /不得恢复非协作本地工作/);
  assert.match(gateSource, /MAX_RENDERED_TOOL_RESULT_CHARS/);
  assert.match(gateSource, /STATE_ERROR_SINCE_FILE/);
  assert.match(gateSource, /active_agent_count_or_recover_corrupt_state/);
  assert.match(gateSource, /协作工具返回内容已截断/);
  assert.match(gateSource, /SubagentStop/);
  assert.match(gateSource, /transcript_path: Option<String>/);
  assert.match(gateSource, /agent_transcript_path: Option<String>/);
  assert.match(gateSource, /subagent_started_with_context/);
  assert.match(gateSource, /subagent_orchestrator::pre_spawn/);
  assert.match(gateSource, /subagent_orchestrator::authorize_child_tool/);
  assert.match(gateSource, /subagent_orchestrator::pending_acceptance_reason/);
  assert.match(orchestratorSource, /CODEY_DELEGATION_V2=/);
  assert.match(orchestratorSource, /CODEY_DELEGATION_V1=/);
  assert.match(orchestratorSource, /struct SessionLedger/);
  const ledgerSchemaVersion = Number(
    orchestratorSource.match(/const LEDGER_SCHEMA_VERSION: u32 = (\d+)/)?.[1],
  );
  assert.ok(ledgerSchemaVersion >= 6, `unexpected ledger schema ${ledgerSchemaVersion}`);
  assert.match(orchestratorSource, /MAX_BATCHES_PER_TURN: u16 = 3/);
  assert.match(orchestratorSource, /MAX_TOTAL_ATTEMPTS_PER_TURN/);
  assert.match(orchestratorSource, /CODEY_SUBAGENT_BATCH_BUDGET_EXHAUSTED/);
  assert.match(orchestratorSource, /CODEY_SUBAGENT_TURN_BUDGET_EXHAUSTED/);
  assert.match(orchestratorSource, /CODEY_SUBAGENT_DUPLICATE_TASK_ID/);
  assert.match(orchestratorSource, /duplicate_task_id_denial/);
  assert.match(orchestratorSource, /CODEY_SUBAGENT_FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT/);
  assert.match(orchestratorSource, /CODEY_SUBAGENT_UNBOUND_ATTEMPT/);
  assert.match(orchestratorSource, /task_id_from_subagent_transcript/);
  assert.match(orchestratorSource, /MAX_TRANSCRIPT_METADATA_LINE_BYTES/);
  assert.match(orchestratorSource, /parse_json_encoded_spawn_response/);
  assert.match(protocolSource, /decode_json_encoded_response/);
  assert.match(orchestratorSource, /pub\(crate\) fn pre_followup_task/);
  assert.match(gateSource, /is_followup_task_tool/);
  assert.match(gateSource, /subagent_orchestrator::pre_followup_task/);
  assert.match(orchestratorSource, /不带筛选的 `agents\.list_agents` 对账/);
  assert.match(guidanceSource, /A `CODEY_SUBAGENT_DUPLICATE_TASK_ID` denial is a recovery event/);
  assert.match(guidanceSource, /never retry the old `task_name` or Stop immediately/);
  assert.match(guidanceSource, /an exactly matching `CODEY_DELEGATION_V2\.id`/);
  assert.match(guidanceSource, /`followup_task` only while/);
  assert.match(orchestratorSource, /advance_batch_if_settled/);
  assert.match(orchestratorSource, /enum RootBatchDecision/);
  assert.match(orchestratorSource, /enum BatchDecisionState/);
  assert.match(orchestratorSource, /prepare_batch_decision/);
  assert.match(orchestratorSource, /post_batch_decision/);
  assert.match(orchestratorSource, /decision_receipt_matches/);
  assert.match(gateSource, /is_batch_decision_tool/);
  assert.match(gateSource, /batch_decision_stop_reason/);
  assert.match(controlSource, /pub\(crate\) const TOOL_NAME: &str = "resolve_batch"/);
  assert.match(controlSource, /"spawn_next_batch", "continue_root", "complete", "blocked"/);
  assert.match(controlConfigSource, /server\["enabled_tools"\]/);
  assert.match(controlConfigSource, /server\["disabled_tools"\]/);
  assert.match(controlConfigSource, /resolve_batch\["approval_mode"\] = value\("approve"\)/);
  assert.doesNotMatch(
    controlConfigSource,
    /server\["default_tools_approval_mode"\]\s*=/,
  );
  assert.match(configSource, /enable_subagent_control_mcp/);
  assert.match(configSource, /mcp_servers\.codey_subagent_control\.tools\.resolve_batch\.approval_mode/);
  assert.match(guidanceSource, /mcp__codey_subagent_control__resolve_batch/);
  assert.match(orchestratorSource, /CODEY_SUBAGENT_CONTROL_PLANE_FAILED/);
  assert.match(orchestratorSource, /MAX_BATCH_DECISION_CONTROL_FAILURES: u16 = 3/);
  assert.match(orchestratorSource, /ControlPlaneFailed/);
  assert.match(orchestratorSource, /issued_task_ids/);
  assert.match(orchestratorSource, /lock_exclusive/);
  assert.match(orchestratorSource, /resource_conflict/);
  assert.match(orchestratorSource, /classify_acceptance_evidence/);
  assert.match(orchestratorSource, /enum AcceptanceEvidence/);
});
