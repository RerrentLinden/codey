use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use codey_runtime_core::settings::RelayProtocol;
use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

#[cfg(test)]
use crate::codex_config_guidance::{
    CODEY_FASTCTX_GUIDANCE_VERSIONS, PREVIOUS_ROOT_AGENT_COLLABORATION_USAGE_HINT,
    ROOT_AGENT_COLLABORATION_USAGE_HINT,
};
use crate::codex_config_guidance::{
    SUBAGENT_GUIDANCE, append_root_agent_collaboration_usage_hint, append_subagent_guidance,
    codey_fastctx_guidance_blocks, codey_fastctx_guidance_for_namespace,
    default_agent_config_with_fastctx_guidance, remove_codey_fastctx_guidance,
    remove_previous_codey_fastctx_guidance, remove_subagent_guidance,
};
#[cfg(test)]
use crate::config::{DEFAULT_SUBAGENT_MODEL, DEFAULT_SUBAGENT_REASONING_EFFORT};
use crate::config::{ProviderProfile, SUBAGENT_REASONING_EFFORTS, default_config_path};
use crate::fs_util::timestamp_millis;
use crate::provider_lease::CODEY_PROVIDER_ID;

mod fs_io;
mod toml_restore;

use fs_io::{
    atomic_write, create_private_dir_all, read_optional, remove_optional, write_private_file,
};
use toml_restore::restore_owned_config_changes;
#[cfg(test)]
use toml_restore::{items_semantically_equal, tables_semantically_equal};

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
            .and_then(Item::as_table_mut)
            .and_then(|features| features.get_mut("multi_agent_v2"))
            .and_then(Item::as_table_mut)
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

fn restore_legacy_owned_config_changes(
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

fn fastctx_table_server_is_codey_owned(server: &Table) -> bool {
    server
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(arguments_have_codey_fastctx_marker)
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
            subagent_model,
            subagent_reasoning_effort,
            fastctx_namespace.as_deref(),
        )?;
    }
    document_string(&doc)
}

fn enable_subagent_optimization(
    doc: &mut DocumentMut,
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
    Ok(())
}

fn enable_fast_context_tools(doc: &mut DocumentMut, command: &Path) -> Result<Option<String>> {
    let codey_owned_server = mcp_server_is_codey_owned_by_id(doc, CODEY_FASTCTX_SERVER_ID);
    if configured_user_fastctx_server_id(doc).is_some() {
        disable_fast_context_tools(doc);
        return Ok(None);
    }

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
    server["tool_timeout_sec"] = value(120);
    let mut env = server
        .get("env")
        .and_then(item_table_clone)
        .unwrap_or_default();
    env["FASTCTX_TOKEN_BUDGET"] = value(CODEY_FASTCTX_TOKEN_BUDGET);
    server["env"] = Item::Table(env);

    // Direct-only namespaces disappear from the nested `tools` object used by
    // code mode. FastCtx must be available there as well as through direct
    // calls, otherwise code-mode turns fall back to Codex's generic MCP
    // Resources helpers and can pass the tool namespace as an invalid server id.
    remove_direct_only_tool_namespace(doc, CODEY_FASTCTX_NAMESPACE);

    if doc.get("tool_output_token_limit").is_none() {
        doc["tool_output_token_limit"] = value(10_000);
    }
    apply_fastctx_guidance(doc, CODEY_FASTCTX_NAMESPACE)?;
    Ok(Some(CODEY_FASTCTX_NAMESPACE.to_string()))
}

fn apply_fastctx_guidance(doc: &mut DocumentMut, namespace: &str) -> Result<()> {
    apply_fastctx_guidance_to_table(
        doc.as_table_mut(),
        "developer_instructions",
        namespace,
        "developer_instructions",
    )
}

fn apply_fastctx_guidance_to_table(
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

fn disable_fast_context_tools(doc: &mut DocumentMut) {
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
        .and_then(Item::as_table_mut)
        .and_then(|features| features.get_mut("multi_agent_v2"))
        .and_then(Item::as_table_mut)
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
    let Some(namespaces) = doc
        .get_mut("features")
        .and_then(Item::as_table_mut)
        .and_then(|features| features.get_mut("code_mode"))
        .and_then(Item::as_table_mut)
        .and_then(|code_mode| code_mode.get_mut("direct_only_tool_namespaces"))
        .and_then(Item::as_array_mut)
    else {
        return false;
    };
    let original_len = namespaces.len();
    namespaces.retain(|entry| entry.as_str() != Some(namespace));
    namespaces.len() != original_len
}

fn remove_guidance_from_table(
    table: &mut Table,
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
        table[key] = value(restored_guidance);
    }
    true
}

fn fast_context_tools_status_from_document(doc: &DocumentMut) -> FastContextToolsStatus {
    let server_id = configured_user_fastctx_server_id(doc);
    FastContextToolsStatus {
        user_configured: server_id.is_some(),
        detection_failed: false,
        server_id,
    }
}

