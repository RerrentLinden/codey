//! Delegation-contract parsing and validation.
//!
//! Resource paths produced here are coordination metadata. Runtime filesystem
//! authority remains with Codex and is enforced outside this module.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::subagent::api::{InvocationMode, TraceContext};
#[cfg(test)]
use crate::subagent::rules;
use crate::subagent::rules::{RoleAccess, RolePolicy, RuleSet};

use super::CONTRACT_PREFIX;
use super::identity::consistent_string_field;

pub(super) const LEGACY_CONTRACT_PREFIX_V1: &str = "CODEY_DELEGATION_V1=";
pub(super) const MAX_CLAIMS_PER_MODE: usize = 16;
pub(super) const MAX_ACCEPTANCE_CHECKS: usize = 8;
pub(super) const MAX_ACCEPTANCE_COMMAND_CHARS: usize = 1024;
pub(super) const MAX_ACCEPTANCE_TOTAL_CHARS: usize = 4 * 1024;
pub(super) const MAX_CONTRACT_LINE_CHARS: usize = 8 * 1024;
pub(super) const MAX_REASON_CHARS: usize = 128;
pub(super) const MAX_SCHEMA_BYTES: usize = 4 * 1024;
pub(super) const MAX_SCHEMA_DEPTH: usize = 16;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DelegationContract {
    pub(super) id: String,
    #[serde(rename = "why")]
    pub(super) reason: String,
    #[serde(default)]
    pub(super) visual: bool,
    #[serde(default, rename = "root")]
    pub(super) workspace_root: Option<String>,
    #[serde(default, rename = "read")]
    pub(super) read_paths: Vec<String>,
    #[serde(default, rename = "write")]
    pub(super) write_paths: Vec<String>,
    #[serde(default, rename = "checks")]
    pub(super) acceptance: Vec<AcceptanceSpec>,
    #[serde(default)]
    pub(super) mode: InvocationMode,
    #[serde(default)]
    pub(super) trace_id: Option<String>,
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    #[serde(default)]
    pub(super) capabilities: Vec<String>,
    #[serde(default)]
    pub(super) deadline_ms: Option<u64>,
    #[serde(default)]
    pub(super) input_schema: Option<Value>,
    #[serde(default)]
    pub(super) output_schema: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AcceptanceSpec {
    pub(super) id: String,
    #[serde(rename = "cmd")]
    pub(super) command: String,
}

#[derive(Clone, Debug)]
pub(super) struct PreparedContract {
    pub(super) contract: DelegationContract,
    pub(super) role: String,
    pub(super) policy: RolePolicy,
    pub(super) workspace_root: Option<String>,
    pub(super) read_paths: Vec<String>,
    pub(super) native_read_scope: bool,
    pub(super) write_paths: Vec<String>,
    pub(super) trace: TraceContext,
    pub(super) invocation_mode: InvocationMode,
    pub(super) capabilities: Vec<String>,
}

#[cfg(test)]
pub(super) fn prepare_contract(
    tool_input: Option<&Value>,
) -> std::result::Result<PreparedContract, String> {
    prepare_contract_with_workspace(tool_input, None)
}

#[cfg(test)]
pub(super) fn prepare_contract_with_workspace(
    tool_input: Option<&Value>,
    hook_workspace_root: Option<&str>,
) -> std::result::Result<PreparedContract, String> {
    prepare_contract_with_rules(tool_input, hook_workspace_root, rules::embedded())
}

pub(super) fn prepare_contract_with_rules(
    tool_input: Option<&Value>,
    hook_workspace_root: Option<&str>,
    rule_set: &RuleSet,
) -> std::result::Result<PreparedContract, String> {
    let input = tool_input
        .and_then(Value::as_object)
        .ok_or_else(|| contract_error("spawn 输入不是 JSON object"))?;
    let task_name = consistent_spawn_field(input, &["task_name", "taskName"], "task_name")?
        .ok_or_else(|| contract_error("缺少 task_name"))?;
    let role = consistent_spawn_field(
        input,
        &["agent_type", "agentType", "agent_role", "agentRole"],
        "agent_type",
    )?
    .ok_or_else(|| contract_error("缺少 agent_type"))?;
    let message = consistent_spawn_field(input, &["message", "prompt"], "message")?
        .ok_or_else(|| contract_error("缺少 message"))?;
    let fork_turns = consistent_spawn_field(input, &["fork_turns", "forkTurns"], "fork_turns")?
        .ok_or_else(|| contract_error("缺少 fork_turns；必须显式为 none"))?;
    if fork_turns != "none" {
        return Err(contract_error("fork_turns 必须为 none"));
    }
    let policy = rule_set
        .role_policy(role)
        .ok_or_else(|| contract_error(&format!("未知或不允许的 agent_type `{role}`")))?;
    if is_opaque_encrypted_message(message) {
        return prepare_opaque_contract(task_name, role, policy, hook_workspace_root);
    }
    let line = message
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .ok_or_else(|| contract_error("message 为空"))?;
    let (payload, legacy_v1) = if let Some(payload) = line.strip_prefix(CONTRACT_PREFIX) {
        (payload, false)
    } else if let Some(payload) = line.strip_prefix(LEGACY_CONTRACT_PREFIX_V1) {
        (payload, true)
    } else {
        return Err(contract_error(
            "message 最后一行缺少 CODEY_DELEGATION_V2 契约",
        ));
    };
    if payload.chars().count() > MAX_CONTRACT_LINE_CHARS {
        return Err(contract_error("契约行超过 8K 字符"));
    }
    let mut contract_value: Value = serde_json::from_str(payload)
        .map_err(|error| contract_error(&format!("契约 JSON 无效：{error}")))?;
    if legacy_v1 {
        let values = contract_value
            .as_object_mut()
            .ok_or_else(|| contract_error("V1 契约 JSON 必须是 object"))?;
        for retired in [
            "calls",
            "files",
            "dirs",
            "large",
            "risk",
            "budget_class",
            "branch_calls",
        ] {
            values.remove(retired);
        }
    }
    let contract: DelegationContract = serde_json::from_value(contract_value)
        .map_err(|error| contract_error(&format!("契约 JSON 无效：{error}")))?;
    validate_task_id(&contract.id)?;
    if contract.id != task_name {
        return Err(contract_error("契约 id 必须与 task_name 完全一致"));
    }
    validate_delegation_reason(&contract.reason)?;
    if contract.visual != policy.visual {
        return Err(contract_error(if policy.visual {
            "视觉角色的契约必须设置 visual=true"
        } else {
            "非视觉角色不能声明 visual=true"
        }));
    }
    if contract.read_paths.len() > MAX_CLAIMS_PER_MODE
        || contract.write_paths.len() > MAX_CLAIMS_PER_MODE
    {
        return Err(contract_error("read/write 资源声明各自最多 16 项"));
    }
    if contract.acceptance.len() > MAX_ACCEPTANCE_CHECKS {
        return Err(contract_error("checks 最多 8 项"));
    }
    if contract.capabilities.len() > 16 {
        return Err(contract_error("capabilities 最多 16 项"));
    }
    let mut capabilities = contract
        .capabilities
        .iter()
        .map(|capability| capability.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    for capability in &capabilities {
        if capability.is_empty()
            || capability.chars().count() > 64
            || !capability.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
            })
        {
            return Err(contract_error(
                "capabilities 必须为 1..=64 个 ASCII 字母、数字、点、连字符或下划线",
            ));
        }
        if !matches!(
            capability.as_str(),
            "files.read" | "workspace.write" | "command.execute"
        ) {
            return Err(contract_error(&format!(
                "未知 capability `{capability}`；仅支持 files.read、workspace.write、command.execute"
            )));
        }
    }
    capabilities.sort();
    capabilities.dedup();
    if legacy_v1 && capabilities.is_empty() && policy.access == RoleAccess::ReadOnly {
        capabilities.push("files.read".to_string());
    }
    if contract.deadline_ms.is_some_and(|deadline| deadline == 0) {
        return Err(contract_error("deadline_ms 必须大于 0"));
    }
    for (name, schema) in [
        ("input_schema", contract.input_schema.as_ref()),
        ("output_schema", contract.output_schema.as_ref()),
    ] {
        if let Some(schema) = schema {
            validate_contract_schema(name, schema)?;
        }
    }
    let trace =
        TraceContext::normalized(contract.trace_id.as_deref(), contract.parent_id.as_deref())
            .map_err(|error| contract_error(&error))?;
    let workspace_root = if let Some(root) = contract.workspace_root.as_deref() {
        Some(
            normalize_coordination_path(root)
                .map_err(|error| contract_error(&format!("root 无效：{error}")))?,
        )
    } else {
        hook_workspace_root.and_then(|root| normalize_coordination_path(root).ok())
    };
    let mut read_paths = normalize_claims(&contract.read_paths, workspace_root.as_deref())?;
    let write_paths = normalize_claims(&contract.write_paths, workspace_root.as_deref())?;
    if read_paths.is_empty() {
        if policy.access == RoleAccess::Write {
            read_paths.clone_from(&write_paths);
        } else if let Some(root) = workspace_root.as_ref() {
            read_paths.push(root.clone());
        }
    }
    match policy.access {
        RoleAccess::ReadOnly => {
            if !write_paths.is_empty() || !contract.acceptance.is_empty() {
                return Err(contract_error("只读角色不能声明 write 或 checks"));
            }
            if capabilities
                .iter()
                .any(|capability| capability == "workspace.write")
            {
                return Err(contract_error(
                    "只读角色不能声明 workspace.write capability",
                ));
            }
        }
        RoleAccess::Write => {
            if write_paths.is_empty() {
                return Err(contract_error("写入角色必须声明至少一个 write ownership"));
            }
            if contract.acceptance.is_empty() {
                return Err(contract_error("写入角色必须声明至少一个机械 checks"));
            }
            if !capabilities
                .iter()
                .any(|capability| capability == "workspace.write")
            {
                return Err(contract_error(
                    "写入角色必须声明 workspace.write capability",
                ));
            }
        }
    }
    if !capabilities
        .iter()
        .any(|capability| capability == "files.read")
    {
        return Err(contract_error(
            "所有可执行契约都必须显式声明 files.read capability",
        ));
    }
    let mut check_ids = BTreeSet::new();
    let mut total_check_chars = 0_usize;
    for check in &contract.acceptance {
        validate_task_id(&check.id)?;
        if !check_ids.insert(check.id.as_str()) {
            return Err(contract_error("checks id 不能重复"));
        }
        let command = check.command.trim();
        if command.is_empty() || command.chars().count() > MAX_ACCEPTANCE_COMMAND_CHARS {
            return Err(contract_error("checks cmd 必须为 1..=1024 个字符"));
        }
        total_check_chars = total_check_chars.saturating_add(command.chars().count());
        if total_check_chars > MAX_ACCEPTANCE_TOTAL_CHARS {
            return Err(contract_error("checks 命令总长度不能超过 4096 个字符"));
        }
        if command
            .lines()
            .next()
            .is_some_and(|line| line.trim_start().starts_with("# codey-accept:"))
        {
            return Err(contract_error("checks cmd 不能自行包含 codey-accept 标记"));
        }
    }
    Ok(PreparedContract {
        invocation_mode: contract.mode,
        capabilities,
        trace,
        contract,
        role: role.to_string(),
        policy,
        workspace_root,
        read_paths,
        native_read_scope: false,
        write_paths,
    })
}

