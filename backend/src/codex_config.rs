use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use codey_runtime_core::settings::RelayProtocol;
use serde::{Deserialize, Serialize};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

#[cfg(test)]
use crate::codex_config_guidance::{
    CODEY_FASTCTX_GUIDANCE_VERSIONS, PREVIOUS_ROOT_AGENT_COLLABORATION_USAGE_HINT,
    ROOT_AGENT_COLLABORATION_USAGE_HINT,
};
use crate::codex_config_guidance::{
    SUBAGENT_GUIDANCE, append_root_agent_collaboration_usage_hint, append_subagent_guidance,
    codey_fastctx_guidance_blocks, default_agent_config_with_fastctx_guidance,
    remove_previous_codey_fastctx_guidance, remove_subagent_guidance,
};
#[cfg(test)]
use crate::config::{DEFAULT_SUBAGENT_MODEL, DEFAULT_SUBAGENT_REASONING_EFFORT};
use crate::config::{ProviderProfile, SUBAGENT_REASONING_EFFORTS, default_config_path};
use crate::fs_util::timestamp_millis;
use crate::provider_lease::CODEY_PROVIDER_ID;

mod fastctx;
mod fs_io;
mod legacy_restore;
mod toml_restore;

use fastctx::{
    apply_fastctx_guidance_to_table, arguments_have_codey_fastctx_marker,
    direct_only_tool_namespaces, direct_only_tool_namespaces_mut, disable_fast_context_tools,
    enable_fast_context_tools, fast_context_tools_status_from_document, remove_guidance_from_table,
};
#[cfg(test)]
use fastctx::{configured_user_fastctx_server_id, mcp_server_exists};
use fs_io::{
    atomic_write, create_private_dir_all, read_optional, remove_optional, write_private_file,
};
use legacy_restore::restore_legacy_owned_config_changes;
#[cfg(test)]
use toml_restore::{items_semantically_equal, tables_semantically_equal};
use toml_restore::{restore_owned_config_changes, restore_owned_model_provider_changes};

pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const BUILTIN_OPENAI_PROVIDER_ID: &str = "openai";
const OPENAI_PROVIDER_NAME: &str = "OpenAI";
const CODEY_FASTCTX_SERVER_ID: &str = "codey_fastctx";
const CODEY_FASTCTX_NAMESPACE: &str = "mcp__codey_fastctx";
const CODEY_FASTCTX_ARG_MARKER: &str = "--codey-fastctx-mcp";
const CODEY_FASTCTX_TOKEN_BUDGET: &str = "8500";
const CODEY_FASTCTX_STARTUP_TIMEOUT_SECONDS: i64 = 15;
const SUBAGENT_DEFAULTS_VERIFY_TIMEOUT_MS: u64 = 1_500;
const SUBAGENT_DEFAULTS_VERIFY_POLL_MS: u64 = 75;
const APPLIED_CONFIG_FILE: &str = "applied-config.toml";
const APPLIED_AGENTS_MD_FILE: &str = "applied-AGENTS.md";
const APPLIED_DEFAULT_AGENT_FILE: &str = "agents/applied-default.toml";
const RESERVED_PROVIDER_IDS: [&str; 6] = [
    "amazon-bedrock",
    "openai",
    "ollama",
    "lmstudio",
    "oss",
    "ollama-chat",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeConfigLease {
    backup_dir: PathBuf,
    // Older route-overlay leases may point at a rebased snapshot. Current
    // runtimes restart on route changes, but still need to restore those leases.
    #[serde(default)]
    config_snapshot_dir: Option<PathBuf>,
    original_config_exists: bool,
    #[serde(default)]
    preserve_provider_route: bool,
    #[serde(default)]
    protocol_proxy_base_url: Option<String>,
    #[serde(default)]
    fastctx_command: Option<PathBuf>,
    #[serde(default)]
    subagent_optimization_applied: bool,
    #[serde(default)]
    subagent_model: String,
    #[serde(default)]
    subagent_reasoning_effort: String,
    #[serde(default)]
    original_agents_md_exists: bool,
    #[serde(default)]
    original_default_agent_exists: bool,
    #[serde(default)]
    original_agents_dir_exists: bool,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    applied_base_url: Option<String>,
}

pub fn codex_home() -> PathBuf {
    codey_runtime_core::relay_config::default_codex_home_dir()
}

fn lease_marker_path() -> PathBuf {
    default_config_path()
        .parent()
        .unwrap_or_else(|| Path::new(".codey"))
        .join("codex-lease.json")
}

pub(crate) struct RuntimeProviderConfigOptions<'a> {
    pub use_official_catalog: bool,
    pub default_model: Option<&'a str>,
    pub fast_context_tools: bool,
    pub subagent_optimization: bool,
    pub subagent_model: &'a str,
    pub subagent_reasoning_effort: &'a str,
    pub preserve_provider_route: bool,
    pub protocol_proxy_base_url: Option<&'a str>,
    pub expected_config: Option<&'a [u8]>,
}