fn configured_user_fastctx_server_id(doc: &DocumentMut) -> Option<String> {
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

fn arguments_have_codey_fastctx_marker(arguments: &Array) -> bool {
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

fn mcp_server_exists(doc: &DocumentMut, server_id: &str) -> bool {
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
mod tests {
    use super::*;
    use crate::codex_config_guidance::{
        CODEY_FASTCTX_GUIDANCE, DEFAULT_AGENT_CONFIG, PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3,
    };

    const GLOBAL_PROVIDER_ID: &str = "codey_global";

    #[test]
    fn runtime_input_guard_detects_concurrent_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");

        assert!(optional_file_matches(&path, None).unwrap());
        fs::write(&path, b"original").unwrap();
        assert!(!optional_file_matches(&path, None).unwrap());
        assert!(optional_file_matches(&path, Some(b"original")).unwrap());
        fs::write(&path, b"concurrent").unwrap();
        assert!(!optional_file_matches(&path, Some(b"original")).unwrap());
    }

    #[test]
    fn failed_lease_marker_removal_keeps_the_recovery_backup() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("codex-lease.json");
        let backup_dir = temp.path().join("codex-backups/active");
        fs::create_dir_all(&marker).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(backup_dir.join("config.toml"), b"recoverable").unwrap();

        let error = discard_runtime_lease(&marker, &backup_dir)
            .unwrap_err()
            .to_string();

        assert!(error.contains("删除文件失败"));
        assert!(marker.is_dir());
        assert!(backup_dir.is_dir());
        assert_eq!(
            fs::read(backup_dir.join("config.toml")).unwrap(),
            b"recoverable"
        );
    }

    #[test]
    fn failed_subagent_lease_write_restores_the_previous_applied_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("codex-lease.json");
        let backup_dir = temp.path().join("codex-backups/active");
        let applied_path = backup_dir.join(APPLIED_CONFIG_FILE);
        fs::create_dir_all(&marker).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(&applied_path, b"previous applied").unwrap();
        let state = RuntimeConfigLease {
            backup_dir,
            config_snapshot_dir: None,
            original_config_exists: true,
            preserve_provider_route: false,
            protocol_proxy_base_url: None,
            fastctx_command: None,
            subagent_optimization_applied: true,
            subagent_model: "provider-coder".into(),
            subagent_reasoning_effort: "high".into(),
            original_agents_md_exists: false,
            original_default_agent_exists: false,
            original_agents_dir_exists: false,
            provider_id: Some(GLOBAL_PROVIDER_ID.into()),
            applied_base_url: None,
        };

        let error = commit_runtime_subagent_snapshot(
            &marker,
            &state,
            &applied_path,
            b"previous applied",
            b"updated applied",
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("已恢复原快照"));
        assert_eq!(fs::read(&applied_path).unwrap(), b"previous applied");
        assert!(marker.is_dir());
    }

    #[test]
    fn stale_backup_dirs_are_pruned_beyond_retention() {
        let temp = tempfile::tempdir().unwrap();
        let backup_root = temp.path().join("codex-backups");
        for index in 0..8_u32 {
            fs::create_dir_all(backup_root.join(format!("{}-42", 1000 + index))).unwrap();
        }
        fs::create_dir_all(backup_root.join("unrelated")).unwrap();
        let marker = temp.path().join("codex-lease.json");
        let lease = serde_json::json!({
            "backupDir": backup_root.join("1000-42"),
            "originalConfigExists": true,
        });
        fs::write(&marker, lease.to_string()).unwrap();

        prune_stale_backup_dirs(&backup_root, &marker);

        assert!(backup_root.join("1000-42").is_dir(), "lease dir kept");
        assert!(!backup_root.join("1001-42").is_dir(), "oldest pruned");
        assert!(!backup_root.join("1002-42").is_dir(), "oldest pruned");
        for index in 3..8_u32 {
            assert!(backup_root.join(format!("{}-42", 1000 + index)).is_dir());
        }
        assert!(backup_root.join("unrelated").is_dir(), "foreign dir kept");
    }

    fn official_profile() -> ProviderProfile {
        let mut profile = ProviderProfile::new("OpenAI Official");
        profile.id = "codex-official".to_string();
        profile.cc_switch_read_only = true;
        profile
    }

    fn direct_profile(protocol: RelayProtocol) -> ProviderProfile {
        let mut profile = ProviderProfile::new("Relay");
        profile.base_url = "https://relay.example/v1".to_string();
        profile.api_key = "sk-direct".to_string();
        profile.protocol = protocol;
        profile
    }

    fn relative_model_catalog_path() -> Option<&'static Path> {
        Some(Path::new(crate::model_catalog::relative_path()))
    }

    fn write_legacy_runtime_lease(
        marker: &Path,
        backup_dir: &Path,
        original: Option<&str>,
        provider_id: &str,
        applied_base_url: &str,
    ) {
        fs::create_dir_all(backup_dir).unwrap();
        if let Some(original) = original {
            fs::write(backup_dir.join("config.toml"), original).unwrap();
        }
        write_lease(
            marker,
            &RuntimeConfigLease {
                backup_dir: backup_dir.to_path_buf(),
                config_snapshot_dir: None,
                original_config_exists: original.is_some(),
                preserve_provider_route: false,
                protocol_proxy_base_url: None,
                fastctx_command: None,
                subagent_optimization_applied: false,
                subagent_model: String::new(),
                subagent_reasoning_effort: String::new(),
                original_agents_md_exists: false,
                original_default_agent_exists: false,
                original_agents_dir_exists: false,
                provider_id: Some(provider_id.to_string()),
                applied_base_url: Some(applied_base_url.to_string()),
            },
        )
        .unwrap();
    }

    #[test]
    fn official_patch_uses_the_official_endpoint_and_catalog() {
        let result = patch_config(
            "model = \"gpt\"\nmodel_catalog_json = \"old.json\"\n",
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            true,
        )
        .unwrap();
        assert!(result.contains("base_url = \"https://chatgpt.com/backend-api/codex\""));
        assert!(!result.contains("experimental_bearer_token"));
        assert_eq!(
            root_key_string(&result, "model_catalog_json").as_deref(),
            Some("model-catalogs/codey-official.json")
        );
        assert_eq!(root_key_string(&result, "model"), None);
        assert_eq!(
            root_key_string(&result, "service_tier").as_deref(),
            Some("default")
        );
        let document = result.parse::<DocumentMut>().unwrap();
        assert!(
            document["desktop"]["enabled-reasoning-efforts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|effort| effort.as_str() == Some("ultra"))
        );
    }

    #[test]
    fn official_patch_keeps_builtin_openai_without_a_configured_provider_table() {
        let result = patch_config(
            "model_provider = \"openai\"\n",
            &official_profile(),
            BUILTIN_OPENAI_PROVIDER_ID,
            true,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert_eq!(
            document["model_provider"].as_str(),
            Some(BUILTIN_OPENAI_PROVIDER_ID)
        );
        assert!(
            document
                .get("model_providers")
                .and_then(Item::as_table)
                .is_none_or(|providers| providers.get(BUILTIN_OPENAI_PROVIDER_ID).is_none())
        );
        assert_eq!(
            document["model_catalog_json"].as_str(),
            Some("model-catalogs/codey-official.json")
        );
    }

    #[test]
    fn official_patch_keeps_other_builtin_providers_without_overrides() {
        let result = patch_config(
            "model_provider = \"ollama\"\n",
            &official_profile(),
            "ollama",
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert_eq!(document["model_provider"].as_str(), Some("ollama"));
        assert!(
            document
                .get("model_providers")
                .and_then(Item::as_table)
                .is_none_or(|providers| providers.get("ollama").is_none())
        );
    }

    #[test]
    fn provider_patch_enables_all_desktop_reasoning_efforts() {
        let existing = r#"
[desktop]
enabled-reasoning-efforts = ["low", "medium", "high", "xhigh"]
"#;
        let result = patch_config(existing, &official_profile(), GLOBAL_PROVIDER_ID, true).unwrap();
        let document = result.parse::<DocumentMut>().unwrap();
        let efforts = document["desktop"]["enabled-reasoning-efforts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|effort| effort.as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(efforts, ["low", "medium", "high", "xhigh", "max", "ultra"]);
    }

    #[test]
    fn provider_patch_preserves_selected_service_tier() {
        let result = patch_config(
            "service_tier = \"priority\"\n",
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            true,
        )
        .unwrap();

        assert_eq!(
            root_key_string(&result, "service_tier").as_deref(),
            Some("priority")
        );
    }

    #[test]
    fn provider_patch_sets_the_requested_default_model() {
        let result = patch_config_with_fastctx(
            "model = \"old-model\"\n\n[profiles.work]\nmodel = \"profile-model\"\n",
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            Some("gpt-5.6-sol"),
            None,
            false,
        )
        .unwrap();

        assert_eq!(
            root_key_string(&result, "model").as_deref(),
            Some("gpt-5.6-sol")
        );
        let document = result.parse::<DocumentMut>().unwrap();
        let work_profile = document["profiles"]["work"].as_table().unwrap();
        assert!(work_profile.get("model").is_none());
    }

    #[test]
    fn direct_patch_configures_a_responses_provider_without_a_loopback_endpoint() {
        let result = patch_config(
            "model_provider = \"relay\"\n",
            &direct_profile(RelayProtocol::Responses),
            "relay",
            false,
        )
        .unwrap();
        assert!(result.contains("base_url = \"https://relay.example/v1\""));
        assert!(result.contains("wire_api = \"responses\""));
        assert!(result.contains("experimental_bearer_token = \"sk-direct\""));
        assert!(!result.contains("127.0.0.1"));
        assert_eq!(
            root_key_string(&result, "model_provider").as_deref(),
            Some("relay")
        );
    }

    #[test]
    fn direct_patch_rejects_a_reserved_provider_id_instead_of_silently_renaming_it() {
        let error = patch_config(
            "model_provider = \"openai\"\n",
            &direct_profile(RelayProtocol::Responses),
            BUILTIN_OPENAI_PROVIDER_ID,
            false,
        )
        .unwrap_err();

        assert!(error.to_string().contains("Codex 保留 Provider ID"));
    }

    #[test]
    fn direct_chat_patch_requires_the_local_protocol_proxy() {
        let error = patch_config(
            "model_provider = \"openai\"\n",
            &direct_profile(RelayProtocol::ChatCompletions),
            "openai",
            false,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("本地 Responses 协议代理尚未启动")
        );
    }

    #[test]
    fn direct_chat_patch_routes_codex_through_the_local_responses_proxy() {
        let result = patch_config_with_fastctx_mode_and_proxy(
            "model_provider = \"relay\"\n",
            &direct_profile(RelayProtocol::ChatCompletions),
            "relay",
            ProviderPatchOptions {
                model_catalog_path: None,
                default_model: None,
                fastctx_command: None,
                subagent_optimization: false,
                subagent_model: DEFAULT_SUBAGENT_MODEL,
                subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                preserve_provider_route: false,
                protocol_proxy_base_url: Some("http://127.0.0.1:43123/v1"),
            },
        )
        .unwrap();

        assert!(result.contains("base_url = \"http://127.0.0.1:43123/v1\""));
        assert!(result.contains("wire_api = \"responses\""));
        assert!(result.contains("experimental_bearer_token = \"sk-direct\""));
    }

    #[test]
    fn route_preserving_patch_keeps_cc_switch_routing_and_model_fields() {
        let existing = r#"
model_provider = "cc-switch-official"
model = "route-model"
model_catalog_json = "/cc-switch/catalog.json"

[model_providers.cc-switch-official]
name = "CC Switch Proxy"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "responses"
experimental_bearer_token = "PROXY_MANAGED"

[features.cc_switch_owned]
enabled = true
"#;
        let result = patch_config_with_fastctx_mode_and_proxy(
            existing,
            &direct_profile(RelayProtocol::ChatCompletions),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                model_catalog_path: relative_model_catalog_path(),
                default_model: Some("codey-model"),
                fastctx_command: Some(Path::new("/opt/codey")),
                subagent_optimization: true,
                subagent_model: DEFAULT_SUBAGENT_MODEL,
                subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                preserve_provider_route: true,
                protocol_proxy_base_url: None,
            },
        )
        .unwrap();
        let before = parse_document(existing).unwrap();
        let after = parse_document(&result).unwrap();

        assert_eq!(
            root_key_string(&result, "model_provider").as_deref(),
            Some("cc-switch-official")
        );
        assert_eq!(
            root_key_string(&result, "model").as_deref(),
            Some("route-model")
        );
        assert_eq!(
            root_key_string(&result, "model_catalog_json").as_deref(),
            Some("/cc-switch/catalog.json")
        );
        assert!(items_semantically_equal(
            before.get("model_providers").unwrap(),
            after.get("model_providers").unwrap()
        ));
        assert!(
            after["features"]["cc_switch_owned"]["enabled"]
                .as_bool()
                .unwrap()
        );
        assert!(
            after["features"]["multi_agent_v2"]["enabled"]
                .as_bool()
                .unwrap()
        );
        assert!(after["mcp_servers"][CODEY_FASTCTX_SERVER_ID].is_table());
    }

    #[test]
    fn route_preserving_chat_patch_requires_the_local_protocol_proxy() {
        let existing = r#"
model_provider = "cc-switch-live"

[model_providers.cc-switch-live]
name = "CC Switch Live"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "chat"
"#;
        let error = patch_config_with_fastctx_mode_and_proxy(
            existing,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                model_catalog_path: None,
                default_model: None,
                fastctx_command: None,
                subagent_optimization: false,
                subagent_model: DEFAULT_SUBAGENT_MODEL,
                subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                preserve_provider_route: true,
                protocol_proxy_base_url: None,
            },
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("请将 CC Switch Live 线路改为 Responses API")
        );
    }

    #[test]
    fn route_preserving_chat_patch_overlays_only_the_runtime_endpoint() {
        let existing = r#"
model_provider = "cc-switch-live"
model = "deepseek-reasoner"

[model_providers.cc-switch-live]
name = "CC Switch Live"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "chat"
experimental_bearer_token = "PROXY_MANAGED"
"#;
        let result = patch_config_with_fastctx_mode_and_proxy(
            existing,
            &direct_profile(RelayProtocol::ChatCompletions),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                model_catalog_path: None,
                default_model: None,
                fastctx_command: None,
                subagent_optimization: false,
                subagent_model: DEFAULT_SUBAGENT_MODEL,
                subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                preserve_provider_route: true,
                protocol_proxy_base_url: Some("http://127.0.0.1:43123/v1"),
            },
        )
        .unwrap();

        assert_eq!(
            root_key_string(&result, "model_provider").as_deref(),
            Some("cc-switch-live")
        );
        assert_eq!(
            root_key_string(&result, "model").as_deref(),
            Some("deepseek-reasoner")
        );
        assert_eq!(
            provider_base_url(&result, "cc-switch-live").as_deref(),
            Some("http://127.0.0.1:43123/v1")
        );
        assert!(result.contains("wire_api = \"responses\""));
        assert!(result.contains("experimental_bearer_token = \"PROXY_MANAGED\""));
    }

    #[test]
    fn fast_context_tools_status_reports_only_user_configured_servers() {
        let document = parse_document(
            r#"
[mcp_servers.codey_fastctx]
command = "/Applications/Codey.app/Contents/MacOS/codey-fastctx"
args = ["--codey-fastctx-mcp"]

[mcp_servers.context_tools]
command = "uvx"
args = ["fastctx", "--stdio"]
"#,
        )
        .unwrap();

        assert_eq!(
            fast_context_tools_status_from_document(&document),
            FastContextToolsStatus {
                user_configured: true,
                detection_failed: false,
                server_id: Some("context_tools".to_string()),
            }
        );

        let owned_only = parse_document(
            r#"
[mcp_servers.codey_fastctx]
command = "/Applications/Codey.app/Contents/MacOS/codey-fastctx"
args = ["--codey-fastctx-mcp"]
"#,
        )
        .unwrap();
        assert_eq!(
            fast_context_tools_status_from_document(&owned_only),
            FastContextToolsStatus::default()
        );
    }

    #[test]
    fn fast_context_tools_status_reads_the_current_codex_config() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            fast_context_tools_status(temp.path()).unwrap(),
            FastContextToolsStatus::default()
        );

        fs::write(
            temp.path().join("config.toml"),
            r#"[mcp_servers.fastctx]
command = "/custom/fastctx"
args = ["serve"]
"#,
        )
        .unwrap();

        assert_eq!(
            fast_context_tools_status(temp.path()).unwrap(),
            FastContextToolsStatus {
                user_configured: true,
                detection_failed: false,
                server_id: Some("fastctx".to_string()),
            }
        );
    }

    #[test]
    fn fast_context_tools_detect_root_inline_user_servers() {
        let existing = r#"
mcp_servers = { context_tools = { command = "uvx", args = ["fastctx", "--stdio"] } }
"#;
        let document = parse_document(existing).unwrap();

        assert_eq!(
            fast_context_tools_status_from_document(&document),
            FastContextToolsStatus {
                user_configured: true,
                detection_failed: false,
                server_id: Some("context_tools".to_string()),
            }
        );

        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey-fastctx")),
            false,
        )
        .unwrap();
        let document = parse_document(&result).unwrap();
        assert!(!mcp_server_exists(&document, CODEY_FASTCTX_SERVER_ID));
        assert_eq!(
            configured_user_fastctx_server_id(&document).as_deref(),
            Some("context_tools")
        );
    }

    #[test]
    fn disabling_fast_context_tools_removes_inline_owned_servers() {
        for existing in [
            r#"
[mcp_servers]
codey_fastctx = { command = "/tmp/codey-fastctx", args = ["--codey-fastctx-mcp"] }

[features.code_mode]
direct_only_tool_namespaces = ["mcp__codey_fastctx"]
"#,
            r#"
mcp_servers = { codey_fastctx = { command = "/tmp/codey-fastctx", args = ["--codey-fastctx-mcp"] } }

[features.code_mode]
direct_only_tool_namespaces = ["mcp__codey_fastctx"]
"#,
        ] {
            let result = patch_config_with_fastctx(
                existing,
                &official_profile(),
                GLOBAL_PROVIDER_ID,
                relative_model_catalog_path(),
                None,
                None,
                false,
            )
            .unwrap();
            let document = parse_document(&result).unwrap();

            assert!(!mcp_server_exists(&document, CODEY_FASTCTX_SERVER_ID));
            assert!(
                document["features"]["code_mode"]["direct_only_tool_namespaces"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn enabling_fast_context_tools_normalizes_inline_owned_servers() {
        let existing = r#"
mcp_servers = { codey_fastctx = { command = "/old/codey", args = ["--codey-fastctx-mcp"], env = { CUSTOM = "preserve" } } }
"#;
        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/new/codey-fastctx")),
            false,
        )
        .unwrap();
        let document = parse_document(&result).unwrap();
        let server = document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]
            .as_table()
            .unwrap();

        assert_eq!(server["command"].as_str(), Some("/new/codey-fastctx"));
        assert_eq!(server["env"]["CUSTOM"].as_str(), Some("preserve"));
        assert_eq!(
            server["env"]["FASTCTX_TOKEN_BUDGET"].as_str(),
            Some(CODEY_FASTCTX_TOKEN_BUDGET)
        );
        assert_eq!(
            fast_context_tools_status_from_document(&document),
            FastContextToolsStatus::default()
        );
    }

    #[test]
    fn user_fastctx_blocks_the_embedded_server_without_injecting_codey_guidance() {
        let existing = r#"
developer_instructions = "Keep my guidance."
tool_output_token_limit = 16000

[mcp_servers.fastctx]
command = "/custom/fastctx"
args = ["serve", "--enable-shell"]

[features.code_mode]
direct_only_tool_namespaces = ["mcp__existing", "mcp__fastctx"]
"#;
        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/Applications/Codey.app/Contents/MacOS/codey")),
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert_eq!(
            document["mcp_servers"]["fastctx"]["command"].as_str(),
            Some("/custom/fastctx")
        );
        assert!(
            document["mcp_servers"]
                .as_table()
                .unwrap()
                .get(CODEY_FASTCTX_SERVER_ID)
                .is_none()
        );
        assert_eq!(
            document["tool_output_token_limit"].as_integer(),
            Some(16_000)
        );
        let namespaces = document["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__fastctx"))
        );
        assert!(
            namespaces
                .iter()
                .all(|entry| entry.as_str() != Some(CODEY_FASTCTX_NAMESPACE))
        );
        let guidance = document["developer_instructions"].as_str().unwrap();
        assert_eq!(guidance, "Keep my guidance.");
        assert!(!guidance.contains("Codey FastCtx context tools are enabled"));
        assert!(!guidance.contains("mcp__codey_fastctx"));
    }

    #[test]
    fn fast_context_tools_migrate_the_owned_main_executable_proxy_to_the_sidecar() {
        let existing = r#"
[mcp_servers.codey_fastctx]
command = "/Applications/Codey.app/Contents/MacOS/codey"
args = ["--codey-fastctx-mcp"]
startup_timeout_sec = 15
runtime_note = "preserve"

[mcp_servers.codey_fastctx.env]
CONCURRENT = "preserve"
"#;
        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new(
                "/Applications/Codey.app/Contents/MacOS/codey-fastctx",
            )),
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();
        let server = document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]
            .as_table()
            .unwrap();

        assert_eq!(
            server["command"].as_str(),
            Some("/Applications/Codey.app/Contents/MacOS/codey-fastctx")
        );
        assert_eq!(
            server["args"]
                .as_array()
                .and_then(|arguments| arguments.get(0))
                .and_then(Value::as_str),
            Some("--codey-fastctx-mcp")
        );
        assert_eq!(
            server["startup_timeout_sec"].as_integer(),
            Some(CODEY_FASTCTX_STARTUP_TIMEOUT_SECONDS)
        );
        assert_eq!(server["runtime_note"].as_str(), Some("preserve"));
        assert_eq!(server["env"]["CONCURRENT"].as_str(), Some("preserve"));
        assert_eq!(
            server["env"]["FASTCTX_TOKEN_BUDGET"].as_str(),
            Some(CODEY_FASTCTX_TOKEN_BUDGET)
        );
    }

    #[test]
    fn fast_context_tools_detect_fastctx_invoked_by_another_server_id() {
        let existing = r#"
[mcp_servers.context_tools]
command = "uvx"
args = ["fastctx", "--stdio"]
"#;
        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert!(
            document["mcp_servers"]
                .as_table()
                .unwrap()
                .get(CODEY_FASTCTX_SERVER_ID)
                .is_none()
        );
        assert!(document.get("developer_instructions").is_none());
        assert!(document.get("tool_output_token_limit").is_none());
    }

    #[test]
    fn fast_context_tools_detect_fastctx_in_the_command_case_insensitively() {
        let existing = r#"
[mcp_servers]
context_tools = { command = "/opt/tools/FASTCTX.exe", args = ["--stdio"] }
"#;
        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert!(
            document["mcp_servers"]
                .as_table()
                .unwrap()
                .get(CODEY_FASTCTX_SERVER_ID)
                .is_none()
        );
    }

    #[test]
    fn fast_context_tools_do_not_confuse_fastctx_substrings_with_the_server() {
        let existing = r#"
[mcp_servers.breakfastctx]
command = "/custom/breakfastctx"
"#;
        let result = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert_eq!(
            document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]["command"].as_str(),
            Some("/tmp/codey")
        );
    }

    #[test]
    fn disabling_fast_context_tools_removes_only_codey_owned_artifacts() {
        let original = r#"
developer_instructions = "User guidance."
tool_output_token_limit = 16000

[mcp_servers.user_tools]
command = "/custom/context-server"

[features.code_mode]
direct_only_tool_namespaces = ["mcp__existing"]
"#;
        let enabled = patch_config_with_fastctx(
            original,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        let mut stale = enabled.parse::<DocumentMut>().unwrap();
        let guidance = stale["developer_instructions"]
            .as_str()
            .unwrap()
            .to_string();
        let stale_guidance = CODEY_FASTCTX_GUIDANCE_VERSIONS[1..].join("\n\n");
        stale["developer_instructions"] = value(format!(
            "{guidance}\n\n{stale_guidance}\n\nConcurrent guidance."
        ));
        let features = ensure_root_table(&mut stale, "features").unwrap();
        let multi_agent = ensure_child_table(features, "multi_agent_v2").unwrap();
        multi_agent["subagent_developer_instructions"] = value(format!(
            "Subagent guidance.\n\n{guidance}\n\n{stale_guidance}"
        ));

        let disabled = patch_config_with_fastctx(
            &document_string(&stale).unwrap(),
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            None,
            false,
        )
        .unwrap();
        let document = disabled.parse::<DocumentMut>().unwrap();

        let mcp_servers = document["mcp_servers"].as_table().unwrap();
        assert!(mcp_servers.get(CODEY_FASTCTX_SERVER_ID).is_none());
        assert_eq!(
            mcp_servers["user_tools"]["command"].as_str(),
            Some("/custom/context-server")
        );
        assert_eq!(
            document["developer_instructions"].as_str(),
            Some("User guidance.\n\nConcurrent guidance.")
        );
        assert_eq!(
            document["features"]["multi_agent_v2"]["subagent_developer_instructions"].as_str(),
            Some("Subagent guidance.\n\nUser guidance.")
        );
        assert_eq!(
            document["tool_output_token_limit"].as_integer(),
            Some(16_000)
        );
        let namespaces = document["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert_eq!(
            namespaces
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec!["mcp__existing"]
        );
    }

    #[test]
    fn disabling_fast_context_tools_removes_user_fastctx_guidance_only() {
        let user_fastctx_guidance = codey_fastctx_guidance_for_namespace("mcp__fastctx");
        let existing = format!(
            r#"
developer_instructions = "User guidance.\n\n{user_fastctx_guidance}"

[mcp_servers.fastctx]
command = "/custom/fastctx"
args = ["serve"]

[features.code_mode]
direct_only_tool_namespaces = ["mcp__fastctx"]
"#
        );

        let result = patch_config_with_fastctx(
            &existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            None,
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();

        assert_eq!(
            document["mcp_servers"]["fastctx"]["command"].as_str(),
            Some("/custom/fastctx")
        );
        assert_eq!(
            document["developer_instructions"].as_str(),
            Some("User guidance.")
        );
        let namespaces = document["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__fastctx"))
        );
    }

    #[test]
    fn disabling_fast_context_tools_preserves_a_user_replacement_under_the_reserved_id() {
        let existing = format!(
            r#"developer_instructions = "{CODEY_FASTCTX_GUIDANCE}"

[mcp_servers.codey_fastctx]
command = "/user/server"
args = ["serve"]

[features.code_mode]
direct_only_tool_namespaces = ["mcp__codey_fastctx"]
"#
        );
        let disabled = patch_config_with_fastctx(
            &existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            None,
            false,
        )
        .unwrap();
        let document = disabled.parse::<DocumentMut>().unwrap();

        assert_eq!(
            document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]["command"].as_str(),
            Some("/user/server")
        );
        assert!(document.get("developer_instructions").is_none());
        assert!(
            document["features"]["code_mode"]["direct_only_tool_namespaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
        );
    }

    #[test]
    fn disabling_fast_context_tools_preserves_an_unproven_user_namespace() {
        let existing = r#"
[features.code_mode]
direct_only_tool_namespaces = ["mcp__codey_fastctx", "mcp__user"]
"#;
        let disabled = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            None,
            false,
        )
        .unwrap();
        let document = disabled.parse::<DocumentMut>().unwrap();
        let namespaces = document["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();

        assert_eq!(
            namespaces
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec!["mcp__codey_fastctx", "mcp__user"]
        );
    }

    #[test]
    fn fastctx_guidance_cleanup_requires_complete_paragraph_boundaries() {
        let embedded = format!("User prefix {CODEY_FASTCTX_GUIDANCE} user suffix");

        assert_eq!(remove_codey_fastctx_guidance(&embedded), None);
    }

    #[test]
    fn enabling_fast_context_tools_replaces_stale_guidance_versions() {
        let stale_guidance = CODEY_FASTCTX_GUIDANCE_VERSIONS[1..].join("\\n\\n");
        let existing = format!(
            r#"
developer_instructions = "User guidance.\n\n{stale_guidance}\n\n{CODEY_FASTCTX_GUIDANCE}"
"#
        );

        let result = patch_config_with_fastctx(
            &existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();
        let guidance = document["developer_instructions"].as_str().unwrap();

        assert_eq!(
            guidance
                .matches("Codey FastCtx context tools are enabled")
                .count(),
            1
        );
        assert!(guidance.contains(CODEY_FASTCTX_GUIDANCE));
        for stale_guidance in &CODEY_FASTCTX_GUIDANCE_VERSIONS[1..] {
            assert!(!guidance.contains(stale_guidance));
        }
        assert_eq!(
            guidance,
            format!("User guidance.\n\n{CODEY_FASTCTX_GUIDANCE}")
        );
    }

    #[test]
    fn fast_context_tools_are_idempotent_and_default_the_host_output_limit() {
        let existing = r#"
[features.code_mode]
direct_only_tool_namespaces = ["mcp__existing", "mcp__codey_fastctx", "mcp__codey_fastctx"]
"#;
        let first = patch_config_with_fastctx(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        let second = patch_config_with_fastctx(
            &first,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            relative_model_catalog_path(),
            None,
            Some(Path::new("/tmp/codey")),
            false,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.matches(CODEY_FASTCTX_GUIDANCE).count(), 1);
        let document = first.parse::<DocumentMut>().unwrap();
        let guidance = document["developer_instructions"].as_str().unwrap();
        assert!(guidance.contains("tools.mcp__codey_fastctx__inspect_local_file"));
        assert!(guidance.contains("available inside a code-mode program"));
        assert!(guidance.contains("drive-letter path such as `E:/repo/file.ts`"));
        assert!(!guidance.contains("list_mcp_resources"));
        assert!(!guidance.contains("read_mcp_resource"));
        assert!(!guidance.contains("Write-Output"));
        for stale_guidance in &CODEY_FASTCTX_GUIDANCE_VERSIONS[1..] {
            assert!(!guidance.contains(stale_guidance));
        }
        assert_eq!(
            document["features"]["code_mode"]["direct_only_tool_namespaces"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec!["mcp__existing"]
        );
        assert_eq!(
            document["tool_output_token_limit"].as_integer(),
            Some(10_000)
        );
    }

    #[test]
    fn subagent_optimization_enables_v2_and_removes_legacy_agents() {
        let existing = r#"
[agents]
max_threads = 6
max_depth = 1
interrupt_message = true
custom_setting = "preserved"

[features.multi_agent_v2]
enabled = false
custom_setting = "preserved"
subagent_developer_instructions = "Preserve my subagent guidance."
root_agent_usage_hint_text = "Preserve my root usage hint."
"#;
        let result = patch_config_with_fastctx_mode_and_proxy(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                model_catalog_path: relative_model_catalog_path(),
                default_model: None,
                fastctx_command: None,
                subagent_optimization: true,
                subagent_model: "gpt-5.6-sol",
                subagent_reasoning_effort: "high",
                preserve_provider_route: false,
                protocol_proxy_base_url: None,
            },
        )
        .unwrap();
        let document = result.parse::<DocumentMut>().unwrap();
        let agents = document["agents"].as_table().unwrap();
        let multi_agent = document["features"]["multi_agent_v2"].as_table().unwrap();

        assert!(agents.get("max_threads").is_none());
        assert!(agents.get("max_depth").is_none());
        assert!(agents.get("interrupt_message").is_none());
        assert_eq!(agents["custom_setting"].as_str(), Some("preserved"));
        assert_eq!(
            agents["default_subagent_model"].as_str(),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            agents["default_subagent_reasoning_effort"].as_str(),
            Some("high")
        );
        assert_eq!(multi_agent["enabled"].as_bool(), Some(true));
        assert_eq!(multi_agent["wait_agent_enabled"].as_bool(), Some(true));
        assert_eq!(
            multi_agent["hide_spawn_agent_metadata"].as_bool(),
            Some(true)
        );
        assert_eq!(
            multi_agent["expose_spawn_agent_model_overrides"].as_bool(),
            Some(false)
        );
        assert_eq!(multi_agent["tool_namespace"].as_str(), Some("agents"));
        assert_eq!(
            multi_agent["max_concurrent_threads_per_session"].as_integer(),
            Some(7)
        );
        assert_eq!(
            multi_agent["min_wait_timeout_ms"].as_integer(),
            Some(10_000)
        );
        assert_eq!(
            multi_agent["default_wait_timeout_ms"].as_integer(),
            Some(30_000)
        );
        assert_eq!(
            multi_agent["max_wait_timeout_ms"].as_integer(),
            Some(120_000)
        );
        assert_eq!(multi_agent["custom_setting"].as_str(), Some("preserved"));
        assert_eq!(
            multi_agent["subagent_developer_instructions"].as_str(),
            Some("Preserve my subagent guidance.")
        );
        let root_usage_hint = multi_agent["root_agent_usage_hint_text"].as_str().unwrap();
        assert!(root_usage_hint.contains("Preserve my root usage hint."));
        assert!(root_usage_hint.contains(ROOT_AGENT_COLLABORATION_USAGE_HINT));
    }

    #[test]
    fn subagent_optimization_accepts_dynamic_model_ids_and_rejects_empty_values() {
        let patched = patch_config_with_fastctx_mode_and_proxy(
            "",
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                model_catalog_path: relative_model_catalog_path(),
                default_model: None,
                fastctx_command: None,
                subagent_optimization: true,
                subagent_model: "gpt-5.6-luna",
                subagent_reasoning_effort: "high",
                preserve_provider_route: false,
                protocol_proxy_base_url: None,
            },
        )
        .unwrap();
        let document = patched.parse::<DocumentMut>().unwrap();
        assert_eq!(
            document["agents"]["default_subagent_model"].as_str(),
            Some("gpt-5.6-luna")
        );

        let error = patch_config_with_fastctx_mode_and_proxy(
            "",
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                model_catalog_path: relative_model_catalog_path(),
                default_model: None,
                fastctx_command: None,
                subagent_optimization: true,
                subagent_model: "   ",
                subagent_reasoning_effort: "high",
                preserve_provider_route: false,
                protocol_proxy_base_url: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("子代理模型不能为空"));
    }

    #[test]
    fn subagent_lease_applies_and_restores_all_owned_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(home.join("agents")).unwrap();
        let original_config = b"model_provider = \"codey_global\"\n\n[agents]\nmax_threads = 3\n\n[model_providers.codey_global]\nbase_url = \"https://chatgpt.com/backend-api/codex\"\n";
        let original_agents_md = b"# Existing guidance\n\nKeep this verbatim.\n";
        let original_default_agent = b"name = \"custom\"\nmodel = \"custom-model\"\n";
        fs::write(home.join("config.toml"), original_config).unwrap();
        fs::write(home.join("AGENTS.md"), original_agents_md).unwrap();
        fs::write(home.join("agents/default.toml"), original_default_agent).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let temporary_config = fs::read_to_string(home.join("config.toml")).unwrap();
        let document = temporary_config.parse::<DocumentMut>().unwrap();
        assert_eq!(
            document["agents"]["default_subagent_model"].as_str(),
            Some(DEFAULT_SUBAGENT_MODEL)
        );
        assert_eq!(
            document["agents"]["default_subagent_reasoning_effort"].as_str(),
            Some(DEFAULT_SUBAGENT_REASONING_EFFORT)
        );
        assert_eq!(
            document["model_catalog_json"].as_str(),
            Some(
                home.join(crate::model_catalog::relative_path())
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert_eq!(
            document["features"]["multi_agent_v2"]["tool_namespace"].as_str(),
            Some("agents")
        );
        assert_eq!(
            document["features"]["multi_agent_v2"]["wait_agent_enabled"].as_bool(),
            Some(true)
        );
        assert_eq!(
            document["features"]["multi_agent_v2"]["root_agent_usage_hint_text"].as_str(),
            Some(ROOT_AGENT_COLLABORATION_USAGE_HINT)
        );
        assert!(
            fs::read_to_string(home.join("AGENTS.md"))
                .unwrap()
                .contains(SUBAGENT_GUIDANCE)
        );
        assert_eq!(
            fs::read_to_string(home.join("agents/default.toml")).unwrap(),
            DEFAULT_AGENT_CONFIG
        );

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(fs::read(home.join("config.toml")).unwrap(), original_config);
        assert_eq!(
            fs::read(home.join("AGENTS.md")).unwrap(),
            original_agents_md
        );
        assert_eq!(
            fs::read(home.join("agents/default.toml")).unwrap(),
            original_default_agent
        );
        assert!(!marker.exists());
    }

    #[test]
    fn user_fastctx_does_not_inject_codey_guidance_into_subagents() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let original_config = r#"model_provider = "codey_global"

[model_providers.codey_global]
base_url = "https://chatgpt.com/backend-api/codex"

[mcp_servers.fastctx]
command = "/custom/fastctx"
args = ["serve"]
"#;
        fs::write(home.join("config.toml"), original_config).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let temporary_config = fs::read_to_string(home.join("config.toml")).unwrap();
        let document = temporary_config.parse::<DocumentMut>().unwrap();
        assert!(
            document["mcp_servers"]
                .as_table()
                .unwrap()
                .get(CODEY_FASTCTX_SERVER_ID)
                .is_none()
        );
        let default_agent = fs::read_to_string(home.join("agents/default.toml")).unwrap();
        assert_eq!(default_agent, DEFAULT_AGENT_CONFIG);
        assert!(default_agent.contains("每次工具调用都必须推进任务本身"));
        assert!(document.get("developer_instructions").is_none());
        assert!(
            document["features"]["multi_agent_v2"]
                .as_table()
                .unwrap()
                .get("subagent_developer_instructions")
                .is_none()
        );

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original_config
        );
        assert!(!home.join("agents/default.toml").exists());
    }

    #[test]
    fn previous_fastctx_guidance_is_migrated_before_the_runtime_lease() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(home.join("agents")).unwrap();
        let dynamic_previous = PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3
            .replace(CODEY_FASTCTX_NAMESPACE, "mcp__legacy_fastctx");
        let original_config = format!(
            r#"model_provider = "codey_global"
developer_instructions = {}

[model_providers.codey_global]
base_url = "https://chatgpt.com/backend-api/codex"

[features.multi_agent_v2]
subagent_developer_instructions = {}
"#,
            Value::from(format!(
                "Root user guidance.\n\n{PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3}"
            )),
            Value::from(format!("Subagent user guidance.\n\n{dynamic_previous}")),
        );
        let original_default_agent = format!(
            r#"name = "custom"
developer_instructions = {}
"#,
            Value::from(format!(
                "Default user guidance.\n\n{PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3}"
            )),
        );
        fs::write(home.join("config.toml"), original_config).unwrap();
        fs::write(home.join("agents/default.toml"), original_default_agent).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let temporary_config = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(temporary_config.contains(CODEY_FASTCTX_GUIDANCE));
        assert!(!temporary_config.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3));
        let temporary_default = fs::read_to_string(home.join("agents/default.toml")).unwrap();
        assert!(temporary_default.contains(CODEY_FASTCTX_GUIDANCE));
        assert!(!temporary_default.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3));

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored_config = fs::read_to_string(home.join("config.toml")).unwrap();
        let restored_document = parse_document(&restored_config).unwrap();
        assert_eq!(
            restored_document["developer_instructions"].as_str(),
            Some("Root user guidance.")
        );
        assert_eq!(
            restored_document["features"]["multi_agent_v2"]["subagent_developer_instructions"]
                .as_str(),
            Some("Subagent user guidance.")
        );
        assert!(!restored_config.contains(CODEY_FASTCTX_GUIDANCE));
        assert!(!restored_config.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3));

        let restored_default = fs::read_to_string(home.join("agents/default.toml")).unwrap();
        let restored_default_document = parse_document(&restored_default).unwrap();
        assert_eq!(
            restored_default_document["developer_instructions"].as_str(),
            Some("Default user guidance.")
        );
        assert!(!restored_default.contains(CODEY_FASTCTX_GUIDANCE));
        assert!(!restored_default.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V3));
    }

    #[test]
    fn hot_reloaded_subagent_defaults_are_adopted_by_runtime_lease() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let original_config = b"model_provider = \"codey_global\"\n\n[agents]\ncustom = \"keep\"\n\n[model_providers.codey_global]\nbase_url = \"https://chatgpt.com/backend-api/codex\"\n";
        fs::write(home.join("config.toml"), original_config).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let mut current = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        current["agents"]["default_subagent_model"] = value("provider-coder");
        current["agents"]["default_subagent_reasoning_effort"] = value("high");
        fs::write(home.join("config.toml"), document_string(&current).unwrap()).unwrap();

        mark_runtime_subagent_defaults_applied_at(&home, &marker, "provider-coder", "high")
            .unwrap();

        let state =
            serde_json::from_str::<RuntimeConfigLease>(&fs::read_to_string(&marker).unwrap())
                .unwrap();
        assert_eq!(state.subagent_model, "provider-coder");
        assert_eq!(state.subagent_reasoning_effort, "high");
        let snapshot_dir = state
            .config_snapshot_dir
            .as_deref()
            .unwrap_or(&state.backup_dir);
        let applied = fs::read_to_string(snapshot_dir.join(APPLIED_CONFIG_FILE))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            applied["agents"]["default_subagent_model"].as_str(),
            Some("provider-coder")
        );
        assert_eq!(
            applied["agents"]["default_subagent_reasoning_effort"].as_str(),
            Some("high")
        );

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(fs::read(home.join("config.toml")).unwrap(), original_config);
    }

    #[test]
    fn subagent_lease_preserves_concurrent_user_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "model_provider = \"codey_global\"\n",
        )
        .unwrap();
        fs::write(home.join("AGENTS.md"), "# Original\n").unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();
        let mut concurrent_agents_md = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        concurrent_agents_md.push_str("\n## User addition\nKeep this too.\n");
        fs::write(home.join("AGENTS.md"), concurrent_agents_md).unwrap();
        fs::write(
            home.join("agents/default.toml"),
            "name = \"user-replacement\"\n",
        )
        .unwrap();

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored_agents_md = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        assert!(restored_agents_md.contains("# Original"));
        assert!(restored_agents_md.contains("## User addition"));
        assert!(!restored_agents_md.contains(SUBAGENT_GUIDANCE));
        assert_eq!(
            fs::read_to_string(home.join("agents/default.toml")).unwrap(),
            "name = \"user-replacement\"\n"
        );
    }

    #[test]
    fn subagent_lease_removes_runtime_only_files_on_restore() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "model_provider = \"codey_global\"\n",
        )
        .unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();
        assert!(home.join("AGENTS.md").exists());
        assert!(home.join("agents/default.toml").exists());

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert!(!home.join("AGENTS.md").exists());
        assert!(!home.join("agents/default.toml").exists());
        assert!(!home.join("agents").exists());
    }

    #[test]
    fn subagent_lease_restores_owned_files_after_a_provider_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "model_provider = \"codey_global\"\n",
        )
        .unwrap();
        let original_agents_md = b"# Original guidance\n";
        fs::write(home.join("AGENTS.md"), original_agents_md).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                subagent_optimization: true,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();
        let replacement_config = b"model_provider = \"user-provider\"\n\n[model_providers.user-provider]\nbase_url = \"https://user.example/v1\"\n";
        fs::write(home.join("config.toml"), replacement_config).unwrap();

        assert!(!restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(
            fs::read(home.join("config.toml")).unwrap(),
            replacement_config
        );
        assert_eq!(
            fs::read(home.join("AGENTS.md")).unwrap(),
            original_agents_md
        );
        assert!(!home.join("agents/default.toml").exists());
        assert!(!marker.exists());
    }

    #[test]
    fn non_route_lease_preserves_a_user_route_change_and_removes_owned_overlay() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            format!(
                "model_provider = \"{GLOBAL_PROVIDER_ID}\"\n\n\
                 [model_providers.{GLOBAL_PROVIDER_ID}]\n\
                 base_url = \"{CHATGPT_CODEX_BASE_URL}\"\n"
            ),
        )
        .unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();
        let mut current = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        current["model_providers"][GLOBAL_PROVIDER_ID]["base_url"] =
            value("https://user.example/v1");
        fs::write(home.join("config.toml"), document_string(&current).unwrap()).unwrap();

        assert!(!restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            restored["model_providers"][GLOBAL_PROVIDER_ID]["base_url"].as_str(),
            Some("https://user.example/v1")
        );
        assert!(restored.get("model_catalog_json").is_none());
        assert!(restored.get("service_tier").is_none());
        assert!(restored.get("developer_instructions").is_none());
        assert!(
            restored
                .get("mcp_servers")
                .and_then(Item::as_table)
                .and_then(|servers| servers.get(CODEY_FASTCTX_SERVER_ID))
                .is_none()
        );
        assert!(!marker.exists());
    }

    #[test]
    fn lease_restores_the_exact_original_config() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let original = b"model_provider = \"codey_global\"\n\n[model_providers.codey_global]\nbase_url = \"https://chatgpt.com/backend-api/codex\"\n";
        fs::write(home.join("config.toml"), original).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions::for_test(&marker, &backup_root),
        )
        .unwrap();
        let temporary = fs::read_to_string(home.join("config.toml")).unwrap();
        assert_eq!(
            provider_base_url(&temporary, GLOBAL_PROVIDER_ID).as_deref(),
            Some("https://relay.example/v1")
        );
        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(fs::read(home.join("config.toml")).unwrap(), original);
        assert!(!marker.exists());
    }

    #[test]
    fn route_apply_rejects_a_config_changed_after_live_snapshot_validation() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let expected = b"model_provider = \"route-a\"\n";
        let current = b"model_provider = \"route-b\"\n";
        fs::write(home.join("config.toml"), current).unwrap();

        let error = apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            "route-a",
            ProviderApplyOptions {
                preserve_provider_route: true,
                expected_config: Some(expected),
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("启动准备期间发生变化"));
        assert_eq!(fs::read(home.join("config.toml")).unwrap(), current);
        assert!(!marker.exists());
    }

    #[test]
    fn route_lease_restores_latest_cc_switch_route_before_restart() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let route_a = r#"