fn consistent_spawn_field<'a>(
    input: &'a Map<String, Value>,
    aliases: &[&str],
    field_name: &str,
) -> std::result::Result<Option<&'a str>, String> {
    consistent_string_field(input, aliases)
        .map_err(|()| contract_error(&format!("{field_name} 别名冲突或类型无效")))
}

pub(super) fn is_opaque_encrypted_message(message: &str) -> bool {
    let message = message.trim();
    message.len() >= 128
        && message.starts_with("gAAAAA")
        && message
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'='))
}

pub(super) fn prepare_opaque_contract(
    task_name: &str,
    role: &str,
    policy: RolePolicy,
    hook_workspace_root: Option<&str>,
) -> std::result::Result<PreparedContract, String> {
    validate_task_id(task_name)?;
    if policy.access == RoleAccess::Write {
        return Err(contract_error(
            "message 已由上游加密，无法验证 write ownership 与机械 checks；写入角色必须使用可验证的明文或签名 sidecar 契约",
        ));
    }
    let workspace_root =
        hook_workspace_root.and_then(|root| normalize_coordination_path(root).ok());
    let workspace_claims = workspace_root.iter().cloned().collect::<Vec<_>>();
    let read_paths = workspace_claims;
    let write_paths = Vec::new();
    let contract = DelegationContract {
        id: task_name.to_string(),
        reason: "encrypted_message".to_string(),
        visual: policy.visual,
        workspace_root: workspace_root.clone(),
        read_paths: read_paths.clone(),
        write_paths: write_paths.clone(),
        acceptance: Vec::new(),
        mode: InvocationMode::Async,
        trace_id: None,
        parent_id: None,
        capabilities: vec!["files.read".to_string(), "command.execute".to_string()],
        deadline_ms: None,
        input_schema: None,
        output_schema: None,
    };
    Ok(PreparedContract {
        trace: TraceContext::new(None),
        invocation_mode: InvocationMode::Async,
        capabilities: contract.capabilities.clone(),
        contract,
        role: role.to_string(),
        policy,
        workspace_root,
        read_paths,
        native_read_scope: true,
        write_paths,
    })
}