pub(crate) struct AppliedRuntimeProviderConfig {
    pub config_contents: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FastContextToolsStatus {
    pub user_configured: bool,
    pub detection_failed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

struct ProviderApplyOptions<'a> {
    use_official_catalog: bool,
    default_model: Option<&'a str>,
    fastctx_command: Option<&'a Path>,
    subagent_optimization: bool,
    subagent_model: &'a str,
    subagent_reasoning_effort: &'a str,
    marker: &'a Path,
    backup_root: &'a Path,
    preserve_provider_route: bool,
    protocol_proxy_base_url: Option<&'a str>,
    expected_config: Option<&'a [u8]>,
}

#[cfg(test)]
impl<'a> ProviderApplyOptions<'a> {
    fn for_test(marker: &'a Path, backup_root: &'a Path) -> Self {
        Self {
            use_official_catalog: true,
            default_model: None,
            fastctx_command: None,
            subagent_optimization: false,
            subagent_model: DEFAULT_SUBAGENT_MODEL,
            subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
            marker,
            backup_root,
            preserve_provider_route: false,
            protocol_proxy_base_url: None,
            expected_config: None,
        }
    }
}

pub(crate) fn apply_runtime_provider_config(
    home: &Path,
    profile: &ProviderProfile,
    provider_id: &str,
    options: RuntimeProviderConfigOptions<'_>,
) -> Result<AppliedRuntimeProviderConfig> {
    let marker = lease_marker_path();
    let backup_root = marker
        .parent()
        .unwrap_or_else(|| Path::new(".codey"))
        .join("codex-backups");
    let fastctx_command = resolve_fastctx_command(options.fast_context_tools);
    let use_official_catalog = !options.preserve_provider_route && options.use_official_catalog;
    let default_model = (!options.preserve_provider_route)
        .then_some(options.default_model)
        .flatten();
    let backup_dir = apply_runtime_provider_config_at_mode(
        home,
        profile,
        provider_id,
        ProviderApplyOptions {
            use_official_catalog,
            default_model,
            fastctx_command: fastctx_command.as_deref(),
            subagent_optimization: options.subagent_optimization,
            subagent_model: options.subagent_model,
            subagent_reasoning_effort: options.subagent_reasoning_effort,
            marker: &marker,
            backup_root: &backup_root,
            preserve_provider_route: options.preserve_provider_route,
            protocol_proxy_base_url: options.protocol_proxy_base_url,
            expected_config: options.expected_config,
        },
    )?;
    let config_contents =
        fs::read(backup_dir.join(APPLIED_CONFIG_FILE)).context("读取 Codey 已应用配置快照失败")?;
    Ok(AppliedRuntimeProviderConfig { config_contents })
}

const FASTCTX_SERVER_BINARY: &str = if cfg!(windows) {
    "codey-fastctx.exe"
} else {
    "codey-fastctx"
};

/// FastCtx 以 sidecar 程序随 Codey 一起分发，主程序因此不携带内嵌分词器
/// 常量。启用了 FastCtx 但 sidecar 缺失时降级为本次不注册该工具：损失的是
/// 可选增强，而中止启动会让 Codex 完全用不了；缺失会记入错误日志便于定位
/// 打包问题。
fn resolve_fastctx_command(fast_context_tools: bool) -> Option<PathBuf> {
    if !fast_context_tools {
        return None;
    }
    match fastctx_server_command() {
        Ok(command) => Some(command),
        Err(error) => {
            eprintln!("Codey 本次未启用 FastCtx：{error:#}");
            crate::error_log::record_failure(
                "fastctx_sidecar_missing",
                "resolve_fastctx_command",
                format!("{error:#}"),
                serde_json::json!({}),
            );
            None
        }
    }
}

fn fastctx_server_command() -> Result<PathBuf> {
    let current = std::env::current_exe().context("定位 Codey FastCtx 服务失败")?;
    current
        .parent()
        .map(|dir| dir.join(FASTCTX_SERVER_BINARY))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!("未在 Codey 程序目录找到 FastCtx 服务程序 {FASTCTX_SERVER_BINARY}")
        })
}

fn persist_previous_fastctx_guidance_migration(
    path: &Path,
    original: Option<Vec<u8>>,
    include_subagent_guidance: bool,
    label: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(original) = original else {
        return Ok(None);
    };
    let existing = str::from_utf8(&original).with_context(|| format!("{label} 不是 UTF-8"))?;
    let Some(migrated) =
        migrate_previous_fastctx_guidance(existing, include_subagent_guidance, label)?
    else {
        return Ok(Some(original));
    };
    if read_optional(path)?.as_deref() != Some(original.as_slice()) {
        bail!("{label} 在历史 FastCtx 提示词迁移期间发生变化；已取消本次启动");
    }
    atomic_write(path, migrated.as_bytes())
        .with_context(|| format!("持久化 {label} 的历史 FastCtx 提示词迁移失败"))?;
    Ok(Some(migrated.into_bytes()))
}

fn migrate_previous_fastctx_guidance(
    existing: &str,
    include_subagent_guidance: bool,
    label: &str,
) -> Result<Option<String>> {
    if !existing.contains("Codey FastCtx context tools are enabled.") {
        return Ok(None);
    }
    let mut document = parse_document(existing).with_context(|| format!("解析 {label} 失败"))?;
    let root_changed = remove_guidance_from_table(
        document.as_table_mut(),
        "developer_instructions",
        remove_previous_codey_fastctx_guidance,
    );
    let subagent_changed = include_subagent_guidance
        && document
            .get_mut("features")
            .and_then(Item::as_table_like_mut)
            .and_then(|features| features.get_mut("multi_agent_v2"))
            .and_then(Item::as_table_like_mut)
            .is_some_and(|multi_agent| {
                remove_guidance_from_table(
                    multi_agent,
                    "subagent_developer_instructions",
                    remove_previous_codey_fastctx_guidance,
                )
            });
    if !root_changed && !subagent_changed {
        return Ok(None);
    }
    document_string(&document).map(Some)
}