model_provider = "route-a"
model = "model-a"
model_catalog_json = "/cc-switch/catalog-a.json"

[model_providers.route-a]
name = "Route A"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "responses"
experimental_bearer_token = "PROXY_MANAGED"

[mcp_servers.cc-switch]
command = "cc-switch-tool"
"#;
        fs::write(home.join("config.toml"), route_a).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            "route-a",
            ProviderApplyOptions {
                use_official_catalog: false,
                default_model: None,
                fastctx_command: Some(Path::new("/opt/codey")),
                subagent_optimization: false,
                subagent_model: DEFAULT_SUBAGENT_MODEL,
                subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                marker: &marker,
                backup_root: &backup_root,
                preserve_provider_route: true,
                protocol_proxy_base_url: None,
                expected_config: None,
            },
        )
        .unwrap();
        let applied_a = fs::read_to_string(home.join("config.toml")).unwrap();
        assert_eq!(
            root_key_string(&applied_a, "model_provider").as_deref(),
            Some("route-a")
        );
        assert_eq!(
            root_key_string(&applied_a, "model").as_deref(),
            Some("model-a")
        );

        let route_b = r#"
model_provider = "route-b"
model = "model-b"
model_catalog_json = "/cc-switch/catalog-b.json"
cc_switch_generation = 2