pub(super) fn contract_error(detail: &str) -> String {
    format!(
        "Codey 自适应委派门禁：{detail}。请在 message 最后一行追加紧凑契约，例如：{CONTRACT_PREFIX}{{\"id\":\"scan_auth\",\"why\":\"breadth\",\"visual\":false,\"read\":[],\"write\":[],\"capabilities\":[\"files.read\"],\"checks\":[]}}"
    )
}

pub(super) fn validate_task_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(contract_error(
            "id/check id 只允许 1..=64 个小写字母、数字或下划线",
        ));
    }
    Ok(())
}

pub(super) fn validate_delegation_reason(reason: &str) -> std::result::Result<(), String> {
    let reason = reason.trim();
    if reason.is_empty()
        || reason.chars().count() > MAX_REASON_CHARS
        || reason.chars().any(char::is_control)
    {
        return Err(contract_error(
            "why 必须为 1..=128 个不含换行或控制字符的审计说明",
        ));
    }
    Ok(())
}

pub(super) fn validate_contract_schema(
    name: &str,
    schema: &Value,
) -> std::result::Result<(), String> {
    if !schema.is_object() {
        return Err(contract_error(&format!("{name} 必须为 JSON object")));
    }
    let encoded = serde_json::to_vec(schema)
        .map_err(|error| contract_error(&format!("{name} 无法序列化：{error}")))?;
    if encoded.len() > MAX_SCHEMA_BYTES {
        return Err(contract_error(&format!(
            "{name} 序列化后不能超过 {MAX_SCHEMA_BYTES} 字节"
        )));
    }
    validate_schema_node(name, schema, 0)
}