fn apply_runtime_provider_config_at_mode(
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
    let original_config_on_disk = read_optional(&config_path)?;
    if let Some(expected_config) = expected_config
        && original_config_on_disk.as_deref() != Some(expected_config)
    {
        bail!("CC Switch Live 配置在启动准备期间发生变化；已取消本次启动以避免混用线路");
    }
    let original_agents_dir_exists = agents_dir.is_dir();
    let original_config = persist_previous_fastctx_guidance_migration(
        &config_path,
        original_config_on_disk,
        true,
        "Codex config.toml",
    )?;
    let migrated_default_agent = persist_previous_fastctx_guidance_migration(
        &default_agent_path,
        read_optional(&default_agent_path)?,
        false,
        "Codex agents/default.toml",
    )?;
    let original_agents_md = if subagent_optimization {
        read_optional(&agents_md_path)?
    } else {
        None
    };
    let original_default_agent = if subagent_optimization {
        migrated_default_agent
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
    let updated_agents_md = if subagent_optimization {
        let existing_agents_md = str::from_utf8(original_agents_md.as_deref().unwrap_or_default())
            .context("Codex AGENTS.md 不是 UTF-8")?;
        Some(append_subagent_guidance(existing_agents_md))
    } else {
        None
    };
    let provider_id = validated_provider_id(provider_id)?;
    // Codex resolves this path from the app-server working directory, which is
    // `/` for the packaged macOS app, rather than from CODEX_HOME.
    let model_catalog_path =
        use_official_catalog.then(|| home.join(crate::model_catalog::relative_path()));
    let updated = patch_config_with_fastctx_mode_and_proxy(
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
    let applied_base_url = provider_base_url(&updated, &provider_id);
    let updated_default_agent = if subagent_optimization {
        let fastctx_namespace = if fastctx_command.is_some() {
            let updated_document =
                parse_document(&updated).context("解析已应用 Codex 临时配置失败")?;
            updated_document
                .get("mcp_servers")
                .and_then(Item::as_table)
                .and_then(|servers| servers.get(CODEY_FASTCTX_SERVER_ID))
                .and_then(Item::as_table)
                .is_some_and(fastctx_table_server_is_codey_owned)
                .then(|| CODEY_FASTCTX_NAMESPACE.to_string())
        } else {
            None
        };
        Some(default_agent_config_with_fastctx_guidance(
            fastctx_namespace.as_deref(),
        ))
    } else {
        None
    };
    if let Err(error) =
        write_private_file(&backup_dir.join(APPLIED_CONFIG_FILE), updated.as_bytes())
    {
        let _ = fs::remove_dir_all(&backup_dir);
        return Err(error).context("保存 Codey 已应用配置快照失败");
    }
    if subagent_optimization {
        if let Some(bytes) = original_agents_md.as_deref() {
            write_private_file(&backup_dir.join("AGENTS.md"), bytes)?;
        }
        create_private_dir_all(&backup_dir.join("agents"))?;
        if let Some(bytes) = original_default_agent.as_deref() {
            write_private_file(&backup_dir.join("agents/default.toml"), bytes)?;
        }
        write_private_file(
            &backup_dir.join(APPLIED_AGENTS_MD_FILE),
            updated_agents_md
                .as_deref()
                .expect("subagent guidance was prepared")
                .as_bytes(),
        )?;
        write_private_file(
            &backup_dir.join(APPLIED_DEFAULT_AGENT_FILE),
            updated_default_agent
                .as_deref()
                .expect("default agent config was prepared")
                .as_bytes(),
        )?;
    }
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
        original_agents_md_exists: original_agents_md.is_some(),
        original_default_agent_exists: original_default_agent.is_some(),
        original_agents_dir_exists,
        provider_id: Some(provider_id),
        applied_base_url,
    };
    if let Err(error) = write_lease(marker, &state) {
        let _ = fs::remove_dir_all(&backup_dir);
        return Err(error);
    }

    let inputs_unchanged = optional_file_matches(&config_path, original_config.as_deref())?
        && (!subagent_optimization
            || (optional_file_matches(&agents_md_path, original_agents_md.as_deref())?
                && optional_file_matches(&default_agent_path, original_default_agent.as_deref())?));
    if !inputs_unchanged {
        discard_runtime_lease(marker, &backup_dir).with_context(|| {
            "Codex 配置在 Codey 保存运行时快照后发生变化；取消启动时清理租约失败，恢复备份已保留"
        })?;
        bail!("Codex 配置在 Codey 保存运行时快照后发生变化；已取消本次启动");
    }

    let write_result = (|| -> Result<()> {
        atomic_write(&config_path, updated.as_bytes())?;
        if let Some(updated_agents_md) = updated_agents_md.as_deref() {
            atomic_write(&agents_md_path, updated_agents_md.as_bytes())?;
            create_private_dir_all(&agents_dir)?;
            atomic_write(
                &default_agent_path,
                updated_default_agent
                    .as_deref()
                    .expect("default agent config was prepared")
                    .as_bytes(),
            )?;
        }
        Ok(())
    })();
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
    Ok(backup_dir)
}

fn optional_file_matches(path: &Path, expected: Option<&[u8]>) -> Result<bool> {
    Ok(read_optional(path)?.as_deref() == expected)
}

fn discard_runtime_lease(marker: &Path, backup_dir: &Path) -> Result<()> {
    remove_optional(marker)?;
    let _ = fs::remove_dir_all(backup_dir);
    Ok(())
}

fn restore_optional_bytes(path: &Path, original: Option<&[u8]>) -> Result<()> {
    match original {
        Some(bytes) => atomic_write(path, bytes),
        None => remove_optional(path),
    }
}

fn remove_empty_dir(path: &Path) -> Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn write_lease(path: &Path, state: &RuntimeConfigLease) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(path, &serde_json::to_vec_pretty(state)?)
}

pub fn mark_runtime_subagent_defaults_applied(
    home: &Path,
    model: &str,
    reasoning_effort: &str,
) -> Result<()> {
    mark_runtime_subagent_defaults_applied_at(home, &lease_marker_path(), model, reasoning_effort)
}

fn mark_runtime_subagent_defaults_applied_at(
    home: &Path,
    marker: &Path,
    model: &str,
    reasoning_effort: &str,
) -> Result<()> {
    let model = model.trim();
    anyhow::ensure!(!model.is_empty(), "子代理模型不能为空");
    let reasoning_effort = reasoning_effort.trim().to_ascii_lowercase();
    anyhow::ensure!(
        SUBAGENT_REASONING_EFFORTS.contains(&reasoning_effort.as_str()),
        "子代理思考深度无效：{reasoning_effort}"
    );

    let mut state = fs::read_to_string(marker)
        .with_context(|| format!("读取 Codey Codex lease 失败：{}", marker.display()))
        .and_then(|contents| {
            serde_json::from_str::<RuntimeConfigLease>(&contents)
                .with_context(|| format!("解析 Codey Codex lease 失败：{}", marker.display()))
        })?;
    anyhow::ensure!(
        state.subagent_optimization_applied,
        "当前 Codey 运行时未启用子代理协作优化"
    );

    let config_path = home.join("config.toml");
    let current_bytes =
        wait_for_subagent_defaults_in_config(&config_path, model, &reasoning_effort)?;

    let snapshot_dir = state
        .config_snapshot_dir
        .as_deref()
        .unwrap_or(&state.backup_dir);
    let applied_path = snapshot_dir.join(APPLIED_CONFIG_FILE);
    let applied_bytes = fs::read(&applied_path)
        .with_context(|| format!("读取 Codey 已应用配置快照失败：{}", applied_path.display()))?;
    let applied =
        String::from_utf8(applied_bytes.clone()).context("Codey 已应用配置快照不是 UTF-8")?;
    let mut applied_doc = applied
        .parse::<DocumentMut>()
        .context("解析 Codey 已应用配置快照失败")?;
    let agents = ensure_root_table(&mut applied_doc, "agents")?;
    agents["default_subagent_model"] = value(model);
    agents["default_subagent_reasoning_effort"] = value(&reasoning_effort);
    let updated_applied = document_string(&applied_doc)?;

    anyhow::ensure!(
        read_optional(&config_path)?.as_deref() == Some(current_bytes.as_slice()),
        "Codex config.toml 在 Codey 更新租约快照前再次变化"
    );
    state.subagent_model = model.to_string();
    state.subagent_reasoning_effort = reasoning_effort;
    commit_runtime_subagent_snapshot(
        marker,
        &state,
        &applied_path,
        &applied_bytes,
        updated_applied.as_bytes(),
    )
}