[model_providers.route-b]
name = "Route B"
base_url = "http://localhost:15721/v1"
wire_api = "responses"
experimental_bearer_token = "PROXY_MANAGED"

[mcp_servers.cc-switch]
command = "cc-switch-tool-v2"
"#;
        fs::write(home.join("config.toml"), route_b).unwrap();

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored = fs::read_to_string(home.join("config.toml")).unwrap();
        let expected = parse_document(route_b).unwrap();
        let actual = parse_document(&restored).unwrap();
        assert!(tables_semantically_equal(
            expected.as_table(),
            actual.as_table()
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn chat_route_lease_removes_protocol_proxy_from_latest_route_before_restart() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let route_a = r#"
model_provider = "route-a"
model = "deepseek-chat"

[model_providers.route-a]
name = "Route A"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "chat"
experimental_bearer_token = "PROXY_MANAGED"
"#;
        fs::write(home.join("config.toml"), route_a).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::ChatCompletions),
            "route-a",
            ProviderApplyOptions {
                use_official_catalog: false,
                default_model: None,
                fastctx_command: None,
                subagent_optimization: false,
                subagent_model: DEFAULT_SUBAGENT_MODEL,
                subagent_reasoning_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                marker: &marker,
                backup_root: &backup_root,
                preserve_provider_route: true,
                protocol_proxy_base_url: Some("http://127.0.0.1:43123/v1"),
                expected_config: None,
            },
        )
        .unwrap();
        let applied_a = fs::read_to_string(home.join("config.toml")).unwrap();
        assert_eq!(
            provider_base_url(&applied_a, "route-a").as_deref(),
            Some("http://127.0.0.1:43123/v1")
        );
        assert!(applied_a.contains("wire_api = \"responses\""));

        let route_b = r#"