pub(super) fn validate_schema_node(
    name: &str,
    schema: &Value,
    depth: usize,
) -> std::result::Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(contract_error(&format!(
            "{name} 嵌套深度不能超过 {MAX_SCHEMA_DEPTH}"
        )));
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let Some(object) = schema.as_object() else {
        return Err(contract_error(&format!(
            "{name} 的子 schema 必须为 object 或 boolean"
        )));
    };
    if let Some(schema_type) = object.get("type") {
        let valid_type = |value: &str| {
            matches!(
                value,
                "null" | "boolean" | "object" | "array" | "number" | "integer" | "string"
            )
        };
        let valid = schema_type.as_str().is_some_and(valid_type)
            || schema_type.as_array().is_some_and(|values| {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| value.as_str().is_some_and(valid_type))
            });
        if !valid {
            return Err(contract_error(&format!(
                "{name}.type 必须是合法 JSON Schema 类型或非空类型数组"
            )));
        }
    }
    if let Some(required) = object.get("required")
        && !required.as_array().is_some_and(|values| {
            values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        })
    {
        return Err(contract_error(&format!(
            "{name}.required 必须为非空字符串数组"
        )));
    }
    if let Some(properties) = object.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err(contract_error(&format!(
                "{name}.properties 必须为 JSON object"
            )));
        };
        for (property, child) in properties {
            validate_schema_node(&format!("{name}.properties.{property}"), child, depth + 1)?;
        }
    }
    if let Some(items) = object.get("items") {
        match items {
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    validate_schema_node(&format!("{name}.items[{index}]"), child, depth + 1)?;
                }
            }
            child => validate_schema_node(&format!("{name}.items"), child, depth + 1)?,
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(children) = object.get(keyword) {
            let Some(children) = children.as_array() else {
                return Err(contract_error(&format!("{name}.{keyword} 必须为数组")));
            };
            for (index, child) in children.iter().enumerate() {
                validate_schema_node(&format!("{name}.{keyword}[{index}]"), child, depth + 1)?;
            }
        }
    }
    for keyword in ["not", "additionalProperties", "contains"] {
        if let Some(child) = object.get(keyword) {
            validate_schema_node(&format!("{name}.{keyword}"), child, depth + 1)?;
        }
    }
    Ok(())
}