fn commit_runtime_subagent_snapshot(
    marker: &Path,
    state: &RuntimeConfigLease,
    applied_path: &Path,
    previous_applied: &[u8],
    updated_applied: &[u8],
) -> Result<()> {
    atomic_write(applied_path, updated_applied)?;
    if let Err(lease_error) = write_lease(marker, state) {
        if let Err(rollback_error) = atomic_write(applied_path, previous_applied) {
            anyhow::bail!(
                "更新 Codey 子代理运行时租约失败：{lease_error:#}；\
                 恢复已应用配置快照也失败：{rollback_error:#}"
            );
        }
        return Err(lease_error).context("更新 Codey 子代理运行时租约失败；已恢复原快照");
    }
    Ok(())
}

fn wait_for_subagent_defaults_in_config(
    config_path: &Path,
    model: &str,
    reasoning_effort: &str,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_millis(SUBAGENT_DEFAULTS_VERIFY_TIMEOUT_MS);
    loop {
        let error = match read_subagent_defaults_config(config_path, model, reasoning_effort) {
            Ok(Some(bytes)) => return Ok(bytes),
            Ok(None) => "Codex config.toml 尚未写入新的子代理默认配置".to_string(),
            Err(error) => format!("{error:#}"),
        };
        if Instant::now() >= deadline {
            anyhow::bail!("{error}");
        }
        thread::sleep(Duration::from_millis(SUBAGENT_DEFAULTS_VERIFY_POLL_MS));
    }
}

fn read_subagent_defaults_config(
    config_path: &Path,
    model: &str,
    reasoning_effort: &str,
) -> Result<Option<Vec<u8>>> {
    let bytes = fs::read(config_path)
        .with_context(|| format!("读取 Codex 配置失败：{}", config_path.display()))?;
    let current = String::from_utf8(bytes.clone()).context("Codex config.toml 不是 UTF-8")?;
    let document = current
        .parse::<DocumentMut>()
        .context("解析 Codex config.toml 失败")?;
    let agents = document
        .get("agents")
        .and_then(Item::as_table)
        .context("Codex config.toml 缺少 [agents] 配置")?;
    let matches_defaults = agents.get("default_subagent_model").and_then(Item::as_str)
        == Some(model)
        && agents
            .get("default_subagent_reasoning_effort")
            .and_then(Item::as_str)
            == Some(reasoning_effort);
    Ok(matches_defaults.then_some(bytes))
}

pub fn restore_runtime_provider_config(home: &Path) -> Result<bool> {
    restore_runtime_provider_config_at(home, &lease_marker_path())
}

pub(crate) fn restore_runtime_cc_switch_provider_config(home: &Path) -> Result<bool> {
    restore_runtime_cc_switch_provider_config_at(home, &lease_marker_path())
}