model_provider = "route-b"
model = "qwen-coder"

[model_providers.route-b]
name = "Route B"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "chat"
experimental_bearer_token = "PROXY_MANAGED"
"#;
        fs::write(home.join("config.toml"), route_b).unwrap();

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored =
            parse_document(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
        let expected = parse_document(route_b).unwrap();
        assert!(tables_semantically_equal(
            expected.as_table(),
            restored.as_table()
        ));
    }

    #[test]
    fn first_direct_runtime_lease_preserves_chatgpt_auth_json() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let auth = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"free-account-token"}}"#;
        fs::write(home.join("auth.json"), auth).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                use_official_catalog: false,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let temporary = fs::read_to_string(home.join("config.toml")).unwrap();
        assert_eq!(
            provider_base_url(&temporary, GLOBAL_PROVIDER_ID).as_deref(),
            Some("https://relay.example/v1")
        );
        assert!(temporary.contains("experimental_bearer_token = \"sk-direct\""));
        assert!(!temporary.contains("base_url = \"https://chatgpt.com/backend-api/codex\""));
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert!(!home.join("config.toml").exists());
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);
    }

    #[test]
    fn manual_local_provider_settings_survive_the_runtime_lease() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        let original = br#"model_provider = "manual"

[model_providers.manual]
name = "Manual Relay"
base_url = "https://manual.example/v1"
wire_api = "responses"
requires_openai_auth = false
env_key = "MANUAL_RELAY_API_KEY"
request_max_retries = 7

