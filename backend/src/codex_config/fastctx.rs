use std::path::Path;

use anyhow::Result;
use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value, value};

use super::{
    CODEY_FASTCTX_ARG_MARKER, CODEY_FASTCTX_GLOB_TOKEN_BUDGET, CODEY_FASTCTX_GREP_TOKEN_BUDGET,
    CODEY_FASTCTX_HOST_TOKEN_LIMIT, CODEY_FASTCTX_NAMESPACE, CODEY_FASTCTX_SERVER_ID,
    CODEY_FASTCTX_STARTUP_TIMEOUT_SECONDS, CODEY_FASTCTX_TOKEN_BUDGET,
    CODEY_FASTCTX_TOOL_TIMEOUT_SECONDS, FastContextToolsStatus, ensure_child_table,
    ensure_root_table, fastctx_table_server_is_codey_owned,
};
use crate::codex_config_guidance::{
    codey_fastctx_guidance_for_namespace, remove_codey_fastctx_guidance,
};

pub(super) fn enable_fast_context_tools(
    doc: &mut DocumentMut,
    command: &Path,
) -> Result<Option<String>> {
    let codey_owned_server = mcp_server_is_codey_owned_by_id(doc, CODEY_FASTCTX_SERVER_ID);
    if configured_user_fastctx_server_id(doc).is_some() {
        disable_fast_context_tools(doc);
        return Ok(None);
    }
    let budgets = output_budgets(doc);

    let mcp_servers = ensure_mcp_servers_table(doc)?;
    let server_table = if codey_owned_server {
        mcp_servers
            .get(CODEY_FASTCTX_SERVER_ID)
            .and_then(item_table_clone)
            .unwrap_or_default()
    } else {
        Table::new()
    };
    mcp_servers.insert(CODEY_FASTCTX_SERVER_ID, Item::Table(server_table));
    let server = mcp_servers
        .get_mut(CODEY_FASTCTX_SERVER_ID)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            anyhow::anyhow!("mcp_servers.{CODEY_FASTCTX_SERVER_ID} 必须是 TOML table")
        })?;
    server["command"] = value(command.to_string_lossy().to_string());
    let mut args = Array::new();
    args.push(CODEY_FASTCTX_ARG_MARKER);
    server["args"] = Item::Value(toml_edit::Value::Array(args));
    server["startup_timeout_sec"] = value(CODEY_FASTCTX_STARTUP_TIMEOUT_SECONDS);
    server["tool_timeout_sec"] = value(CODEY_FASTCTX_TOOL_TIMEOUT_SECONDS);
    let mut env = server
        .get("env")
        .and_then(item_table_clone)
        .unwrap_or_default();
    env["FASTCTX_TOKEN_BUDGET"] = value(budgets.global.to_string());
    env["FASTCTX_GREP_TOKEN_BUDGET"] = value(budgets.grep.to_string());
    env["FASTCTX_GLOB_TOKEN_BUDGET"] = value(budgets.glob.to_string());
    server["env"] = Item::Table(env);

    // FastCtx reserves a terminal Complete/Partial line. Keeping it direct-only
    // prevents code-mode aggregation from truncating that continuation contract.
    ensure_direct_only_tool_namespace(doc, CODEY_FASTCTX_NAMESPACE)?;
    apply_fastctx_guidance(doc, CODEY_FASTCTX_NAMESPACE)?;
    Ok(Some(CODEY_FASTCTX_NAMESPACE.to_string()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputBudgets {
    global: usize,
    grep: usize,
    glob: usize,
}

fn output_budgets(doc: &mut DocumentMut) -> OutputBudgets {
    let host_limit = doc
        .get("tool_output_token_limit")
        .and_then(Item::as_integer)
        .and_then(|limit| usize::try_from(limit).ok())
        .filter(|limit| *limit > 0)
        .unwrap_or_else(|| {
            doc["tool_output_token_limit"] = value(CODEY_FASTCTX_HOST_TOKEN_LIMIT);
            CODEY_FASTCTX_HOST_TOKEN_LIMIT as usize
        });
    let global = host_limit
        .saturating_mul(9)
        .saturating_div(10)
        .clamp(1, CODEY_FASTCTX_TOKEN_BUDGET);
    OutputBudgets {
        global,
        grep: global.min(CODEY_FASTCTX_GREP_TOKEN_BUDGET),
        glob: global.min(CODEY_FASTCTX_GLOB_TOKEN_BUDGET),
    }
}

fn apply_fastctx_guidance(doc: &mut DocumentMut, namespace: &str) -> Result<()> {
    apply_fastctx_guidance_to_table(
        doc.as_table_mut(),
        "developer_instructions",
        namespace,
        "developer_instructions",
    )
}

pub(super) fn apply_fastctx_guidance_to_table(
    table: &mut Table,
    key: &str,
    namespace: &str,
    qualified_key: &str,
) -> Result<()> {
    let desired_guidance = codey_fastctx_guidance_for_namespace(namespace);
    let existing_guidance = table
        .get(key)
        .map(|item| {
            item.as_str()
                .ok_or_else(|| anyhow::anyhow!("{qualified_key} 必须是字符串"))
        })
        .transpose()?
        .unwrap_or_default();
    let (existing_guidance, fastctx_guidance_was_cleaned) =
        if let Some(cleaned_guidance) = remove_codey_fastctx_guidance(existing_guidance) {
            (cleaned_guidance, true)
        } else {
            (existing_guidance.to_string(), false)
        };
    let fastctx_guidance_needs_append = !existing_guidance.contains(&desired_guidance);
    if fastctx_guidance_was_cleaned || fastctx_guidance_needs_append {
        let guidance = if !fastctx_guidance_needs_append {
            existing_guidance
        } else if existing_guidance.trim().is_empty() {
            desired_guidance
        } else {
            format!("{existing_guidance}\n\n{desired_guidance}")
        };
        table[key] = value(guidance);
    }
    Ok(())
}

pub(super) fn disable_fast_context_tools(doc: &mut DocumentMut) {
    let codey_owned_server_removed = match doc.get_mut("mcp_servers") {
        Some(Item::Table(mcp_servers)) => {
            let codey_owned_server = mcp_servers
                .get(CODEY_FASTCTX_SERVER_ID)
                .is_some_and(fastctx_item_server_is_codey_owned);
            if codey_owned_server {
                mcp_servers.remove(CODEY_FASTCTX_SERVER_ID);
            }
            codey_owned_server
        }
        Some(Item::Value(Value::InlineTable(mcp_servers))) => {
            let codey_owned_server = mcp_servers
                .get(CODEY_FASTCTX_SERVER_ID)
                .is_some_and(fastctx_value_server_is_codey_owned);
            if codey_owned_server {
                mcp_servers.remove(CODEY_FASTCTX_SERVER_ID);
            }
            codey_owned_server
        }
        _ => false,
    };

    let codey_guidance_removed = remove_guidance_from_table(
        doc.as_table_mut(),
        "developer_instructions",
        remove_codey_fastctx_guidance,
    );
    let subagent_guidance_removed = doc
        .get_mut("features")
        .and_then(Item::as_table_like_mut)
        .and_then(|features| features.get_mut("multi_agent_v2"))
        .and_then(Item::as_table_like_mut)
        .is_some_and(|multi_agent| {
            remove_guidance_from_table(
                multi_agent,
                "subagent_developer_instructions",
                remove_codey_fastctx_guidance,
            )
        });

    let reserved_server_remains = mcp_server_exists(doc, CODEY_FASTCTX_SERVER_ID);
    if (codey_owned_server_removed || codey_guidance_removed || subagent_guidance_removed)
        && !reserved_server_remains
    {
        remove_direct_only_tool_namespace(doc, CODEY_FASTCTX_NAMESPACE);
    }
}

fn remove_direct_only_tool_namespace(doc: &mut DocumentMut, namespace: &str) -> bool {
    let Some(namespaces) = direct_only_tool_namespaces_mut(doc) else {
        return false;
    };
    let original_len = namespaces.len();
    namespaces.retain(|entry| entry.as_str() != Some(namespace));
    namespaces.len() != original_len
}

fn ensure_direct_only_tool_namespace(doc: &mut DocumentMut, namespace: &str) -> Result<()> {
    if matches!(
        doc.get("features"),
        Some(Item::Value(Value::InlineTable(_)))
    ) {
        let normalized = item_table_clone(doc.get("features").expect("checked features item"))
            .expect("inline features can be normalized");
        doc.as_table_mut()
            .insert("features", Item::Table(normalized));
    }
    let features = ensure_root_table(doc, "features")?;
    if matches!(
        features.get("code_mode"),
        Some(Item::Value(Value::InlineTable(_)))
    ) {
        let normalized =
            item_table_clone(features.get("code_mode").expect("checked code_mode item"))
                .expect("inline code_mode can be normalized");
        features.insert("code_mode", Item::Table(normalized));
    }
    let code_mode = ensure_child_table(features, "code_mode")?;
    if code_mode.get("direct_only_tool_namespaces").is_none() {
        code_mode.insert(
            "direct_only_tool_namespaces",
            Item::Value(Value::Array(Array::new())),
        );
    }
    let namespaces = code_mode
        .get_mut("direct_only_tool_namespaces")
        .and_then(Item::as_array_mut)
        .ok_or_else(|| {
            anyhow::anyhow!("features.code_mode.direct_only_tool_namespaces 必须是 TOML array")
        })?;
    let mut kept = false;
    namespaces.retain(|entry| {
        if entry.as_str() != Some(namespace) {
            return true;
        }
        if kept {
            false
        } else {
            kept = true;
            true
        }
    });
    if !kept {
        namespaces.push(namespace);
    }
    Ok(())
}

pub(super) fn direct_only_tool_namespaces(doc: &DocumentMut) -> Option<&Array> {
    doc.get("features")
        .and_then(Item::as_table_like)
        .and_then(|features| features.get("code_mode"))
        .and_then(Item::as_table_like)
        .and_then(|code_mode| code_mode.get("direct_only_tool_namespaces"))
        .and_then(Item::as_array)
}

pub(super) fn direct_only_tool_namespaces_mut(doc: &mut DocumentMut) -> Option<&mut Array> {
    doc.get_mut("features")
        .and_then(Item::as_table_like_mut)
        .and_then(|features| features.get_mut("code_mode"))
        .and_then(Item::as_table_like_mut)
        .and_then(|code_mode| code_mode.get_mut("direct_only_tool_namespaces"))
        .and_then(Item::as_array_mut)
}

pub(super) fn remove_guidance_from_table(
    table: &mut dyn TableLike,
    key: &str,
    remove_guidance: fn(&str) -> Option<String>,
) -> bool {
    let restored_guidance = table
        .get(key)
        .and_then(Item::as_str)
        .and_then(remove_guidance);
    let Some(restored_guidance) = restored_guidance else {
        return false;
    };
    if restored_guidance.trim().is_empty() {
        table.remove(key);
    } else {
        table.insert(key, value(restored_guidance));
    }
    true
}

pub(super) fn fast_context_tools_status_from_document(doc: &DocumentMut) -> FastContextToolsStatus {
    let server_id = configured_user_fastctx_server_id(doc);
    FastContextToolsStatus {
        user_configured: server_id.is_some(),
        detection_failed: false,
        server_id,
    }
}

pub(super) fn configured_user_fastctx_server_id(doc: &DocumentMut) -> Option<String> {
    match doc.get("mcp_servers")? {
        Item::Table(mcp_servers) => mcp_servers.iter().find_map(|(server_id, server)| {
            (mcp_server_mentions_fastctx(server_id, server)
                && !fastctx_item_server_is_codey_owned(server))
            .then(|| server_id.to_string())
        }),
        Item::Value(Value::InlineTable(mcp_servers)) => {
            mcp_servers.iter().find_map(|(server_id, server)| {
                (mcp_server_value_mentions_fastctx(server_id, server)
                    && !fastctx_value_server_is_codey_owned(server))
                .then(|| server_id.to_string())
            })
        }
        _ => None,
    }
}

pub(super) fn arguments_have_codey_fastctx_marker(arguments: &Array) -> bool {
    arguments
        .iter()
        .any(|argument| argument.as_str() == Some(CODEY_FASTCTX_ARG_MARKER))
}

fn fastctx_item_server_is_codey_owned(server: &Item) -> bool {
    server
        .as_table()
        .is_some_and(fastctx_table_server_is_codey_owned)
        || matches!(server, Item::Value(value) if fastctx_value_server_is_codey_owned(value))
}

fn fastctx_value_server_is_codey_owned(server: &Value) -> bool {
    matches!(
        server,
        Value::InlineTable(server)
            if server
                .get("args")
                .and_then(Value::as_array)
                .is_some_and(arguments_have_codey_fastctx_marker)
    )
}

fn mcp_server_is_codey_owned_by_id(doc: &DocumentMut, server_id: &str) -> bool {
    match doc.get("mcp_servers") {
        Some(Item::Table(mcp_servers)) => mcp_servers
            .get(server_id)
            .is_some_and(fastctx_item_server_is_codey_owned),
        Some(Item::Value(Value::InlineTable(mcp_servers))) => mcp_servers
            .get(server_id)
            .is_some_and(fastctx_value_server_is_codey_owned),
        _ => false,
    }
}

pub(super) fn mcp_server_exists(doc: &DocumentMut, server_id: &str) -> bool {
    match doc.get("mcp_servers") {
        Some(Item::Table(mcp_servers)) => mcp_servers.contains_key(server_id),
        Some(Item::Value(Value::InlineTable(mcp_servers))) => mcp_servers.contains_key(server_id),
        _ => false,
    }
}

fn ensure_mcp_servers_table(doc: &mut DocumentMut) -> Result<&mut Table> {
    let inline_table = match doc.get("mcp_servers") {
        Some(Item::Value(Value::InlineTable(mcp_servers))) => Some(mcp_servers.clone()),
        _ => None,
    };
    if let Some(inline_table) = inline_table {
        let mut table = Table::new();
        for (server_id, server) in inline_table.iter() {
            table.insert(server_id, Item::Value(server.clone()));
        }
        doc.as_table_mut().insert("mcp_servers", Item::Table(table));
    }
    ensure_root_table(doc, "mcp_servers")
}

fn item_table_clone(item: &Item) -> Option<Table> {
    match item {
        Item::Table(table) => Some(table.clone()),
        Item::Value(Value::InlineTable(inline_table)) => {
            let mut table = Table::new();
            for (key, value) in inline_table.iter() {
                table.insert(key, Item::Value(value.clone()));
            }
            Some(table)
        }
        _ => None,
    }
}

fn mcp_server_mentions_fastctx(server_id: &str, server: &Item) -> bool {
    mentions_fastctx(server_id)
        || server.as_table().is_some_and(|server| {
            mcp_server_fields_mention_fastctx(
                server.get("command").and_then(Item::as_str),
                server.get("args").and_then(Item::as_array),
            )
        })
        || matches!(server, Item::Value(value) if mcp_server_value_fields_mention_fastctx(value))
}

fn mcp_server_value_mentions_fastctx(server_id: &str, server: &Value) -> bool {
    mentions_fastctx(server_id) || mcp_server_value_fields_mention_fastctx(server)
}

fn mcp_server_value_fields_mention_fastctx(server: &Value) -> bool {
    matches!(
        server,
        Value::InlineTable(server)
            if mcp_server_fields_mention_fastctx(
                server.get("command").and_then(Value::as_str),
                server.get("args").and_then(Value::as_array),
            )
    )
}

fn mcp_server_fields_mention_fastctx(command: Option<&str>, arguments: Option<&Array>) -> bool {
    command.is_some_and(mentions_fastctx)
        || arguments.is_some_and(|arguments| {
            arguments
                .iter()
                .filter_map(Value::as_str)
                .any(mentions_fastctx)
        })
}

fn mentions_fastctx(value: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|part| part.eq_ignore_ascii_case("fastctx"))
}