fn restore_runtime_cc_switch_provider_config_at(home: &Path, marker: &Path) -> Result<bool> {
    let state = match fs::read_to_string(marker) {
        Ok(contents) => serde_json::from_str::<RuntimeConfigLease>(&contents)
            .with_context(|| format!("解析 Codey Codex lease 失败：{}", marker.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if state.preserve_provider_route || state.protocol_proxy_base_url.is_none() {
        return Ok(false);
    }
    let Some(provider_id) = state.provider_id.as_deref() else {
        return Ok(false);
    };
    let Some(applied_base_url) = state.applied_base_url.as_deref() else {
        return Ok(false);
    };
    let config_path = home.join("config.toml");
    let current = match fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex 配置失败：{}", config_path.display()));
        }
    };
    let provider_still_active =
        root_key_string(&current, "model_provider").as_deref() == Some(provider_id);
    let proxy_still_applied =
        provider_base_url(&current, provider_id).as_deref() == Some(applied_base_url);
    if !provider_still_active || !proxy_still_applied {
        return Ok(false);
    }

    let config_snapshot_dir = state
        .config_snapshot_dir
        .as_deref()
        .unwrap_or(&state.backup_dir);
    let original = if state.original_config_exists {
        fs::read_to_string(config_snapshot_dir.join("config.toml"))
            .context("读取 Codex 原配置备份失败")?
    } else {
        String::new()
    };
    let applied = fs::read_to_string(config_snapshot_dir.join(APPLIED_CONFIG_FILE))
        .context("读取 Codey 已应用配置快照失败")?;
    let restored = restore_owned_model_provider_changes(&original, &applied, &current)?;
    if restored == current || !optional_file_matches(&config_path, Some(current.as_bytes()))? {
        return Ok(false);
    }
    atomic_write(&config_path, restored.as_bytes())?;
    Ok(true)
}

fn restore_runtime_provider_config_at(home: &Path, marker: &Path) -> Result<bool> {
    let state = match fs::read_to_string(marker) {
        Ok(contents) => serde_json::from_str::<RuntimeConfigLease>(&contents)
            .with_context(|| format!("解析 Codey Codex lease 失败：{}", marker.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let config_path = home.join("config.toml");
    let current = match fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex 配置失败：{}", config_path.display()));
        }
    };
    let provider_id = state.provider_id.as_deref().unwrap_or(CODEY_PROVIDER_ID);
    let provider_matches =
        root_key_string(&current, "model_provider").as_deref() == Some(provider_id);
    let endpoint_matches = state.applied_base_url.as_deref().is_none_or(|base_url| {
        provider_base_url(&current, provider_id).as_deref() == Some(base_url)
    });
    let route_still_applied =
        state.preserve_provider_route || (provider_matches && endpoint_matches);

    let config_snapshot_dir = state
        .config_snapshot_dir
        .as_deref()
        .unwrap_or(&state.backup_dir);
    let backup_config = config_snapshot_dir.join("config.toml");
    let original = if state.original_config_exists {
        fs::read_to_string(&backup_config)
            .with_context(|| format!("找不到 Codex 原配置备份：{}", backup_config.display()))?
    } else {
        String::new()
    };
    let applied_config = config_snapshot_dir.join(APPLIED_CONFIG_FILE);
    let restored = if applied_config.exists() {
        let applied = fs::read_to_string(&applied_config).with_context(|| {
            format!(
                "读取 Codey 已应用配置快照失败：{}",
                applied_config.display()
            )
        })?;
        restore_owned_config_changes(&original, &applied, &current)?
    } else {
        restore_legacy_owned_config_changes(&original, &current, provider_id)?
    };
    if !state.original_config_exists && restored.trim().is_empty() {
        remove_optional(&config_path)?;
    } else {
        atomic_write(&config_path, restored.as_bytes())?;
    }
    restore_runtime_subagent_files(home, &state)?;
    remove_optional(marker)?;
    Ok(route_still_applied)
}

fn restore_runtime_subagent_files(home: &Path, state: &RuntimeConfigLease) -> Result<()> {
    if !state.subagent_optimization_applied {
        return Ok(());
    }

    let agents_md_path = home.join("AGENTS.md");
    let original_agents_md = if state.original_agents_md_exists {
        Some(
            fs::read(state.backup_dir.join("AGENTS.md"))
                .context("找不到 Codex 原 AGENTS.md 租约快照")?,
        )
    } else {
        None
    };
    let applied_agents_md = fs::read(state.backup_dir.join(APPLIED_AGENTS_MD_FILE))
        .context("找不到 Codey 已应用 AGENTS.md 租约快照")?;
    restore_agents_md(
        &agents_md_path,
        original_agents_md.as_deref(),
        &applied_agents_md,
    )?;

    let agents_dir = home.join("agents");
    let default_agent_path = agents_dir.join("default.toml");
    let original_default_agent = if state.original_default_agent_exists {
        Some(
            fs::read(state.backup_dir.join("agents/default.toml"))
                .context("找不到 Codex 原 default.toml 租约快照")?,
        )
    } else {
        None
    };
    let applied_default_agent = fs::read(state.backup_dir.join(APPLIED_DEFAULT_AGENT_FILE))
        .context("找不到 Codey 已应用 default.toml 租约快照")?;
    restore_if_still_applied(
        &default_agent_path,
        original_default_agent.as_deref(),
        &applied_default_agent,
    )?;
    if !state.original_agents_dir_exists {
        remove_empty_dir(&agents_dir)?;
    }
    Ok(())
}

fn restore_agents_md(path: &Path, original: Option<&[u8]>, applied: &[u8]) -> Result<()> {
    let Some(current) = read_optional(path)? else {
        return Ok(());
    };
    if current == applied {
        return restore_optional_bytes(path, original);
    }
    let original_contains_guidance = original
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .is_some_and(|contents| contents.contains(SUBAGENT_GUIDANCE));
    if original_contains_guidance {
        return Ok(());
    }
    let current = String::from_utf8(current).context("Codex 当前 AGENTS.md 不是 UTF-8")?;
    let Some(restored) = remove_subagent_guidance(&current) else {
        return Ok(());
    };
    if original.is_none() && restored.trim().is_empty() {
        remove_optional(path)
    } else {
        atomic_write(path, restored.as_bytes())
    }
}

fn restore_if_still_applied(path: &Path, original: Option<&[u8]>, applied: &[u8]) -> Result<()> {
    if read_optional(path)?.as_deref() == Some(applied) {
        restore_optional_bytes(path, original)?;
    }
    Ok(())
}

fn fastctx_table_server_is_codey_owned(server: &Table) -> bool {
    server
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(arguments_have_codey_fastctx_marker)
}

fn ensure_child_table<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    if parent.get(key).is_none() {
        parent.insert(key, Item::Table(Table::new()));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{key} 必须是 TOML table"))
}

/// Reads the provider bucket selected by the current Codex configuration.
/// Codex defaults to its built-in `openai` provider when the root key is absent.
pub fn current_model_provider(home: &Path) -> Result<String> {
    let config_path = home.join("config.toml");
    let original = read_optional(&config_path)?;
    let existing =
        String::from_utf8(original.unwrap_or_default()).context("Codex config.toml 不是 UTF-8")?;
    let doc = parse_document(&existing)?;
    Ok(doc
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .unwrap_or(BUILTIN_OPENAI_PROVIDER_ID)
        .to_string())
}

pub(crate) fn fast_context_tools_status(home: &Path) -> Result<FastContextToolsStatus> {
    let config_path = home.join("config.toml");
    let original = read_optional(&config_path)?;
    let existing =
        String::from_utf8(original.unwrap_or_default()).context("Codex config.toml 不是 UTF-8")?;
    let document = parse_document(&existing)?;
    Ok(fast_context_tools_status_from_document(&document))
}

#[cfg(test)]
pub fn patch_config(
    existing: &str,
    profile: &ProviderProfile,
    provider_id: &str,
    use_official_catalog: bool,
) -> Result<String> {
    let model_catalog_path =
        use_official_catalog.then(|| Path::new(crate::model_catalog::relative_path()));
    patch_config_with_fastctx(
        existing,
        profile,
        provider_id,
        model_catalog_path,
        None,
        None,
        false,
    )
}

#[cfg(test)]
fn patch_config_with_fastctx(
    existing: &str,
    profile: &ProviderProfile,
    provider_id: &str,
    model_catalog_path: Option<&Path>,
    default_model: Option<&str>,
    fastctx_command: Option<&Path>,
    subagent_optimization: bool,
) -> Result<String> {
    patch_config_with_fastctx_mode_and_proxy(
        existing,
        profile,
        provider_id,
        ProviderPatchOptions {
            config_path: Path::new("config.toml"),
            model_catalog_path,
            default_model,
            fastctx_command,
            subagent_optimization,
            subagent_model: DEFAULT_SUBAGENT_MODEL,
            subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
            preserve_provider_route: false,
            protocol_proxy_base_url: None,
        },
    )
}

struct ProviderPatchOptions<'a> {
    config_path: &'a Path,
    model_catalog_path: Option<&'a Path>,
    default_model: Option<&'a str>,
    fastctx_command: Option<&'a Path>,
    subagent_optimization: bool,
    subagent_model: &'a str,
    subagent_reasoning_effort: &'a str,
    preserve_provider_route: bool,
    protocol_proxy_base_url: Option<&'a str>,
}

fn patch_config_with_fastctx_mode_and_proxy(
    existing: &str,
    profile: &ProviderProfile,
    provider_id: &str,
    options: ProviderPatchOptions<'_>,
) -> Result<String> {
    let ProviderPatchOptions {
        config_path,
        model_catalog_path,
        default_model,
        fastctx_command,
        subagent_optimization,
        subagent_model,
        subagent_reasoning_effort,
        preserve_provider_route,
        protocol_proxy_base_url,
    } = options;
    if !preserve_provider_route {
        ensure_supported_provider_protocol(profile.protocol, protocol_proxy_base_url)?;
    }
    let mut doc = parse_document(existing)?;
    if preserve_provider_route {
        apply_preserved_provider_route(&mut doc, protocol_proxy_base_url)?;
    }
    // CC Switch owns all routing and model-selection fields while Live
    // takeover is active. Codey only layers its independent runtime
    // enhancements onto the current Live document.
    if !preserve_provider_route {
        let provider_id = validated_provider_id(provider_id)?;
        let uses_builtin_provider =
            profile.cc_switch_read_only && is_reserved_provider_id(&provider_id);
        if uses_builtin_provider {
            // Keep built-in providers built-in. Current Codex ignores most
            // configured overrides for reserved provider ids, and these routes
            // already obtain their endpoint and authentication internally.
        } else {
            if is_reserved_provider_id(&provider_id) {
                anyhow::bail!(
                    "当前第三方线路使用了 Codex 保留 Provider ID「{provider_id}」；请改用非保留的自定义 ID 后重试"
                );
            }
            ensure_provider_table(&mut doc)?;
            let existing_local_provider = profile
                .cc_switch_provider_id
                .is_none()
                .then(|| {
                    doc.get("model_providers")
                        .and_then(Item::as_table)
                        .and_then(|providers| providers.get(&provider_id))
                        .and_then(Item::as_table)
                        .cloned()
                })
                .flatten();
            let provider = if profile.cc_switch_read_only {
                official_provider_table()
            } else {
                direct_provider_table(profile, existing_local_provider, protocol_proxy_base_url)?
            };
            doc["model_providers"]
                .as_table_mut()
                .expect("model_providers was initialized")[&provider_id] = Item::Table(provider);
        }
        doc["model_provider"] = value(&provider_id);
        if let Some(model_catalog_path) = model_catalog_path {
            doc["model_catalog_json"] = value(model_catalog_path.to_string_lossy().into_owned());
        } else {
            doc.as_table_mut().remove("model_catalog_json");
        }
        set_model_selection(&mut doc, default_model);
    }
    enable_desktop_reasoning_efforts(&mut doc)?;
    ensure_default_service_tier(&mut doc);
    let fastctx_namespace = if let Some(command) = fastctx_command {
        enable_fast_context_tools(&mut doc, command)?
    } else {
        disable_fast_context_tools(&mut doc);
        None
    };
    if subagent_optimization {
        enable_subagent_optimization(
            &mut doc,
            config_path,
            subagent_model,
            subagent_reasoning_effort,
            fastctx_namespace.as_deref(),
        )?;
    }
    document_string(&doc)
}

fn enable_subagent_optimization(
    doc: &mut DocumentMut,
    config_path: &Path,
    subagent_model: &str,
    subagent_reasoning_effort: &str,
    fastctx_namespace: Option<&str>,
) -> Result<()> {
    let subagent_model = subagent_model.trim();
    if subagent_model.is_empty() {
        bail!("子代理模型不能为空");
    }
    let subagent_reasoning_effort = subagent_reasoning_effort.trim().to_ascii_lowercase();
    if !SUBAGENT_REASONING_EFFORTS.contains(&subagent_reasoning_effort.as_str()) {
        bail!("子代理思考深度无效：{subagent_reasoning_effort}");
    }
    let inherited_developer_instructions = fastctx_namespace.map(|_| {
        doc.get("developer_instructions")
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let agents = ensure_root_table(doc, "agents")?;
    for legacy_key in ["max_threads", "max_depth", "interrupt_message"] {
        agents.remove(legacy_key);
    }
    agents["default_subagent_model"] = value(subagent_model);
    agents["default_subagent_reasoning_effort"] = value(subagent_reasoning_effort);
    let features = ensure_root_table(doc, "features")?;
    if features.get("multi_agent_v2").is_none() {
        features["multi_agent_v2"] = Item::Table(Table::new());
    }
    let multi_agent = features["multi_agent_v2"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("features.multi_agent_v2 必须是 TOML table"))?;
    multi_agent["enabled"] = value(true);
    multi_agent["wait_agent_enabled"] = value(true);
    multi_agent["hide_spawn_agent_metadata"] = value(true);
    multi_agent["expose_spawn_agent_model_overrides"] = value(false);
    multi_agent["tool_namespace"] = value("agents");
    multi_agent["max_concurrent_threads_per_session"] = value(7);
    multi_agent["min_wait_timeout_ms"] = value(10_000);
    multi_agent["default_wait_timeout_ms"] = value(30_000);
    multi_agent["max_wait_timeout_ms"] = value(120_000);
    let existing_root_usage_hint = multi_agent
        .get("root_agent_usage_hint_text")
        .map(|item| {
            item.as_str().ok_or_else(|| {
                anyhow::anyhow!("features.multi_agent_v2.root_agent_usage_hint_text 必须是字符串")
            })
        })
        .transpose()?
        .unwrap_or_default();
    multi_agent["root_agent_usage_hint_text"] = value(append_root_agent_collaboration_usage_hint(
        existing_root_usage_hint,
    ));
    if let Some(namespace) = fastctx_namespace {
        if multi_agent.get("subagent_developer_instructions").is_none() {
            multi_agent["subagent_developer_instructions"] =
                value(inherited_developer_instructions.unwrap_or_default());
        }
        apply_fastctx_guidance_to_table(
            multi_agent,
            "subagent_developer_instructions",
            namespace,
            "features.multi_agent_v2.subagent_developer_instructions",
        )?;
    }
    features["hooks"] = value(true);
    enable_subagent_gate_hooks(doc, config_path)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct SubagentGateHookSpec {
    toml_event: &'static str,
    event_key: &'static str,
    matcher: Option<&'static str>,
    timeout_seconds: u64,
}

const SUBAGENT_GATE_HOOKS: [SubagentGateHookSpec; 6] = [
    SubagentGateHookSpec {
        toml_event: "PreToolUse",
        event_key: "pre_tool_use",
        matcher: Some("*"),
        timeout_seconds: crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    },
    SubagentGateHookSpec {
        toml_event: "PostToolUse",
        event_key: "post_tool_use",
        matcher: Some(".*wait_agent$"),
        timeout_seconds: crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    },
    SubagentGateHookSpec {
        toml_event: "SubagentStart",
        event_key: "subagent_start",
        matcher: None,
        timeout_seconds: crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    },
    SubagentGateHookSpec {
        toml_event: "SubagentStop",
        event_key: "subagent_stop",
        matcher: None,
        timeout_seconds: crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    },
    SubagentGateHookSpec {
        toml_event: "Stop",
        event_key: "stop",
        matcher: None,
        timeout_seconds: crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    },
    SubagentGateHookSpec {
        toml_event: "SessionEnd",
        event_key: "session_end",
        matcher: None,
        timeout_seconds: crate::subagent_gate::SESSION_END_HOOK_TIMEOUT_SECONDS,
    },
];

fn enable_subagent_gate_hooks(doc: &mut DocumentMut, config_path: &Path) -> Result<()> {
    let commands = crate::subagent_gate::hook_commands()?;
    let selected_command = if cfg!(windows) {
        commands.command_windows.as_str()
    } else {
        commands.command.as_str()
    };

    for spec in SUBAGENT_GATE_HOOKS {
        let group_index = {
            let hooks = ensure_root_table(doc, "hooks")?;
            append_subagent_gate_hook(hooks, spec, &commands)?
        };
        let key = format!(
            "{}:{}:{group_index}:0",
            config_path.display(),
            spec.event_key
        );
        let trusted_hash = crate::subagent_gate::hook_trust_hash(
            spec.event_key,
            spec.matcher,
            selected_command,
            spec.timeout_seconds,
        );
        let hooks = ensure_root_table(doc, "hooks")?;
        let state = ensure_child_table(hooks, "state")?;
        let mut entry = Table::new();
        entry["trusted_hash"] = value(trusted_hash);
        state.insert(&key, Item::Table(entry));
    }
    Ok(())
}

fn append_subagent_gate_hook(
    hooks: &mut Table,
    spec: SubagentGateHookSpec,
    commands: &crate::subagent_gate::HookCommands,
) -> Result<usize> {
    if hooks.get(spec.toml_event).is_none() {
        hooks.insert(spec.toml_event, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let event = hooks
        .get_mut(spec.toml_event)
        .expect("subagent gate hook event was initialized");
    match event {
        Item::ArrayOfTables(groups) => {
            let index = groups.len();
            let mut group = Table::new();
            if let Some(matcher) = spec.matcher {
                group["matcher"] = value(matcher);
            }
            let mut handlers = ArrayOfTables::new();
            handlers.push(subagent_gate_hook_table(spec, commands));
            group["hooks"] = Item::ArrayOfTables(handlers);
            groups.push(group);
            Ok(index)
        }
        Item::Value(Value::Array(groups)) => {
            let index = groups.len();
            let mut group = InlineTable::new();
            if let Some(matcher) = spec.matcher {
                group.insert("matcher", Value::from(matcher));
            }
            let mut handlers = Array::new();
            handlers.push(Value::InlineTable(subagent_gate_hook_inline_table(
                spec, commands,
            )));
            group.insert("hooks", Value::Array(handlers));
            groups.push(Value::InlineTable(group));
            Ok(index)
        }
        _ => bail!("hooks.{} 必须是 Hook 配置数组", spec.toml_event),
    }
}

fn subagent_gate_hook_table(
    spec: SubagentGateHookSpec,
    commands: &crate::subagent_gate::HookCommands,
) -> Table {
    let mut handler = Table::new();
    handler["type"] = value("command");
    handler["command"] = value(&commands.command);
    handler["commandWindows"] = value(&commands.command_windows);
    handler["timeout"] = value(spec.timeout_seconds as i64);
    handler
}

fn subagent_gate_hook_inline_table(
    spec: SubagentGateHookSpec,
    commands: &crate::subagent_gate::HookCommands,
) -> InlineTable {
    let mut handler = InlineTable::new();
    handler.insert("type", Value::from("command"));
    handler.insert("command", Value::from(commands.command.as_str()));
    handler.insert(
        "commandWindows",
        Value::from(commands.command_windows.as_str()),
    );
    handler.insert("timeout", Value::from(spec.timeout_seconds as i64));
    handler
}

fn direct_provider_table(
    profile: &ProviderProfile,
    existing_local_provider: Option<Table>,
    protocol_proxy_base_url: Option<&str>,
) -> Result<Table> {
    let base_url = match profile.protocol {
        RelayProtocol::Responses => profile.normalized_base_url(),
        RelayProtocol::ChatCompletions => protocol_proxy_base_url
            .map(str::trim)
            .filter(|base_url| !base_url.is_empty())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Chat Completions 线路的本地协议代理尚未启动"))?,
    };
    if base_url.is_empty() {
        anyhow::bail!("第三方线路缺少 API 地址");
    }
    let preserves_manual_settings = existing_local_provider.is_some();
    let mut provider = existing_local_provider.unwrap_or_default();
    provider["name"] = value(if profile.supports_remote_compaction {
        OPENAI_PROVIDER_NAME
    } else {
        profile.name.trim()
    });
    provider["base_url"] = value(base_url);
    provider["wire_api"] = value("responses");
    if !preserves_manual_settings {
        provider["requires_openai_auth"] = value(true);
    }
    if !profile.api_key.trim().is_empty() {
        provider["experimental_bearer_token"] = value(profile.api_key.trim());
    }
    Ok(provider)
}

fn ensure_supported_provider_protocol(
    protocol: RelayProtocol,
    protocol_proxy_base_url: Option<&str>,
) -> Result<()> {
    match protocol {
        RelayProtocol::Responses => Ok(()),
        RelayProtocol::ChatCompletions
            if protocol_proxy_base_url
                .map(str::trim)
                .is_some_and(|base_url| !base_url.is_empty()) =>
        {
            Ok(())
        }
        RelayProtocol::ChatCompletions => {
            anyhow::bail!("Chat Completions 线路的本地 Responses 协议代理尚未启动")
        }
    }
}

fn apply_preserved_provider_route(
    doc: &mut DocumentMut,
    protocol_proxy_base_url: Option<&str>,
) -> Result<()> {
    let Some(protocol_proxy_base_url) = protocol_proxy_base_url
        .map(str::trim)
        .filter(|base_url| !base_url.is_empty())
    else {
        return ensure_active_provider_uses_responses(doc);
    };
    let provider_id = doc
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider_id| !provider_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("CC Switch Live 配置缺少活动 model_provider"))?
        .to_string();
    let provider = doc
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(&provider_id))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("CC Switch Live 配置缺少活动 Provider「{provider_id}」"))?;
    provider["base_url"] = value(protocol_proxy_base_url);
    provider["wire_api"] = value("responses");
    Ok(())
}

fn ensure_active_provider_uses_responses(doc: &DocumentMut) -> Result<()> {
    let provider_id = doc
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider_id| !provider_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("CC Switch Live 配置缺少活动 model_provider"))?;
    let wire_api = doc
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like)
        .and_then(|provider| provider.get("wire_api"))
        .and_then(Item::as_str)
        .unwrap_or("responses")
        .trim();
    if wire_api.eq_ignore_ascii_case("responses") {
        Ok(())
    } else {
        anyhow::bail!(
            "当前 Codex 已移除 wire_api = {wire_api:?}；请将 CC Switch Live 线路改为 Responses API 后重试"
        )
    }
}

fn official_provider_table() -> Table {
    let mut provider = Table::new();
    provider["name"] = value(OPENAI_PROVIDER_NAME);
    provider["base_url"] = value(CHATGPT_CODEX_BASE_URL);
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    provider
}

fn parse_document(existing: &str) -> Result<DocumentMut> {
    if existing.trim().is_empty() {
        Ok(DocumentMut::new())
    } else {
        existing
            .parse::<DocumentMut>()
            .context("Codex config.toml TOML 解析失败")
    }
}

fn ensure_provider_table(doc: &mut DocumentMut) -> Result<()> {
    if doc
        .get("model_providers")
        .and_then(Item::as_table)
        .is_none()
    {
        doc["model_providers"] = Item::Table(Table::new());
    }
    doc["model_providers"]
        .as_table_mut()
        .map(|_| ())
        .ok_or_else(|| anyhow::anyhow!("model_providers 必须是 TOML table"))
}

fn ensure_root_table<'a>(doc: &'a mut DocumentMut, key: &str) -> Result<&'a mut Table> {
    if doc.get(key).is_none() {
        doc[key] = Item::Table(Table::new());
    }
    doc[key]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("{key} 必须是 TOML table"))
}

