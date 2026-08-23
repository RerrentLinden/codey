use anyhow::{Context, Result, bail};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use super::ensure_root_table;
use super::fastctx::{ensure_direct_only_tool_namespace, remove_direct_only_tool_namespace};

pub(super) fn enable_subagent_control_mcp(doc: &mut DocumentMut) -> Result<()> {
    let executable = std::env::current_exe().context("定位 Codey 子代理批次控制 MCP 程序失败")?;
    let server_id = crate::subagent_control_mcp::SERVER_ID;
    let existing_owned = doc
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .and_then(|servers| servers.get(server_id))
        .is_some_and(server_is_codey_owned);
    if doc
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .and_then(|servers| servers.get(server_id))
        .is_some()
        && !existing_owned
    {
        bail!("mcp_servers.{server_id} 已被非 Codey 配置占用，无法启用子代理批次决策工具");
    }

    let servers = ensure_root_table(doc, "mcp_servers")?;
    let mut server = if existing_owned {
        servers
            .get(server_id)
            .and_then(item_table_clone)
            .unwrap_or_default()
    } else {
        Table::new()
    };
    server["command"] = value(executable.to_string_lossy().into_owned());
    let mut args = Array::new();
    args.push(crate::subagent_control_mcp::ARGUMENT);
    server["args"] = Item::Value(Value::Array(args));
    server["startup_timeout_sec"] = value(crate::subagent_control_mcp::STARTUP_TIMEOUT_SECONDS);
    server["tool_timeout_sec"] = value(crate::subagent_control_mcp::TOOL_TIMEOUT_SECONDS);
    server.remove("default_tools_approval_mode");
    let mut enabled_tools = Array::new();
    enabled_tools.push(crate::subagent_control_mcp::RESOLVE_BATCH_TOOL_NAME);
    enabled_tools.push(crate::subagent_control_mcp::PREPARE_DELEGATION_TOOL_NAME);
    server["enabled_tools"] = Item::Value(Value::Array(enabled_tools));
    server["disabled_tools"] = Item::Value(Value::Array(Array::new()));

    let mut tools = server
        .get("tools")
        .and_then(item_table_clone)
        .unwrap_or_default();
    for tool_name in [
        crate::subagent_control_mcp::RESOLVE_BATCH_TOOL_NAME,
        crate::subagent_control_mcp::PREPARE_DELEGATION_TOOL_NAME,
    ] {
        let mut tool = tools
            .get(tool_name)
            .and_then(item_table_clone)
            .unwrap_or_default();
        tool["approval_mode"] = value("approve");
        tools.insert(tool_name, Item::Table(tool));
    }
    server["tools"] = Item::Table(tools);
    servers.insert(server_id, Item::Table(server));
    ensure_direct_only_tool_namespace(doc, crate::subagent_control_mcp::NAMESPACE)
}

pub(super) fn disable_subagent_control_mcp(doc: &mut DocumentMut) {
    let server_id = crate::subagent_control_mcp::SERVER_ID;
    if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut)
        && servers.get(server_id).is_some_and(server_is_codey_owned)
    {
        servers.remove(server_id);
    }
    let server_still_exists = doc
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .is_some_and(|servers| servers.get(server_id).is_some());
    if !server_still_exists {
        remove_direct_only_tool_namespace(doc, crate::subagent_control_mcp::NAMESPACE);
    }
}

fn server_is_codey_owned(item: &Item) -> bool {
    match item {
        Item::Table(table) => table
            .get("args")
            .and_then(Item::as_array)
            .is_some_and(args_have_marker),
        Item::Value(Value::InlineTable(table)) => table
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(args_have_marker),
        _ => false,
    }
}

fn args_have_marker(args: &Array) -> bool {
    args.iter()
        .any(|argument| argument.as_str() == Some(crate::subagent_control_mcp::ARGUMENT))
}