[model_providers.manual.http_headers]
X-Route = "manual"
"#;
        let auth = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"free-account-token"}}"#;
        fs::write(home.join("config.toml"), original).unwrap();
        fs::write(home.join("auth.json"), auth).unwrap();
        let mut profile = ProviderProfile::new("Manual Relay");
        profile.id = "manual".to_string();
        profile.base_url = "https://manual.example/v1".to_string();

        apply_runtime_provider_config_at_mode(
            &home,
            &profile,
            "manual",
            ProviderApplyOptions {
                use_official_catalog: false,
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let temporary = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let provider = temporary["model_providers"]["manual"].as_table().unwrap();
        assert_eq!(
            provider["base_url"].as_str(),
            Some("https://manual.example/v1")
        );
        assert_eq!(provider["requires_openai_auth"].as_bool(), Some(false));
        assert_eq!(provider["env_key"].as_str(), Some("MANUAL_RELAY_API_KEY"));
        assert_eq!(provider["request_max_retries"].as_integer(), Some(7));
        assert_eq!(provider["http_headers"]["X-Route"].as_str(), Some("manual"));
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(fs::read(home.join("config.toml")).unwrap(), original);
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);
    }

    #[test]
    fn legacy_lease_reverts_owned_fields_without_overwriting_concurrent_config() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_dir = temp.path().join("codey/codex-backups/legacy");
        fs::create_dir_all(&home).unwrap();
        let original = r#"model_provider = "openai"