fn document_string(doc: &DocumentMut) -> Result<String> {
    let mut result = doc.to_string();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

fn enable_desktop_reasoning_efforts(doc: &mut DocumentMut) -> Result<()> {
    if doc.get("desktop").and_then(Item::as_table).is_none() {
        doc["desktop"] = Item::Table(Table::new());
    }
    let desktop = doc["desktop"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("desktop 必须是 TOML table"))?;
    let mut efforts = Array::new();
    for effort in ["low", "medium", "high", "xhigh", "max", "ultra"] {
        efforts.push(effort);
    }
    desktop["enabled-reasoning-efforts"] = value(efforts);
    Ok(())
}

fn ensure_default_service_tier(doc: &mut DocumentMut) {
    if doc.get("service_tier").is_none() {
        doc["service_tier"] = value("default");
    }
}

fn remove_model_selection(doc: &mut DocumentMut) {
    doc.as_table_mut().remove("model");
    let Some(profiles) = doc.get_mut("profiles").and_then(Item::as_table_mut) else {
        return;
    };
    for (_, profile) in profiles.iter_mut() {
        if let Some(profile) = profile.as_table_mut() {
            profile.remove("model");
        }
    }
}

fn set_model_selection(doc: &mut DocumentMut, default_model: Option<&str>) {
    remove_model_selection(doc);
    let Some(default_model) = default_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return;
    };
    doc["model"] = value(default_model);
}