fn item_table_clone(item: &Item) -> Option<Table> {
    match item {
        Item::Table(table) => Some(table.clone()),
        Item::Value(Value::InlineTable(table)) => {
            let mut normalized = Table::new();
            for (key, value) in table.iter() {
                normalized.insert(key, Item::Value(value.clone()));
            }
            Some(normalized)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_removes_owned_server() {
        let mut document = DocumentMut::new();
        enable_subagent_control_mcp(&mut document).unwrap();
        assert_eq!(
            document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID]["args"]
                .as_array()
                .and_then(|args| args.get(0))
                .and_then(Value::as_str),
            Some(crate::subagent_control_mcp::ARGUMENT)
        );
        assert_eq!(
            document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID]["enabled_tools"]
                .as_array()
                .and_then(|tools| tools.get(0))
                .and_then(Value::as_str),
            Some(crate::subagent_control_mcp::RESOLVE_BATCH_TOOL_NAME)
        );
        assert_eq!(
            document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID]["enabled_tools"]
                .as_array()
                .map(Array::len),
            Some(2)
        );
        assert_eq!(
            document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID]["disabled_tools"]
                .as_array()
                .map(Array::len),
            Some(0)
        );
        assert_eq!(
            document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID]["tools"]
                [crate::subagent_control_mcp::RESOLVE_BATCH_TOOL_NAME]["approval_mode"]
                .as_str(),
            Some("approve")
        );
        assert_eq!(
            document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID]["tools"]
                [crate::subagent_control_mcp::PREPARE_DELEGATION_TOOL_NAME]["approval_mode"]
                .as_str(),
            Some("approve")
        );
        disable_subagent_control_mcp(&mut document);
        assert!(
            document["mcp_servers"].as_table().is_some_and(
                |servers| !servers.contains_key(crate::subagent_control_mcp::SERVER_ID)
            )
        );
    }

    #[test]
    fn preserves_non_codey_server_with_reserved_id() {
        let mut document =
            "[mcp_servers.codey_subagent_control]\ncommand = 'custom'\nargs = ['--custom']\n"
                .parse::<DocumentMut>()
                .unwrap();
        assert!(enable_subagent_control_mcp(&mut document).is_err());
        disable_subagent_control_mcp(&mut document);
        assert_eq!(
            document["mcp_servers"]["codey_subagent_control"]["command"].as_str(),
            Some("custom")
        );
    }

    #[test]
    fn repairs_owned_server_tool_policy_without_widening_the_allow_list() {
        let mut document = format!(
            "[mcp_servers.codey_subagent_control]\ncommand = 'old'\nargs = ['{}']\nenabled_tools = ['stale']\ndisabled_tools = ['resolve_batch']\ndefault_tools_approval_mode = 'approve'\n\n[mcp_servers.codey_subagent_control.tools.resolve_batch]\napproval_mode = 'prompt'\n",
            crate::subagent_control_mcp::ARGUMENT
        )
        .parse::<DocumentMut>()
        .unwrap();

        enable_subagent_control_mcp(&mut document).unwrap();

        let server = &document["mcp_servers"][crate::subagent_control_mcp::SERVER_ID];
        let enabled = server["enabled_tools"].as_array().unwrap();
        assert_eq!(enabled.len(), 2);
        assert_eq!(
            enabled.get(0).and_then(Value::as_str),
            Some(crate::subagent_control_mcp::RESOLVE_BATCH_TOOL_NAME)
        );
        assert_eq!(
            enabled.get(1).and_then(Value::as_str),
            Some(crate::subagent_control_mcp::PREPARE_DELEGATION_TOOL_NAME)
        );
        assert_eq!(server["disabled_tools"].as_array().unwrap().len(), 0);
        assert_eq!(
            server["tools"][crate::subagent_control_mcp::RESOLVE_BATCH_TOOL_NAME]["approval_mode"]
                .as_str(),
            Some("approve")
        );
        assert_eq!(
            server["tools"][crate::subagent_control_mcp::PREPARE_DELEGATION_TOOL_NAME]
                ["approval_mode"]
                .as_str(),
            Some("approve")
        );
        assert!(server.get("default_tools_approval_mode").is_none());
    }
}