pub(super) fn normalize_claims(
    claims: &[String],
    workspace_root: Option<&str>,
) -> std::result::Result<Vec<String>, String> {
    let mut normalized = BTreeSet::new();
    for claim in claims {
        let path = if is_absolute_path(claim) {
            normalize_coordination_path(claim)
        } else if let Some(root) = workspace_root {
            normalize_coordination_path(&format!("{}/{}", root.trim_end_matches('/'), claim))
        } else {
            Err("相对资源路径需要绝对 root".to_string())
        }
        .map_err(|error| contract_error(&format!("资源路径 `{claim}` 无效：{error}")))?;
        normalized.insert(path);
    }
    Ok(normalized.into_iter().collect())
}

/// Canonicalize existing ancestors when possible so coordination claims for
/// obvious aliases overlap. These paths are scheduling metadata, not a file
/// ACL: metadata/canonicalization failures fall back to the lexical absolute
/// path, while the Codex executor remains the only filesystem authority.
pub(super) fn normalize_coordination_path(value: &str) -> std::result::Result<String, String> {
    let lexical = normalize_absolute_path(value)?;
    let path = PathBuf::from(&lexical);
    if !path.is_absolute() {
        // A foreign-platform drive path cannot be resolved on this host. Keep
        // its already-normalized lexical form for portable ledger migration.
        return Ok(lexical);
    }
    let mut ancestor = path.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Ok(lexical);
                };
                missing.push(name.to_os_string());
                let Some(parent) = ancestor.parent() else {
                    return Ok(lexical);
                };
                ancestor = parent;
            }
            Err(_) => return Ok(lexical),
        }
    }
    let Ok(mut resolved) = fs::canonicalize(ancestor) else {
        return Ok(lexical);
    };
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(normalize_absolute_path(&resolved.to_string_lossy()).unwrap_or(lexical))
}

pub(super) fn normalize_absolute_path(value: &str) -> std::result::Result<String, String> {
    let mut replaced = value.trim().replace('\\', "/");
    if let Some(verbatim) = replaced.strip_prefix("//?/") {
        replaced = if verbatim
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("unc/"))
        {
            format!("//{}", &verbatim[4..])
        } else {
            verbatim.to_string()
        };
    }
    if replaced.is_empty() || replaced.contains(['*', '?', '[', ']']) {
        return Err("必须是无 glob 的绝对路径".to_string());
    }
    let (prefix, rest) = if let Some(rest) = replaced.strip_prefix("//") {
        ("//".to_string(), rest)
    } else if replaced.starts_with('/') {
        ("/".to_string(), replaced.trim_start_matches('/'))
    } else if replaced.len() >= 3
        && replaced.as_bytes()[0].is_ascii_alphabetic()
        && replaced.as_bytes()[1] == b':'
        && replaced.as_bytes()[2] == b'/'
    {
        (
            format!(
                "{}:/",
                (replaced.as_bytes()[0] as char).to_ascii_uppercase()
            ),
            &replaced[3..],
        )
    } else {
        return Err("必须是 Unix、UNC 或盘符绝对路径".to_string());
    };
    let mut components = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err("路径不能越过根目录".to_string());
                }
            }
            component => components.push(component),
        }
    }
    let joined = components.join("/");
    let mut result = if joined.is_empty() {
        prefix
    } else {
        format!("{prefix}{joined}")
    };
    if cfg!(windows) {
        result.make_ascii_lowercase();
    }
    Ok(result)
}

pub(super) fn is_absolute_path(value: &str) -> bool {
    let value = value.trim().replace('\\', "/");
    value.starts_with('/')
        || value.starts_with("//")
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && value.as_bytes()[2] == b'/')
}
