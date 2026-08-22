//! Transaction boundary for applying the temporary Codex runtime configuration.
//!
//! The lease, generated role files, config snapshot, runtime config, and policy
//! are committed and compensated in the original order. Helper implementations
//! remain in the parent module so this file contains only transaction control.

use super::*;

pub(super) fn apply_runtime_provider_config_at_mode(
    home: &Path,
    profile: &ProviderProfile,
    provider_id: &str,
    options: ProviderApplyOptions<'_>,
) -> Result<PathBuf> {
    let ProviderApplyOptions {
        use_official_catalog,
        default_model,
        fastctx_command,
        subagent_optimization,
        subagent_model,
        subagent_reasoning_effort,
        subagent_roles,
        marker,
        backup_root,
        preserve_provider_route,
        protocol_proxy_base_url,
        expected_config,
    } = options;
    ensure_supported_provider_protocol(profile.protocol, protocol_proxy_base_url)?;
    fs::create_dir_all(home)?;
    let config_path = home.join("config.toml");
    let agents_md_path = home.join("AGENTS.md");
    let agents_dir = home.join("agents");
    let default_agent_path = agents_dir.join("default.toml");
    let original_config_on_disk = read_codex_config(&config_path)?;
    if let Some(expected_config) = expected_config
        && original_config_on_disk.as_deref() != Some(expected_config)
    {
        bail!("CC Switch Live 配置在启动准备期间发生变化；已取消本次启动以避免混用线路");
    }
    let original_agents_dir_exists = agents_dir.is_dir();
    let original_config =
        persist_embedded_config_prompt_migration(&config_path, original_config_on_disk)?;
    let original_agents_md = if subagent_optimization {
        persist_embedded_subagent_guidance_migration(
            &agents_md_path,
            read_optional(&agents_md_path)?,
        )?
    } else {
        None
    };
    let original_default_agent = if subagent_optimization {
        let migrated_default_agent = persist_previous_fastctx_guidance_migration(
            &default_agent_path,
            read_optional(&default_agent_path)?,
            false,
            "Codex agents/default.toml",
        )?;
        persist_legacy_default_agent_migration(&default_agent_path, migrated_default_agent)?
    } else {
        None
    };
    create_private_dir_all(backup_root)?;
    prune_stale_backup_dirs(backup_root, marker);
    let backup_dir = backup_root.join(format!("{}-{}", timestamp_millis(), std::process::id()));
    create_private_dir_all(&backup_dir)?;
    if let Some(bytes) = original_config.as_deref() {
        write_private_file(&backup_dir.join("config.toml"), bytes)?;
    }

    let existing = str::from_utf8(original_config.as_deref().unwrap_or_default())
        .context("Codex config.toml 不是 UTF-8")?;
    let provider_id = validated_provider_id(provider_id)?;
    // Codex resolves this path from the app-server working directory, which is
    // `/` for the packaged macOS app, rather than from CODEX_HOME.
    let model_catalog_path =
        use_official_catalog.then(|| home.join(crate::model_catalog::relative_path()));
    let mut updated = patch_config_with_fastctx_mode_and_proxy(
        existing,
        profile,
        &provider_id,
        ProviderPatchOptions {
            config_path: &config_path,
            model_catalog_path: model_catalog_path.as_deref(),
            default_model,
            fastctx_command,
            subagent_optimization,
            subagent_model,
            subagent_reasoning_effort,
            preserve_provider_route,
            protocol_proxy_base_url,
        },
    )?;
    let mut updated_document = parse_document(&updated).context("解析已应用 Codex 临时配置失败")?;
    let fastctx_namespace = updated_document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .and_then(|servers| servers.get(CODEY_FASTCTX_SERVER_ID))
        .and_then(Item::as_table)
        .is_some_and(fastctx_table_server_is_codey_owned)
        .then_some(CODEY_FASTCTX_NAMESPACE);
    let constraints_dir = marker.with_file_name(CODEY_CONSTRAINTS_DIR);
    if fastctx_namespace.is_some() || subagent_optimization {
        create_private_dir_all(&constraints_dir)?;
    }
    let fastctx_instructions = if fastctx_namespace.is_some() {
        Some(read_or_create_constraint_file_with_exact_migration(
            &constraints_dir.join(CODEY_FASTCTX_INSTRUCTIONS_FILE),
            CODEY_FASTCTX_GUIDANCE,
            &CODEY_FASTCTX_GUIDANCE_VERSIONS[1..],
        )?)
    } else {
        None
    };
    let runtime_roles =
        runtime_subagent_roles(subagent_roles, subagent_model, subagent_reasoning_effort);
    let runtime_agents = if subagent_optimization {
        read_or_create_constraint_file_with_exact_migration(
            &constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE),
            SUBAGENT_GUIDANCE,
            &SUBAGENT_GUIDANCE_VERSIONS[1..],
        )?;
        read_or_create_constraint_file_with_exact_migration(
            &constraints_dir.join(CODEY_COLLABORATION_HINT_FILE),
            ROOT_AGENT_COLLABORATION_USAGE_HINT,
            &ROOT_AGENT_COLLABORATION_USAGE_HINT_VERSIONS[1..],
        )?;
        let runtime_agents = prepare_runtime_agent_files(
            &constraints_dir,
            &runtime_roles,
            fastctx_instructions.as_deref(),
        )?;
        register_runtime_agents(&mut updated_document, &runtime_agents)?;
        runtime_agents
    } else {
        Vec::new()
    };
    remove_embedded_codey_prompt_sources(&mut updated_document);
    updated = document_string(&updated_document)?;
    let applied_base_url = provider_base_url(&updated, &provider_id);
    if let Err(error) =
        write_private_file(&backup_dir.join(APPLIED_CONFIG_FILE), updated.as_bytes())
    {
        let _ = fs::remove_dir_all(&backup_dir);
        return Err(error).context("保存 Codey 已应用配置快照失败");
    }
    let runtime_agent_hashes = runtime_agent_hashes(&runtime_agents);
    let state = RuntimeConfigLease {
        backup_dir: backup_dir.clone(),
        config_snapshot_dir: None,
        original_config_exists: original_config.is_some(),
        preserve_provider_route,
        protocol_proxy_base_url: protocol_proxy_base_url.map(str::to_string),
        fastctx_command: fastctx_command.map(Path::to_path_buf),
        subagent_optimization_applied: subagent_optimization,
        subagent_model: subagent_model.to_string(),
        subagent_reasoning_effort: subagent_reasoning_effort.to_string(),
        subagent_roles: runtime_roles,
        runtime_home: home.to_path_buf(),
        runtime_agent_schema_version: if subagent_optimization {
            RUNTIME_AGENT_SCHEMA_VERSION
        } else {
            0
        },
        runtime_agent_hashes,
        original_agents_md_exists: original_agents_md.is_some(),
        original_default_agent_exists: original_default_agent.is_some(),
        original_agents_dir_exists,
        provider_id: Some(provider_id),
        applied_base_url,
        isolated_runtime_constraints: false,
        independent_prompt_sources: true,
        runtime_hooks_applied: false,
        original_hooks_file_exists: false,
    };
    if let Err(error) = write_lease(marker, &state) {
        let _ = fs::remove_dir_all(&backup_dir);
        return Err(error);
    }

    let inputs_unchanged = codex_config_matches(&config_path, original_config.as_deref())?
        && (!subagent_optimization
            || (optional_file_matches(&agents_md_path, original_agents_md.as_deref())?
                && optional_file_matches(&default_agent_path, original_default_agent.as_deref())?));
    if !inputs_unchanged {
        discard_runtime_lease(home, marker, &backup_dir).with_context(|| {
            "Codex 配置在 Codey 保存运行时快照后发生变化；取消启动时清理租约失败，恢复备份已保留"
        })?;
        bail!("Codex 配置在 Codey 保存运行时快照后发生变化；已取消本次启动");
    }

    let write_result = write_codex_config(
        &config_path,
        original_config.as_deref(),
        updated.as_bytes(),
        if preserve_provider_route {
            "apply route-mode runtime overlay without replacing route data"
        } else {
            "apply non-route provider runtime configuration"
        },
        "codex_config.apply_runtime_provider_config_at_mode",
    );
    if let Err(write_error) = write_result {
        match restore_runtime_provider_config_at(home, marker) {
            Ok(_) => {
                let _ = fs::remove_dir_all(&backup_dir);
                return Err(write_error);
            }
            Err(rollback_error) => {
                anyhow::bail!(
                    "写入 Codey 临时 Codex 配置失败：{write_error}；按租约恢复原配置也失败：{rollback_error:#}"
                );
            }
        }
    }

    let policy_result = if subagent_optimization {
        crate::subagent_gate::write_runtime_subagent_policy(
            home,
            &state.subagent_roles,
            &state.runtime_agent_hashes,
        )
    } else {
        crate::subagent_gate::clear_runtime_subagent_policy(home)
    };
    if let Err(policy_error) = policy_result {
        match restore_runtime_provider_config_at(home, marker) {
            Ok(_) => {
                let _ = fs::remove_dir_all(&backup_dir);
                return Err(policy_error)
                    .context("提交 Codey 子代理运行时策略失败，已恢复启动前配置");
            }
            Err(rollback_error) => {
                anyhow::bail!(
                    "提交 Codey 子代理运行时策略失败：{policy_error:#}；恢复启动前配置也失败：{rollback_error:#}"
                );
            }
        }
    }
    Ok(backup_dir)
}