fn root_key_string(contents: &str, key: &str) -> Option<String> {
    let doc = contents.parse::<DocumentMut>().ok()?;
    doc.get(key).and_then(Item::as_str).map(ToString::to_string)
}

fn provider_base_url(contents: &str, provider_id: &str) -> Option<String> {
    let doc = contents.parse::<DocumentMut>().ok()?;
    doc.get("model_providers")
        .and_then(Item::as_table)?
        .get(provider_id)
        .and_then(Item::as_table)?
        .get("base_url")
        .and_then(Item::as_str)
        .map(|value| value.trim_end_matches('/').to_string())
}

fn validated_provider_id(provider_id: &str) -> Result<String> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        anyhow::bail!("Codex 活动 model_provider 不能为空");
    }
    Ok(provider_id.to_string())
}

pub(crate) fn is_reserved_provider_id(provider_id: &str) -> bool {
    RESERVED_PROVIDER_IDS.contains(&provider_id)
}

const BACKUP_RETENTION_COUNT: usize = 5;

/// Best-effort retention for the launch backup root: keeps the newest few
/// `{timestamp}-{pid}` run directories plus any directory a live lease still
/// references, so crash recovery always finds its snapshot while stale runs
/// stop accumulating forever.
fn prune_stale_backup_dirs(backup_root: &Path, marker: &Path) {
    let protected = fs::read_to_string(marker)
        .ok()
        .and_then(|contents| serde_json::from_str::<RuntimeConfigLease>(&contents).ok())
        .map(|lease| lease.backup_dir);
    let Ok(entries) = fs::read_dir(backup_root) else {
        return;
    };
    let mut runs = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let name = entry.file_name();
            let (timestamp, pid) = name.to_str()?.split_once('-')?;
            if timestamp.is_empty()
                || pid.is_empty()
                || !timestamp.bytes().all(|byte| byte.is_ascii_digit())
                || !pid.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let path = entry.path();
            if protected.as_deref() == Some(path.as_path()) {
                return None;
            }
            Some((timestamp.parse::<u128>().ok()?, path))
        })
        .collect::<Vec<_>>();
    if runs.len() <= BACKUP_RETENTION_COUNT {
        return;
    }
    runs.sort_by_key(|run| std::cmp::Reverse(run.0));
    for (_, path) in runs.drain(BACKUP_RETENTION_COUNT..) {
        let _ = fs::remove_dir_all(&path);
    }
}

#[cfg(test)]
mod tests;
