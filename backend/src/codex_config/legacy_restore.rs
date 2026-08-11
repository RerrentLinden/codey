use super::*;

pub(super) fn restore_legacy_owned_config_changes(
    original: &str,
    current: &str,
    provider_id: &str,
) -> Result<String> {
    let original_document = parse_document(original).context("解析旧版 Codex 原配置备份失败")?;
    let current_document = parse_document(current).context("解析旧版 Codex 当前配置失败")?;
    let mut applied_document =
        parse_document(original).context("准备旧版 Codey 配置恢复基线失败")?;

    if current_document
        .get("model_provider")
        .and_then(Item::as_str)
        == Some(provider_id)
        && let Some(item) = current_document.get("model_provider")
    {
        applied_document
            .as_table_mut()
            .insert("model_provider", item.clone());
    }

    if let Some(current_provider) = current_document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table)
    {
        let applied_provider = table_with_selected_fields(
            current_provider,
            &[
                "name",
                "base_url",
                "wire_api",
                "requires_openai_auth",
                "experimental_bearer_token",
            ],
        );
        ensure_root_table(&mut applied_document, "model_providers")?
            .insert(provider_id, Item::Table(applied_provider));
    }

    match current_document.get("model_catalog_json") {
        Some(item) if item.as_str() == Some(crate::model_catalog::relative_path()) => {
            applied_document
                .as_table_mut()
                .insert("model_catalog_json", item.clone());
        }
        None if original_document.get("model_catalog_json").is_some() => {
            applied_document.as_table_mut().remove("model_catalog_json");
        }
        _ => {}
    }

    if original_document.get("model").is_some() && current_document.get("model").is_none() {
        applied_document.as_table_mut().remove("model");
    }
    remove_legacy_active_profile_model(
        &original_document,
        &current_document,
        &mut applied_document,
    );

    if let Some(efforts) = current_document
        .get("desktop")
        .and_then(Item::as_table)
        .and_then(|desktop| desktop.get("enabled-reasoning-efforts"))
        .filter(|item| is_legacy_reasoning_efforts(item))
    {
        ensure_root_table(&mut applied_document, "desktop")?
            .insert("enabled-reasoning-efforts", efforts.clone());
    }

    if let Some(server) = current_document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .and_then(|servers| servers.get(CODEY_FASTCTX_SERVER_ID))
        .and_then(Item::as_table)
        .filter(|server| fastctx_table_server_is_codey_owned(server))
    {
        let mut applied_server = table_with_selected_fields(
            server,
            &["command", "args", "startup_timeout_sec", "tool_timeout_sec"],
        );
        if let Some(environment) = server.get("env").and_then(Item::as_table) {
            let applied_environment =
                table_with_selected_fields(environment, &["FASTCTX_TOKEN_BUDGET"]);
            if !applied_environment.is_empty() {
                applied_server.insert("env", Item::Table(applied_environment));
            }
        }
        ensure_root_table(&mut applied_document, "mcp_servers")?
            .insert(CODEY_FASTCTX_SERVER_ID, Item::Table(applied_server));
    }

    let original_namespaces = fastctx_namespaces(&original_document);
    let current_namespaces = fastctx_namespaces(&current_document);
    let original_has_fastctx = original_namespaces.is_some_and(|namespaces| {
        namespaces
            .iter()
            .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
    });
    let current_has_fastctx = current_namespaces.is_some_and(|namespaces| {
        namespaces
            .iter()
            .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
    });
    if !original_has_fastctx && current_has_fastctx {
        let mut applied_namespaces = original_namespaces.cloned().unwrap_or_else(Array::new);
        applied_namespaces.push(CODEY_FASTCTX_NAMESPACE);
        let features = ensure_root_table(&mut applied_document, "features")?;
        let code_mode = ensure_child_table(features, "code_mode")?;
        code_mode.insert(
            "direct_only_tool_namespaces",
            Item::Value(Value::Array(applied_namespaces)),
        );
    }

    if original_document.get("tool_output_token_limit").is_none()
        && current_document
            .get("tool_output_token_limit")
            .and_then(Item::as_integer)
            == Some(10_000)
        && let Some(item) = current_document.get("tool_output_token_limit")
    {
        applied_document
            .as_table_mut()
            .insert("tool_output_token_limit", item.clone());
    }

    let original_guidance = original_document
        .get("developer_instructions")
        .and_then(Item::as_str)
        .unwrap_or_default();
    let current_guidance = current_document
        .get("developer_instructions")
        .and_then(Item::as_str)
        .unwrap_or_default();
    let mut applied_guidance = original_guidance.to_string();
    let mut fastctx_guidance_was_applied = false;
    for guidance in codey_fastctx_guidance_blocks(current_guidance) {
        if original_guidance.contains(&guidance) {
            continue;
        }
        if applied_guidance.trim().is_empty() {
            applied_guidance = guidance;
        } else {
            applied_guidance.push_str("\n\n");
            applied_guidance.push_str(&guidance);
        }
        fastctx_guidance_was_applied = true;
    }
    if fastctx_guidance_was_applied {
        applied_document["developer_instructions"] = value(applied_guidance);
    }

    let applied = document_string(&applied_document)?;
    restore_owned_config_changes(original, &applied, current)
}

fn remove_legacy_active_profile_model(
    original: &DocumentMut,
    current: &DocumentMut,
    applied: &mut DocumentMut,
) {
    let Some(active_profile) = original.get("profile").and_then(Item::as_str) else {
        return;
    };
    let original_has_model = original
        .get("profiles")
        .and_then(Item::as_table)
        .and_then(|profiles| profiles.get(active_profile))
        .and_then(Item::as_table)
        .is_some_and(|profile| profile.get("model").is_some());
    let current_profile = current
        .get("profiles")
        .and_then(Item::as_table)
        .and_then(|profiles| profiles.get(active_profile))
        .and_then(Item::as_table);
    if !original_has_model || current_profile.is_none_or(|profile| profile.get("model").is_some()) {
        return;
    }
    if let Some(applied_profile) = applied
        .get_mut("profiles")
        .and_then(Item::as_table_mut)
        .and_then(|profiles| profiles.get_mut(active_profile))
        .and_then(Item::as_table_mut)
    {
        applied_profile.remove("model");
    }
}

fn table_with_selected_fields(source: &Table, fields: &[&str]) -> Table {
    let mut selected = Table::new();
    for field in fields {
        if let Some(item) = source.get(field) {
            selected.insert(field, item.clone());
        }
    }
    selected
}

fn fastctx_namespaces(document: &DocumentMut) -> Option<&Array> {
    document
        .get("features")
        .and_then(Item::as_table)
        .and_then(|features| features.get("code_mode"))
        .and_then(Item::as_table)
        .and_then(|code_mode| code_mode.get("direct_only_tool_namespaces"))
        .and_then(Item::as_array)
}

fn is_legacy_reasoning_efforts(item: &Item) -> bool {
    const LEGACY_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
    item.as_array().is_some_and(|efforts| {
        efforts.len() == LEGACY_EFFORTS.len()
            && efforts
                .iter()
                .zip(LEGACY_EFFORTS)
                .all(|(actual, expected)| actual.as_str() == Some(expected))
    })
}