model = "gpt-original"
model_catalog_json = "user-catalog.json"
profile = "work"
developer_instructions = "Original guidance"

[model_providers.codey_global]
name = "Original provider"
base_url = "https://chatgpt.com/backend-api/codex"
wire_api = "responses"
requires_openai_auth = true
custom_original = "restore"

[desktop]
enabled-reasoning-efforts = ["medium"]

[profiles.work]
model = "profile-original"

[mcp_servers.codey_fastctx]
command = "/user/server"
args = ["serve"]
custom_original = "restore"

[features.code_mode]
direct_only_tool_namespaces = ["mcp__existing"]
"#;
        let stale_guidance = CODEY_FASTCTX_GUIDANCE_VERSIONS[1..].join("\\n\\n");
        let current = format!(
            r#"model_provider = "codey_global"
model_catalog_json = "model-catalogs/codey-official.json"
profile = "work"
developer_instructions = "Original guidance\n\n{stale_guidance}\n\n{CODEY_FASTCTX_GUIDANCE}\n\nConcurrent guidance"
tool_output_token_limit = 10000
approval_policy = "never"
service_tier = "fast"

[model_providers.codey_global]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "chat"
requires_openai_auth = true
experimental_bearer_token = "sk-temporary"
runtime_note = "preserve"

[desktop]
enabled-reasoning-efforts = ["low", "medium", "high", "xhigh"]

[profiles.work]
approval_policy = "never"

[mcp_servers.codey_fastctx]
command = "/Applications/Codey.app/Contents/MacOS/codey"
args = ["--codey-fastctx-mcp"]
startup_timeout_sec = 120
tool_timeout_sec = 120
runtime_note = "preserve"

[mcp_servers.codey_fastctx.env]
FASTCTX_TOKEN_BUDGET = "8500"
CONCURRENT = "preserve"

[features.code_mode]
direct_only_tool_namespaces = ["mcp__existing", "mcp__codey_fastctx", "mcp__concurrent"]

[marketplaces.openai-bundled]
last_updated = "new"
"#
        );
        fs::write(home.join("config.toml"), current).unwrap();
        write_legacy_runtime_lease(
            &marker,
            &backup_dir,
            Some(original),
            GLOBAL_PROVIDER_ID,
            "https://relay.example/v1",
        );

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();

        assert_eq!(restored["model_provider"].as_str(), Some("openai"));
        assert_eq!(restored["model"].as_str(), Some("gpt-original"));
        assert_eq!(
            restored["model_catalog_json"].as_str(),
            Some("user-catalog.json")
        );
        assert_eq!(
            restored["developer_instructions"].as_str(),
            Some("Original guidance\n\nConcurrent guidance")
        );
        assert!(restored.get("tool_output_token_limit").is_none());
        assert_eq!(restored["approval_policy"].as_str(), Some("never"));
        assert_eq!(restored["service_tier"].as_str(), Some("fast"));

        let provider = restored["model_providers"][GLOBAL_PROVIDER_ID]
            .as_table()
            .unwrap();
        assert_eq!(provider["name"].as_str(), Some("Original provider"));
        assert_eq!(provider["base_url"].as_str(), Some(CHATGPT_CODEX_BASE_URL));
        assert_eq!(provider["wire_api"].as_str(), Some("responses"));
        assert!(provider.get("experimental_bearer_token").is_none());
        assert_eq!(provider["custom_original"].as_str(), Some("restore"));
        assert_eq!(provider["runtime_note"].as_str(), Some("preserve"));

        let efforts = restored["desktop"]["enabled-reasoning-efforts"]
            .as_array()
            .unwrap();
        assert_eq!(
            efforts.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
            vec!["medium"]
        );
        assert_eq!(
            restored["profiles"]["work"]["model"].as_str(),
            Some("profile-original")
        );
        assert_eq!(
            restored["profiles"]["work"]["approval_policy"].as_str(),
            Some("never")
        );

        let fastctx = restored["mcp_servers"][CODEY_FASTCTX_SERVER_ID]
            .as_table()
            .unwrap();
        assert_eq!(fastctx["command"].as_str(), Some("/user/server"));
        assert_eq!(fastctx["args"][0].as_str(), Some("serve"));
        assert!(fastctx.get("startup_timeout_sec").is_none());
        assert!(fastctx.get("tool_timeout_sec").is_none());
        assert_eq!(fastctx["custom_original"].as_str(), Some("restore"));
        assert_eq!(fastctx["runtime_note"].as_str(), Some("preserve"));
        assert!(fastctx["env"].get("FASTCTX_TOKEN_BUDGET").is_none());
        assert_eq!(fastctx["env"]["CONCURRENT"].as_str(), Some("preserve"));

        let namespaces = restored["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert!(
            namespaces
                .iter()
                .all(|entry| entry.as_str() != Some(CODEY_FASTCTX_NAMESPACE))
        );
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__existing"))
        );
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__concurrent"))
        );
        assert_eq!(
            restored["marketplaces"]["openai-bundled"]["last_updated"].as_str(),
            Some("new")
        );
        assert!(!marker.exists());
    }

    #[test]
    fn legacy_lease_preserves_a_new_user_config_when_no_original_existed() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_dir = temp.path().join("codey/codex-backups/legacy");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"model_provider = "codey_global"
approval_policy = "never"

[model_providers.codey_global]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-temporary"

[plugins.browser]
enabled = true
"#,
        )
        .unwrap();
        write_legacy_runtime_lease(
            &marker,
            &backup_dir,
            None,
            GLOBAL_PROVIDER_ID,
            "https://relay.example/v1",
        );

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert!(restored.get("model_provider").is_none());
        assert!(restored.get("model_providers").is_none());
        assert_eq!(restored["approval_policy"].as_str(), Some("never"));
        assert_eq!(
            restored["plugins"]["browser"]["enabled"].as_bool(),
            Some(true)
        );
        assert!(!marker.exists());
    }

    #[test]
    fn legacy_lease_removes_a_runtime_only_config_when_no_original_existed() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_dir = temp.path().join("codey/codex-backups/legacy");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"model_provider = "codey_global"

[model_providers.codey_global]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-temporary"
"#,
        )
        .unwrap();
        write_legacy_runtime_lease(
            &marker,
            &backup_dir,
            None,
            GLOBAL_PROVIDER_ID,
            "https://relay.example/v1",
        );

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert!(!home.join("config.toml").exists());
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn lease_snapshots_use_private_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "model_provider = \"codey_global\"\n",
        )
        .unwrap();

        let backup_dir = apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions::for_test(&marker, &backup_root),
        )
        .unwrap();

        for path in [&backup_root, &backup_dir] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700,
                "{} should only be accessible by its owner",
                path.display()
            );
        }
        for path in [
            backup_dir.join("config.toml"),
            backup_dir.join(APPLIED_CONFIG_FILE),
            marker,
            home.join("config.toml"),
        ] {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "{} should only be readable and writable by its owner",
                path.display()
            );
        }
    }

    #[test]
    fn lease_preserves_concurrent_codex_updates_while_reverting_codey_fields() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"model_provider = "codey_global"
model = "gpt-old"

[model_providers.codey_global]
name = "Original"
base_url = "https://chatgpt.com/backend-api/codex"

[marketplaces.openai-bundled]
last_updated = "old"

[features.code_mode]
direct_only_tool_namespaces = ["mcp__existing", "mcp__codey_fastctx"]
"#,
        )
        .unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions {
                fastctx_command: Some(Path::new("/tmp/codey")),
                ..ProviderApplyOptions::for_test(&marker, &backup_root)
            },
        )
        .unwrap();

        let mut current = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let applied_namespaces = current["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert!(
            applied_namespaces
                .iter()
                .all(|entry| entry.as_str() != Some(CODEY_FASTCTX_NAMESPACE))
        );
        assert!(
            applied_namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__existing"))
        );
        current["model"] = value("gpt-new");
        current["service_tier"] = value("fast");
        current["developer_instructions"] = value(format!(
            "{}\n\nKeep concurrent guidance.",
            current["developer_instructions"].as_str().unwrap()
        ));
        current["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array_mut()
            .unwrap()
            .push("mcp__concurrent");
        current["mcp_servers"][CODEY_FASTCTX_SERVER_ID]["runtime_note"] = value("concurrent field");
        let marketplaces = ensure_root_table(&mut current, "marketplaces").unwrap();
        let mut bundled = Table::new();
        bundled["last_updated"] = value("new");
        marketplaces["openai-bundled"] = Item::Table(bundled);
        let plugins = ensure_root_table(&mut current, "plugins").unwrap();
        let mut browser = Table::new();
        browser["enabled"] = value(true);
        plugins["browser@openai-bundled"] = Item::Table(browser);
        atomic_write(
            &home.join("config.toml"),
            document_string(&current).unwrap().as_bytes(),
        )
        .unwrap();

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let restored = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();

        assert_eq!(restored["model"].as_str(), Some("gpt-new"));
        assert_eq!(restored["service_tier"].as_str(), Some("fast"));
        assert_eq!(
            restored["developer_instructions"].as_str(),
            Some("Keep concurrent guidance.")
        );
        let namespaces = restored["features"]["code_mode"]["direct_only_tool_namespaces"]
            .as_array()
            .unwrap();
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
        );
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__existing"))
        );
        assert!(
            namespaces
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__concurrent"))
        );
        assert_eq!(
            restored["marketplaces"]["openai-bundled"]["last_updated"].as_str(),
            Some("new")
        );
        assert_eq!(
            restored["plugins"]["browser@openai-bundled"]["enabled"].as_bool(),
            Some(true)
        );
        assert_eq!(
            restored["model_providers"][GLOBAL_PROVIDER_ID]["base_url"].as_str(),
            Some(CHATGPT_CODEX_BASE_URL)
        );
        assert!(restored.get("model_catalog_json").is_none());
        assert!(
            restored
                .get("mcp_servers")
                .and_then(Item::as_table)
                .and_then(|servers| servers.get(CODEY_FASTCTX_SERVER_ID))
                .is_none()
        );
        assert!(!marker.exists());
    }

    #[test]
    fn restore_preserves_a_concurrent_replacement_of_the_reserved_fastctx_server() {
        let applied = r#"
[mcp_servers.codey_fastctx]
command = "/Applications/Codey.app/Contents/MacOS/codey"
args = ["--codey-fastctx-mcp"]
startup_timeout_sec = 15
"#;
        let current = r#"
[mcp_servers.codey_fastctx]
command = "/custom/server"
args = ["serve"]
note = "user replacement"
"#;

        let restored = restore_owned_config_changes("", applied, current)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();

        assert_eq!(
            restored["mcp_servers"][CODEY_FASTCTX_SERVER_ID]["command"].as_str(),
            Some("/custom/server")
        );
        assert_eq!(
            restored["mcp_servers"][CODEY_FASTCTX_SERVER_ID]["args"][0].as_str(),
            Some("serve")
        );
        assert_eq!(
            restored["mcp_servers"][CODEY_FASTCTX_SERVER_ID]["note"].as_str(),
            Some("user replacement")
        );
    }

    #[test]
    fn restore_removes_dynamic_user_fastctx_guidance_after_concurrent_edits() {
        let user_fastctx_guidance = codey_fastctx_guidance_for_namespace("mcp__fastctx");
        let original = r#"developer_instructions = "User guidance""#;
        let applied =
            format!(r#"developer_instructions = "User guidance\n\n{user_fastctx_guidance}""#);
        let current = format!(
            r#"developer_instructions = "User guidance\n\n{user_fastctx_guidance}\n\nConcurrent guidance""#
        );

        let restored = restore_owned_config_changes(original, &applied, &current)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();

        assert_eq!(
            restored["developer_instructions"].as_str(),
            Some("User guidance\n\nConcurrent guidance")
        );
    }

    #[test]
    fn restore_removes_root_agent_hint_after_concurrent_user_edits() {
        let config_with_hint = |hint: &str| {
            format!(
                "[features.multi_agent_v2]\nroot_agent_usage_hint_text = {}\n",
                Value::from(hint)
            )
        };
        let original_hint = "User root hint.";
        let applied_hint = format!("{original_hint}\n\n{ROOT_AGENT_COLLABORATION_USAGE_HINT}");
        let current_hint = format!("{applied_hint}\n\nConcurrent root hint.");

        let restored = restore_owned_config_changes(
            &config_with_hint(original_hint),
            &config_with_hint(&applied_hint),
            &config_with_hint(&current_hint),
        )
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(
            restored["features"]["multi_agent_v2"]["root_agent_usage_hint_text"].as_str(),
            Some("User root hint.\n\nConcurrent root hint.")
        );

        let applied = config_with_hint(ROOT_AGENT_COLLABORATION_USAGE_HINT);
        let current = config_with_hint(&format!(
            "{ROOT_AGENT_COLLABORATION_USAGE_HINT}\n\nConcurrent root hint."
        ));
        let restored = restore_owned_config_changes("", &applied, &current)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            restored["features"]["multi_agent_v2"]["root_agent_usage_hint_text"].as_str(),
            Some("Concurrent root hint.")
        );

        let applied = config_with_hint(PREVIOUS_ROOT_AGENT_COLLABORATION_USAGE_HINT);
        let current = config_with_hint(&format!(
            "{PREVIOUS_ROOT_AGENT_COLLABORATION_USAGE_HINT}\n\nConcurrent root hint."
        ));
        let restored = restore_owned_config_changes("", &applied, &current)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            restored["features"]["multi_agent_v2"]["root_agent_usage_hint_text"].as_str(),
            Some("Concurrent root hint.")
        );
    }

    #[test]
    fn lease_preserves_plugin_install_metadata_across_relaunches() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        let marker = temp.path().join("codey/codex-lease.json");
        let backup_root = temp.path().join("codey/codex-backups");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "model_provider = \"codey_global\"\n",
        )
        .unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions::for_test(&marker, &backup_root),
        )
        .unwrap();

        let mut current = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let marketplaces = ensure_root_table(&mut current, "marketplaces").unwrap();
        let mut bundled = Table::new();
        bundled["source_type"] = value("local");
        bundled["source"] = value("/tmp/openai-bundled");
        bundled["last_updated"] = value("2026-07-21T09:00:00Z");
        marketplaces["openai-bundled"] = Item::Table(bundled);
        let plugins = ensure_root_table(&mut current, "plugins").unwrap();
        let mut browser = Table::new();
        browser["enabled"] = value(true);
        browser["version"] = value("26.715.52143");
        browser["install_path"] = value("/tmp/plugins/browser/26.715.52143");
        plugins["browser@openai-bundled"] = Item::Table(browser);
        atomic_write(
            &home.join("config.toml"),
            document_string(&current).unwrap().as_bytes(),
        )
        .unwrap();

        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        let first_restore = fs::read(home.join("config.toml")).unwrap();

        apply_runtime_provider_config_at_mode(
            &home,
            &direct_profile(RelayProtocol::Responses),
            GLOBAL_PROVIDER_ID,
            ProviderApplyOptions::for_test(&marker, &backup_root),
        )
        .unwrap();
        assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
        assert_eq!(fs::read(home.join("config.toml")).unwrap(), first_restore);

        let restored = String::from_utf8(first_restore)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            restored["marketplaces"]["openai-bundled"]["last_updated"].as_str(),
            Some("2026-07-21T09:00:00Z")
        );
        assert_eq!(
            restored["plugins"]["browser@openai-bundled"]["version"].as_str(),
            Some("26.715.52143")
        );
        assert_eq!(
            restored["plugins"]["browser@openai-bundled"]["install_path"].as_str(),
            Some("/tmp/plugins/browser/26.715.52143")
        );
    }

    #[test]
    fn current_provider_defaults_to_builtin_openai_without_creating_config() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();

        assert_eq!(
            current_model_provider(&home).unwrap(),
            BUILTIN_OPENAI_PROVIDER_ID
        );
        assert!(!home.join("config.toml").exists());
    }

    #[test]
    fn preserves_the_builtin_openai_provider_without_adding_a_global_alias() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        let original = "model_provider = \"openai\"\nmodel = \"gpt-5\"\n";
        fs::write(home.join("config.toml"), original).unwrap();

        assert_eq!(
            current_model_provider(&home).unwrap(),
            BUILTIN_OPENAI_PROVIDER_ID
        );
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn preserves_the_current_legacy_global_official_provider() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"model_provider = "codey_global"

[model_providers.codey_global]
name = "OpenAI (Codey Global)"
base_url = "https://chatgpt.com/backend-api/codex/"
wire_api = "responses"
requires_openai_auth = true
"#,
        )
        .unwrap();

        let original = fs::read_to_string(home.join("config.toml")).unwrap();
        assert_eq!(current_model_provider(&home).unwrap(), GLOBAL_PROVIDER_ID);
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn preserves_a_reserved_current_provider_without_rewriting_its_config() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        let original = r#"model_provider = "openai"

[model_providers.openai]
name = "Private Relay"
base_url = "https://relay.example/v1"
wire_api = "chat"
requires_openai_auth = true
experimental_bearer_token = "sk-existing"
"#;
        fs::write(home.join("config.toml"), original).unwrap();

        assert_eq!(
            current_model_provider(&home).unwrap(),
            BUILTIN_OPENAI_PROVIDER_ID
        );
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn preserves_an_existing_global_provider_api_address() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        let original = r#"model_provider = "codey_global"

[model_providers.codey_global]
name = "Private Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-existing"
"#;
        fs::write(home.join("config.toml"), original).unwrap();

        assert_eq!(current_model_provider(&home).unwrap(), GLOBAL_PROVIDER_ID);
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn preserves_a_legacy_official_global_provider_with_extra_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        let original = r#"model_provider = "codey_global"

[model_providers.codey_global]
name = "OpenAI (Codey Global)"
base_url = "https://chatgpt.com/backend-api/codex/"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "must-not-be-removed"
"#;
        fs::write(home.join("config.toml"), original).unwrap();

        assert_eq!(current_model_provider(&home).unwrap(), GLOBAL_PROVIDER_ID);
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn preserves_an_existing_non_reserved_provider() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        let original = "model_provider = \"company\"\n\n[model_providers.company]\nname = \"Company\"\nbase_url = \"https://example.com/v1\"\n";
        fs::write(home.join("config.toml"), original).unwrap();
        assert_eq!(
            current_model_provider(&home).unwrap(),
            "company".to_string()
        );
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            original
        );
    }
}
