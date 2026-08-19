use super::*;
use crate::codex_config_guidance::{
    CODEY_FASTCTX_GUIDANCE, DEFAULT_AGENT_CONFIG, PREVIOUS_CODEY_FASTCTX_GUIDANCE_V4,
    PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5, PREVIOUS_SUBAGENT_GUIDANCE_V2,
    codey_fastctx_guidance_for_namespace, remove_codey_fastctx_guidance,
};

const GLOBAL_PROVIDER_ID: &str = "codey_global";

#[test]
fn codex_home_is_resolved_once_per_process() {
    assert!(std::ptr::eq(codex_home(), codex_home()));
}

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
fn runtime_config_lock_serializes_codey_writers() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("codex-lease.json");
    let first = RuntimeConfigLock::acquire(&marker).unwrap();
    let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
    let second_marker = marker.clone();
    let second = std::thread::spawn(move || {
        let _guard = RuntimeConfigLock::acquire(&second_marker).unwrap();
        acquired_tx.send(()).unwrap();
    });

    assert!(
        acquired_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err()
    );
    drop(first);
    acquired_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    second.join().unwrap();
}

#[test]
fn default_agent_source_exactly_migrates_to_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let constraints_dir = temp.path().join("codex-constraints");
    fs::create_dir_all(&constraints_dir).unwrap();
    let source_path = constraints_dir.join(CODEY_SUBAGENT_SOURCE_FILE);
    fs::write(
        &source_path,
        crate::codex_config_guidance::previous_default_agent_config_without_sandbox(),
    )
    .unwrap();

    let roles = crate::config::default_subagent_roles();
    prepare_runtime_agent_files(&constraints_dir, &roles, None).unwrap();

    assert_eq!(
        fs::read_to_string(&source_path).unwrap(),
        DEFAULT_AGENT_CONFIG
    );
    assert!(
        fs::read_to_string(constraints_dir.join(CODEY_RUNTIME_DEFAULT_AGENT_FILE))
            .unwrap()
            .contains("sandbox_mode = \"read-only\"")
    );
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
            subagent_roles: BTreeMap::new(),
            original_agents_md_exists: false,
            original_default_agent_exists: false,
            original_agents_dir_exists: false,
            provider_id: Some(provider_id.to_string()),
            applied_base_url: Some(applied_base_url.to_string()),
            isolated_runtime_constraints: false,
            independent_prompt_sources: false,
            runtime_hooks_applied: false,
            original_hooks_file_exists: false,
        },
    )
    .unwrap();
}

#[test]
fn official_patch_uses_the_official_endpoint_without_a_custom_catalog() {
    let result = patch_config(
        "model = \"gpt\"\nmodel_catalog_json = \"old.json\"\n",
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        true,
    )
    .unwrap();
    assert!(result.contains("base_url = \"https://chatgpt.com/backend-api/codex\""));
    assert!(!result.contains("experimental_bearer_token"));
    assert_eq!(root_key_string(&result, "model_catalog_json"), None);
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
    assert!(document.get("model_catalog_json").is_none());
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
fn direct_patch_installs_the_codey_model_catalog_when_requested() {
    let result = patch_config(
        "model_catalog_json = \"old.json\"\n",
        &direct_profile(RelayProtocol::Responses),
        "relay",
        true,
    )
    .unwrap();

    assert_eq!(
        root_key_string(&result, "model_catalog_json").as_deref(),
        Some("model-catalogs/codey-official.json")
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
fn direct_responses_patch_routes_codex_through_the_local_proxy_when_it_is_running() {
    // Responses 线路存在第三方模型时代理接管 base_url；官方模型直通、
    // 第三方模型由代理逐请求转换为 Chat Completions。
    let result = patch_config_with_fastctx_mode_and_proxy(
        "model_provider = \"relay\"\n",
        &direct_profile(RelayProtocol::Responses),
        "relay",
        ProviderPatchOptions {
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
    assert!(!result.contains("https://relay.example/v1"));
}

#[test]
fn direct_chat_patch_routes_codex_through_the_local_responses_proxy() {
    let result = patch_config_with_fastctx_mode_and_proxy(
        "model_provider = \"relay\"\n",
        &direct_profile(RelayProtocol::ChatCompletions),
        "relay",
        ProviderPatchOptions {
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
        server["env"]["FASTCTX_TOKEN_BUDGET"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        CODEY_FASTCTX_TOKEN_BUDGET
    );
    assert_eq!(
        server["env"]["FASTCTX_GREP_TOKEN_BUDGET"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        CODEY_FASTCTX_GREP_TOKEN_BUDGET
    );
    assert_eq!(
        server["env"]["FASTCTX_GLOB_TOKEN_BUDGET"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        CODEY_FASTCTX_GLOB_TOKEN_BUDGET
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
    assert_eq!(
        server["tool_timeout_sec"].as_integer(),
        Some(CODEY_FASTCTX_TOOL_TIMEOUT_SECONDS)
    );
    assert_eq!(server["runtime_note"].as_str(), Some("preserve"));
    assert_eq!(server["env"]["CONCURRENT"].as_str(), Some("preserve"));
    assert_eq!(
        server["env"]["FASTCTX_TOKEN_BUDGET"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        CODEY_FASTCTX_TOKEN_BUDGET
    );
}

#[test]
fn fast_context_tools_scale_budgets_down_for_a_smaller_user_host_limit() {
    let result = patch_config_with_fastctx(
        "tool_output_token_limit = 16000\n",
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        relative_model_catalog_path(),
        None,
        Some(Path::new("/tmp/codey-fastctx")),
        false,
    )
    .unwrap();
    let document = parse_document(&result).unwrap();
    let server = document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]
        .as_table()
        .unwrap();

    assert_eq!(
        document["tool_output_token_limit"].as_integer(),
        Some(16_000)
    );
    assert_eq!(
        server["env"]["FASTCTX_TOKEN_BUDGET"].as_str(),
        Some("14400")
    );
    assert_eq!(
        server["env"]["FASTCTX_GREP_TOKEN_BUDGET"].as_str(),
        Some("10800")
    );
    assert_eq!(
        server["env"]["FASTCTX_GLOB_TOKEN_BUDGET"].as_str(),
        Some("5400")
    );
}

#[test]
fn fast_context_tools_keep_an_explicit_zero_host_output_limit() {
    let result = patch_config_with_fastctx(
        "tool_output_token_limit = 0\n",
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        relative_model_catalog_path(),
        None,
        Some(Path::new("/tmp/codey-fastctx")),
        false,
    )
    .unwrap();
    let document = parse_document(&result).unwrap();
    let server = document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]
        .as_table()
        .unwrap();

    assert_eq!(document["tool_output_token_limit"].as_integer(), Some(0));
    assert!(
        server.get("env").is_none(),
        "显式 0 且此前无 env 时不应派生预算环境变量"
    );
}

#[test]
fn fast_context_tools_drop_stale_budget_keys_under_an_explicit_zero_limit() {
    let existing = r#"tool_output_token_limit = 0

[mcp_servers.codey_fastctx]
command = "/old/codey-fastctx"
args = ["--codey-fastctx-mcp"]

[mcp_servers.codey_fastctx.env]
FASTCTX_TOKEN_BUDGET = "54000"
FASTCTX_GREP_TOKEN_BUDGET = "10800"
FASTCTX_GLOB_TOKEN_BUDGET = "5400"
USER_KEY = "preserve"
"#;
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
    let server = document["mcp_servers"][CODEY_FASTCTX_SERVER_ID]
        .as_table()
        .unwrap();

    assert_eq!(document["tool_output_token_limit"].as_integer(), Some(0));
    let env = server
        .get("env")
        .and_then(|env| env.as_table())
        .expect("已有 env 中的用户键应保留");
    assert_eq!(env["USER_KEY"].as_str(), Some("preserve"));
    for key in [
        "FASTCTX_TOKEN_BUDGET",
        "FASTCTX_GREP_TOKEN_BUDGET",
        "FASTCTX_GLOB_TOKEN_BUDGET",
    ] {
        assert!(env.get(key).is_none(), "应清掉残留的 {key}");
    }
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
fn disabling_fast_context_tools_cleans_an_orphan_reserved_namespace() {
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
        vec!["mcp__user"]
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
    assert!(guidance.contains("`mcp__codey_fastctx__inspect_local_file`"));
    assert!(guidance.contains("`mcp__codey_fastctx__grep`"));
    assert!(guidance.contains("`mcp__codey_fastctx__glob`"));
    assert!(guidance.contains("`mcp__codey_fastctx__replace`"));
    assert!(guidance.contains("Use CodeGraph only for semantic code understanding"));
    assert!(guidance.contains("Batch 2-32 already-known text files"));
    assert!(guidance.contains("Start broad searches with grep's `files_with_matches`"));
    assert!(guidance.contains("FastCtx is a direct-only tool namespace"));
    assert!(guidance.contains("never transparently retry a write"));
    assert!(guidance.contains("using `tool_search` when a direct tool is deferred"));
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
        vec!["mcp__existing", "mcp__codey_fastctx"]
    );
    assert_eq!(
        document["tool_output_token_limit"].as_integer(),
        Some(CODEY_FASTCTX_HOST_TOKEN_LIMIT)
    );
    assert_eq!(document["features"]["hooks"].as_bool(), Some(true));
    let route_hooks = document["hooks"]["PreToolUse"]
        .as_array_of_tables()
        .unwrap();
    assert_eq!(route_hooks.len(), 1);
    assert_eq!(
        route_hooks.get(0).unwrap()["matcher"].as_str(),
        Some(crate::fastctx_route_gate::HOOK_MATCHER)
    );
    assert!(first.contains(crate::fastctx_route_gate::HOOK_ARGUMENT));
    assert_eq!(document["hooks"]["state"].as_table().unwrap().len(), 1);
}

#[test]
fn fast_context_tools_normalize_and_keep_direct_only_namespace_in_inline_tables() {
    for existing in [
        r#"
features = { code_mode = { direct_only_tool_namespaces = ["mcp__existing", "mcp__codey_fastctx"] }, user_flag = true }
"#,
        r#"
[features]
code_mode = { direct_only_tool_namespaces = ["mcp__existing", "mcp__codey_fastctx"] }
user_flag = true
"#,
    ] {
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
        let namespaces = direct_only_tool_namespaces(&document).unwrap();

        assert_eq!(
            namespaces
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec!["mcp__existing", "mcp__codey_fastctx"]
        );
        assert_eq!(
            document["features"]["user_flag"].as_bool(),
            Some(true),
            "inline feature fields must be preserved"
        );
    }
}

#[test]
fn subagent_optimization_writes_public_agents_schema_and_migrates_legacy_threads() {
    let existing = r#"
[agents]
max_threads = 6
max_depth = 1
interrupt_message = true
custom_setting = "preserved"

[features.multi_agent_v2]
enabled = false
max_concurrent_threads_per_session = 2
default_subagent_model = "legacy-v2-model"
default_subagent_reasoning_effort = "low"
custom_setting = "preserved"
subagent_developer_instructions = "Preserve my subagent guidance."
root_agent_usage_hint_text = "Preserve my root usage hint."
multi_agent_mode_hint_text = "Require explicit requests."

[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "echo preserve-user-hook"
"#;
    let result = patch_config_with_fastctx_mode_and_proxy(
        existing,
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        ProviderPatchOptions {
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
    assert_eq!(agents["interrupt_message"].as_bool(), Some(true));
    assert_eq!(agents["custom_setting"].as_str(), Some("preserved"));
    assert_eq!(agents["enabled"].as_bool(), Some(true));
    assert_eq!(
        agents["max_concurrent_threads_per_session"].as_integer(),
        Some(6)
    );
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
    assert!(
        multi_agent
            .get("max_concurrent_threads_per_session")
            .is_none()
    );
    assert!(multi_agent.get("default_subagent_model").is_none());
    assert!(
        multi_agent
            .get("default_subagent_reasoning_effort")
            .is_none()
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
    assert_eq!(document["features"]["hooks"].as_bool(), Some(true));
    assert_eq!(
        multi_agent["subagent_developer_instructions"].as_str(),
        Some("Preserve my subagent guidance.")
    );
    let root_usage_hint = multi_agent["root_agent_usage_hint_text"].as_str().unwrap();
    assert!(root_usage_hint.contains("Preserve my root usage hint."));
    assert!(root_usage_hint.contains(ROOT_AGENT_COLLABORATION_USAGE_HINT));
    assert_eq!(
        multi_agent["multi_agent_mode_hint_text"].as_str(),
        Some(ROOT_AGENT_MULTI_AGENT_MODE_HINT)
    );

    let pre_tool_use = document["hooks"]["PreToolUse"]
        .as_array_of_tables()
        .unwrap();
    assert_eq!(pre_tool_use.len(), 2);
    let preserved_handler = pre_tool_use.get(0).unwrap()["hooks"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        preserved_handler["command"].as_str(),
        Some("echo preserve-user-hook")
    );
    let gate_handler = pre_tool_use.get(1).unwrap()["hooks"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(gate_handler["type"].as_str(), Some("command"));
    assert!(
        gate_handler["command"]
            .as_str()
            .unwrap()
            .contains(crate::subagent_gate::HOOK_ARGUMENT)
    );
    let windows_command = gate_handler["commandWindows"].as_str().unwrap();
    assert!(windows_command.starts_with("& '"), "{windows_command}");
    assert!(windows_command.contains(crate::subagent_gate::HOOK_ARGUMENT));
    assert_eq!(
        gate_handler["timeout"].as_integer(),
        Some(crate::subagent_gate::HOOK_TIMEOUT_SECONDS as i64)
    );
    let post_tool_use = document["hooks"]["PostToolUse"]
        .as_array_of_tables()
        .unwrap();
    assert_eq!(post_tool_use.len(), 1);
    assert_eq!(
        post_tool_use.get(0).unwrap()["matcher"].as_str(),
        Some(crate::subagent_orchestrator::POST_TOOL_HOOK_MATCHER)
    );
    for event in ["SubagentStart", "SubagentStop", "Stop", "SessionEnd"] {
        assert_eq!(
            document["hooks"][event].as_array_of_tables().unwrap().len(),
            1,
            "{event}"
        );
    }
    let hook_state = document["hooks"]["state"].as_table().unwrap();
    assert_eq!(hook_state.len(), 6);
    let pre_tool_key = "/tmp/codey-codex/config.toml:pre_tool_use:1:0";
    assert!(
        hook_state[pre_tool_key]["trusted_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
}

#[test]
fn subagent_and_fastctx_share_one_pre_tool_hook() {
    let config_path = Path::new("/tmp/codey-codex/config.toml");
    let result = patch_config_with_fastctx_mode_and_proxy(
        "",
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        ProviderPatchOptions {
            config_path,
            model_catalog_path: relative_model_catalog_path(),
            default_model: None,
            fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
            subagent_optimization: true,
            subagent_model: "gpt-5.6-sol",
            subagent_reasoning_effort: "high",
            preserve_provider_route: false,
            protocol_proxy_base_url: None,
        },
    )
    .unwrap();
    let document = result.parse::<DocumentMut>().unwrap();
    let pre_tool_use = document["hooks"]["PreToolUse"]
        .as_array_of_tables()
        .unwrap();
    assert_eq!(pre_tool_use.len(), 1);
    assert_eq!(pre_tool_use.get(0).unwrap()["matcher"].as_str(), Some("*"));
    let handler = pre_tool_use.get(0).unwrap()["hooks"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap();
    assert!(
        handler["command"]
            .as_str()
            .unwrap()
            .contains(crate::subagent_gate::COMBINED_HOOK_ARGUMENT)
    );
    assert!(
        !handler["command"]
            .as_str()
            .unwrap()
            .contains(crate::fastctx_route_gate::HOOK_ARGUMENT)
    );
    for event in [
        "PostToolUse",
        "SubagentStart",
        "SubagentStop",
        "Stop",
        "SessionEnd",
    ] {
        assert_eq!(
            document["hooks"][event].as_array_of_tables().unwrap().len(),
            1,
            "{event}"
        );
    }
    assert_eq!(
        document["hooks"]["state"].as_table().unwrap().len(),
        SUBAGENT_GATE_HOOKS.len()
    );

    let combined_commands =
        crate::subagent_gate::hook_commands_for(crate::subagent_gate::COMBINED_HOOK_ARGUMENT)
            .unwrap();
    let selected_command = if cfg!(windows) {
        combined_commands.command_windows.as_str()
    } else {
        combined_commands.command.as_str()
    };
    let expected_hash = crate::subagent_gate::hook_trust_hash(
        "pre_tool_use",
        Some("*"),
        selected_command,
        crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    );
    let state_key = format!("{}:pre_tool_use:0:0", config_path.display());
    assert_eq!(
        document["hooks"]["state"][&state_key]["trusted_hash"].as_str(),
        Some(expected_hash.as_str())
    );
}

#[test]
fn subagent_hook_upgrade_replaces_legacy_codey_groups_and_preserves_user_hook_state() {
    let config_path = Path::new("/tmp/codey-codex/config.toml");
    let existing = format!(
        r#"
[hooks.state."{config_path}:pre_tool_use:0:0"]
trusted_hash = "sha256:legacy-codey-one"

[hooks.state."{config_path}:pre_tool_use:1:0"]
trusted_hash = "sha256:legacy-codey-two"

[hooks.state."{config_path}:pre_tool_use:2:0"]
trusted_hash = "sha256:user-hook"

[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "'/old/codey' {hook_argument}"
commandWindows = '"C:\\old\\codey.exe" {hook_argument}'
timeout = 5

[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "'/second/codey' {hook_argument}"
commandWindows = '"C:\\second\\codey.exe" {hook_argument}'
timeout = 5

[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "echo preserve-user-hook"
timeout = 2
"#,
        config_path = config_path.display(),
        hook_argument = crate::subagent_gate::HOOK_ARGUMENT,
    );
    let result = patch_config_with_fastctx_mode_and_proxy(
        &existing,
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        ProviderPatchOptions {
            config_path,
            model_catalog_path: relative_model_catalog_path(),
            default_model: None,
            fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
            subagent_optimization: true,
            subagent_model: "gpt-5.6-sol",
            subagent_reasoning_effort: "high",
            preserve_provider_route: false,
            protocol_proxy_base_url: None,
        },
    )
    .unwrap();
    let document = result.parse::<DocumentMut>().unwrap();
    let pre_tool_use = document["hooks"]["PreToolUse"]
        .as_array_of_tables()
        .unwrap();
    assert_eq!(pre_tool_use.len(), 2);
    assert_eq!(
        pre_tool_use.get(0).unwrap()["matcher"].as_str(),
        Some("Bash")
    );
    assert_eq!(
        pre_tool_use.get(0).unwrap()["hooks"]
            .as_array_of_tables()
            .unwrap()
            .get(0)
            .unwrap()["command"]
            .as_str(),
        Some("echo preserve-user-hook")
    );
    let gate_command = pre_tool_use.get(1).unwrap()["hooks"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap()["command"]
        .as_str()
        .unwrap();
    assert!(gate_command.contains(crate::subagent_gate::COMBINED_HOOK_ARGUMENT));
    assert!(!result.contains("/old/codey"));
    assert!(!result.contains("/second/codey"));

    let state = document["hooks"]["state"].as_table().unwrap();
    assert_eq!(
        state[&format!("{}:pre_tool_use:0:0", config_path.display())]["trusted_hash"].as_str(),
        Some("sha256:user-hook")
    );
    assert!(
        state[&format!("{}:pre_tool_use:1:0", config_path.display())]["trusted_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert!(
        state
            .get(&format!("{}:pre_tool_use:2:0", config_path.display()))
            .is_none()
    );

    let repeated = patch_config_with_fastctx_mode_and_proxy(
        &result,
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        ProviderPatchOptions {
            config_path,
            model_catalog_path: relative_model_catalog_path(),
            default_model: None,
            fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
            subagent_optimization: true,
            subagent_model: "gpt-5.6-sol",
            subagent_reasoning_effort: "high",
            preserve_provider_route: false,
            protocol_proxy_base_url: None,
        },
    )
    .unwrap();
    assert_eq!(repeated, result);
}

#[test]
fn subagent_optimization_keeps_explicit_agents_concurrency_over_legacy_max_threads() {
    let existing = r#"
[agents]
max_threads = 6
max_concurrent_threads_per_session = 4

[features.multi_agent_v2]
max_concurrent_threads_per_session = 2
default_subagent_model = "legacy-v2-model"
default_subagent_reasoning_effort = "low"
"#;
    let result = patch_config_with_fastctx_mode_and_proxy(
        existing,
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        ProviderPatchOptions {
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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

    assert!(
        document["agents"]
            .as_table()
            .unwrap()
            .get("max_threads")
            .is_none()
    );
    assert_eq!(
        document["agents"]["max_concurrent_threads_per_session"].as_integer(),
        Some(4)
    );
    assert!(
        document["features"]["multi_agent_v2"]
            .as_table()
            .unwrap()
            .get("max_concurrent_threads_per_session")
            .is_none()
    );
    assert!(
        document["features"]["multi_agent_v2"]
            .as_table()
            .unwrap()
            .get("default_subagent_model")
            .is_none()
    );
    assert!(
        document["features"]["multi_agent_v2"]
            .as_table()
            .unwrap()
            .get("default_subagent_reasoning_effort")
            .is_none()
    );
}

#[test]
fn subagent_optimization_defaults_concurrency_for_new_or_invalid_configs() {
    for existing in [
        "",
        "[agents]\nmax_threads = \"invalid\"\n",
        "[agents]\nmax_threads = 0\n",
        "[agents]\nmax_concurrent_threads_per_session = \"invalid\"\n",
        "[agents]\nmax_concurrent_threads_per_session = 0\n",
    ] {
        let result = patch_config_with_fastctx_mode_and_proxy(
            existing,
            &official_profile(),
            GLOBAL_PROVIDER_ID,
            ProviderPatchOptions {
                config_path: Path::new("/tmp/codey-codex/config.toml"),
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

        assert_eq!(
            document["agents"]["max_concurrent_threads_per_session"].as_integer(),
            Some(DEFAULT_SUBAGENT_MAX_CONCURRENCY)
        );
        assert!(
            document["agents"]
                .as_table()
                .unwrap()
                .get("max_threads")
                .is_none()
        );
    }
}

#[test]
fn subagent_optimization_accepts_dynamic_model_ids_and_rejects_empty_values() {
    let patched = patch_config_with_fastctx_mode_and_proxy(
        "",
        &official_profile(),
        GLOBAL_PROVIDER_ID,
        ProviderPatchOptions {
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
            config_path: Path::new("/tmp/codey-codex/config.toml"),
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
fn subagent_lease_keeps_prompt_rules_in_independent_files() {
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
    let mut role_selections = crate::config::uniform_subagent_roles(
        DEFAULT_SUBAGENT_MODEL,
        DEFAULT_SUBAGENT_REASONING_EFFORT,
    );
    role_selections.insert(
        crate::config::SUBAGENT_ROLE_QUICK_SCAN.to_string(),
        SubagentRoleConfig::new("provider-fast", "medium"),
    );

    apply_runtime_provider_config_at_mode(
        &home,
        &direct_profile(RelayProtocol::Responses),
        GLOBAL_PROVIDER_ID,
        ProviderApplyOptions {
            subagent_optimization: true,
            subagent_roles: Some(&role_selections),
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
    assert!(
        document["features"]["multi_agent_v2"]
            .as_table()
            .unwrap()
            .get("root_agent_usage_hint_text")
            .is_none()
    );
    assert_eq!(document["features"]["hooks"].as_bool(), Some(true));
    for role in SUBAGENT_ROLE_IDS {
        assert!(
            document["agents"][role]["config_file"]
                .as_str()
                .is_some_and(|path| Path::new(path).is_absolute()),
            "missing absolute config_file for {role}"
        );
        assert!(
            document["agents"][role]["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "missing description for {role}"
        );
    }
    let quick_runtime_path =
        document["agents"][crate::config::SUBAGENT_ROLE_QUICK_SCAN]["config_file"]
            .as_str()
            .unwrap();
    let quick_runtime = fs::read_to_string(quick_runtime_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(quick_runtime["model"].as_str(), Some("provider-fast"));
    assert_eq!(
        quick_runtime["model_reasoning_effort"].as_str(),
        Some("medium")
    );
    assert!(
        temporary_config.contains(crate::subagent_gate::HOOK_ARGUMENT),
        "runtime config should install the subagent gate hooks"
    );
    assert_eq!(
        document["hooks"]["state"].as_table().unwrap().len(),
        SUBAGENT_GATE_HOOKS.len()
    );
    assert!(!temporary_config.contains(SUBAGENT_GUIDANCE));
    assert!(!temporary_config.contains(ROOT_AGENT_COLLABORATION_USAGE_HINT));
    assert_eq!(
        fs::read(home.join("AGENTS.md")).unwrap(),
        original_agents_md
    );
    assert_eq!(
        fs::read(home.join("agents/default.toml")).unwrap(),
        original_default_agent
    );
    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    assert_eq!(
        fs::read_to_string(constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE)).unwrap(),
        SUBAGENT_GUIDANCE
    );
    assert_eq!(
        fs::read_to_string(constraints_dir.join(CODEY_COLLABORATION_HINT_FILE)).unwrap(),
        ROOT_AGENT_COLLABORATION_USAGE_HINT
    );
    let overrides = build_independent_prompt_runtime_overrides(
        temporary_config.as_bytes(),
        &constraints_dir,
        true,
    )
    .unwrap();
    assert!(overrides.iter().any(|entry| {
        entry.starts_with("developer_instructions=") && entry.contains("## 子代理使用")
    }));
    assert!(overrides.iter().any(|entry| {
        entry.starts_with("features.multi_agent_v2.root_agent_usage_hint_text=")
            && entry.contains("agents.spawn_agent")
    }));

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
fn subagent_lease_migrates_recognized_embedded_rule_files() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex-home");
    let marker = temp.path().join("codey/codex-lease.json");
    let backup_root = temp.path().join("codey/codex-backups");
    fs::create_dir_all(home.join("agents")).unwrap();
    fs::write(
        home.join("config.toml"),
        format!(
            "model_provider = \"codey_global\"\n\
             developer_instructions = {}\n\
             \n\
             [features.multi_agent_v2]\n\
             root_agent_usage_hint_text = {}\n\
             subagent_developer_instructions = {}\n",
            Value::from(format!("User root rule.\n\n{CODEY_FASTCTX_GUIDANCE}")),
            Value::from(ROOT_AGENT_COLLABORATION_USAGE_HINT),
            Value::from(CODEY_FASTCTX_GUIDANCE),
        ),
    )
    .unwrap();
    fs::write(
        home.join("AGENTS.md"),
        format!("# User guidance\n\n{PREVIOUS_SUBAGENT_GUIDANCE_V2}\n"),
    )
    .unwrap();
    fs::write(home.join("agents/default.toml"), DEFAULT_AGENT_CONFIG).unwrap();

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

    assert_eq!(
        fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        "# User guidance\n"
    );
    assert!(!home.join("agents/default.toml").exists());
    let applied_config = fs::read_to_string(home.join("config.toml")).unwrap();
    let applied_document = parse_document(&applied_config).unwrap();
    assert_eq!(
        applied_document["developer_instructions"].as_str(),
        Some("User root rule.")
    );
    assert!(!applied_config.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!applied_config.contains(ROOT_AGENT_COLLABORATION_USAGE_HINT));
    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    assert_eq!(
        fs::read_to_string(constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE)).unwrap(),
        SUBAGENT_GUIDANCE
    );

    assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
    assert_eq!(
        fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        "# User guidance\n"
    );
    assert!(!home.join("agents/default.toml").exists());
    let restored_config = fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(restored_config.contains("User root rule."));
    assert!(!restored_config.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!restored_config.contains(ROOT_AGENT_COLLABORATION_USAGE_HINT));
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
    assert!(!home.join("agents/default.toml").exists());
    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    let default_agent =
        fs::read_to_string(constraints_dir.join(CODEY_SUBAGENT_SOURCE_FILE)).unwrap();
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
fn previous_fastctx_guidance_migration_handles_inline_subagent_tables() {
    let dynamic_previous =
        PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5.replace(CODEY_FASTCTX_NAMESPACE, "mcp__legacy_fastctx");
    let existing = format!(
        "developer_instructions = {}\n\
         features = {{ multi_agent_v2 = {{ subagent_developer_instructions = {} }} }}\n",
        Value::from(format!(
            "Root user guidance.\n\n{PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5}"
        )),
        Value::from(format!("Subagent user guidance.\n\n{dynamic_previous}")),
    );

    let migrated = migrate_previous_fastctx_guidance(&existing, true, "inline config")
        .unwrap()
        .unwrap();
    let document = parse_document(&migrated).unwrap();

    assert_eq!(
        document["developer_instructions"].as_str(),
        Some("Root user guidance.")
    );
    assert_eq!(
        document["features"]["multi_agent_v2"]["subagent_developer_instructions"].as_str(),
        Some("Subagent user guidance.")
    );
    assert!(!migrated.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5));
    assert!(!migrated.contains("mcp__legacy_fastctx"));
}

#[test]
fn previous_fastctx_guidance_is_migrated_before_the_runtime_lease() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex-home");
    let marker = temp.path().join("codey/codex-lease.json");
    let backup_root = temp.path().join("codey/codex-backups");
    fs::create_dir_all(home.join("agents")).unwrap();
    let dynamic_previous =
        PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5.replace(CODEY_FASTCTX_NAMESPACE, "mcp__legacy_fastctx");
    let original_config = format!(
        r#"model_provider = "codey_global"
developer_instructions = {}

[model_providers.codey_global]
base_url = "https://chatgpt.com/backend-api/codex"

[features.multi_agent_v2]
subagent_developer_instructions = {}
"#,
        Value::from(format!(
            "Root user guidance.\n\n{PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5}"
        )),
        Value::from(format!("Subagent user guidance.\n\n{dynamic_previous}")),
    );
    let original_default_agent = format!(
        r#"name = "custom"
developer_instructions = {}
"#,
        Value::from(format!(
            "Default user guidance.\n\n{PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5}"
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
    let temporary_document = parse_document(&temporary_config).unwrap();
    let temporary_root_guidance = temporary_document["developer_instructions"]
        .as_str()
        .unwrap();
    let temporary_subagent_guidance =
        temporary_document["features"]["multi_agent_v2"]["subagent_developer_instructions"]
            .as_str()
            .unwrap();
    assert_eq!(temporary_root_guidance, "Root user guidance.");
    assert_eq!(temporary_subagent_guidance, "Subagent user guidance.");
    assert!(!temporary_config.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!temporary_config.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5));
    let temporary_default = fs::read_to_string(home.join("agents/default.toml")).unwrap();
    assert!(temporary_default.contains("Default user guidance."));
    assert!(!temporary_default.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!temporary_default.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5));
    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    let fastctx_source =
        fs::read_to_string(constraints_dir.join(CODEY_FASTCTX_INSTRUCTIONS_FILE)).unwrap();
    assert_eq!(fastctx_source, CODEY_FASTCTX_GUIDANCE);
    let runtime_default =
        fs::read_to_string(constraints_dir.join(CODEY_RUNTIME_DEFAULT_AGENT_FILE)).unwrap();
    assert!(runtime_default.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!runtime_default.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5));

    assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
    let restored_config = fs::read_to_string(home.join("config.toml")).unwrap();
    let restored_document = parse_document(&restored_config).unwrap();
    assert_eq!(
        restored_document["developer_instructions"].as_str(),
        Some("Root user guidance.")
    );
    assert_eq!(
        restored_document["features"]["multi_agent_v2"]["subagent_developer_instructions"].as_str(),
        Some("Subagent user guidance.")
    );
    assert!(!restored_config.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!restored_config.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5));

    let restored_default = fs::read_to_string(home.join("agents/default.toml")).unwrap();
    let restored_default_document = parse_document(&restored_default).unwrap();
    assert_eq!(
        restored_default_document["developer_instructions"].as_str(),
        Some("Default user guidance.")
    );
    assert!(!restored_default.contains(CODEY_FASTCTX_GUIDANCE));
    assert!(!restored_default.contains(PREVIOUS_CODEY_FASTCTX_GUIDANCE_V4));
}

#[test]
fn runtime_subagent_roles_refresh_in_place_for_the_next_spawn() {
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

    let mut lease =
        serde_json::from_str::<RuntimeConfigLease>(&fs::read_to_string(&marker).unwrap()).unwrap();
    lease.isolated_runtime_constraints = true;
    write_lease(&marker, &lease).unwrap();

    let mut config = CodeyConfig {
        subagent_optimization: true,
        ..CodeyConfig::default()
    };
    for (index, role) in SUBAGENT_ROLE_IDS.into_iter().enumerate() {
        config.subagent_roles.insert(
            role.to_string(),
            SubagentRoleConfig::new(
                format!("provider-role-{index}"),
                if index % 2 == 0 { "low" } else { "high" },
            ),
        );
    }
    config = config.normalize();

    refresh_runtime_subagent_roles_at(&config, &marker).unwrap();

    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    for role in SUBAGENT_ROLE_IDS {
        let document = fs::read_to_string(runtime_agent_path(&constraints_dir, role))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let expected = &config.subagent_roles[role];
        assert_eq!(document["name"].as_str(), Some(role));
        assert_eq!(document["model"].as_str(), Some(expected.model.as_str()));
        assert_eq!(
            document["model_reasoning_effort"].as_str(),
            Some(expected.reasoning_effort.as_str())
        );
    }
    let refreshed =
        serde_json::from_str::<RuntimeConfigLease>(&fs::read_to_string(&marker).unwrap()).unwrap();
    assert!(refreshed.isolated_runtime_constraints);
    assert_eq!(refreshed.subagent_roles, config.subagent_roles);
    assert_eq!(refreshed.subagent_model, config.subagent_model);
    assert_eq!(
        refreshed.subagent_reasoning_effort,
        config.subagent_reasoning_effort
    );

    let original_lease = fs::read(&marker).unwrap();
    let original_runtime_files = SUBAGENT_ROLE_IDS
        .into_iter()
        .map(|role| {
            let path = runtime_agent_path(&constraints_dir, role);
            (path.clone(), fs::read(path).unwrap())
        })
        .collect::<Vec<_>>();
    let mut invalid = config.clone();
    for (index, role) in SUBAGENT_ROLE_IDS.into_iter().enumerate() {
        invalid.subagent_roles.insert(
            role.to_string(),
            SubagentRoleConfig::new(format!("partial-update-{index}"), "medium"),
        );
    }
    invalid
        .subagent_roles
        .get_mut(SUBAGENT_ROLE_DEFAULT)
        .unwrap()
        .reasoning_effort = "invalid".into();

    let error = refresh_runtime_subagent_roles_at(&invalid, &marker).unwrap_err();

    assert!(format!("{error:#}").contains("已恢复原配置"));
    assert_eq!(fs::read(&marker).unwrap(), original_lease);
    for (path, contents) in original_runtime_files {
        assert_eq!(fs::read(path).unwrap(), contents);
    }
}

#[test]
fn runtime_subagent_roles_refresh_follows_the_leased_fastctx_registration() {
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
            fastctx_command: Some(Path::new("/tmp/codey-fastctx")),
            ..ProviderApplyOptions::for_test(&marker, &backup_root)
        },
    )
    .unwrap();

    // 配置开关关闭但租约记录了本次已注册内置 server：热更新仍注入指引，
    // 与启动路径按实际注册判定的语义一致。
    let config = CodeyConfig {
        subagent_optimization: true,
        ..CodeyConfig::default()
    }
    .normalize();
    assert!(!config.fast_context_tools);
    refresh_runtime_subagent_roles_at(&config, &marker).unwrap();

    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    for role in SUBAGENT_ROLE_IDS {
        let document = fs::read_to_string(runtime_agent_path(&constraints_dir, role))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let instructions = document["developer_instructions"]
            .as_str()
            .unwrap_or_default();
        assert!(
            instructions.contains(CODEY_FASTCTX_GUIDANCE),
            "租约已注册 fastctx 时角色 {role} 应携带指引"
        );
    }

    // 租约未记录 fastctx（用户自备 server 或从未启用）时，即使配置开关开启，
    // 热更新也不注入指引。
    let mut lease =
        serde_json::from_str::<RuntimeConfigLease>(&fs::read_to_string(&marker).unwrap()).unwrap();
    lease.fastctx_command = None;
    write_lease(&marker, &lease).unwrap();
    let config = CodeyConfig {
        fast_context_tools: true,
        ..config
    };
    refresh_runtime_subagent_roles_at(&config, &marker).unwrap();

    for role in SUBAGENT_ROLE_IDS {
        let document = fs::read_to_string(runtime_agent_path(&constraints_dir, role))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let instructions = document["developer_instructions"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !instructions.contains(CODEY_FASTCTX_GUIDANCE),
            "租约未注册 fastctx 时角色 {role} 不应携带指引"
        );
    }
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
    fs::create_dir_all(home.join("agents")).unwrap();
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
    assert!(!home.join("AGENTS.md").exists());
    assert!(!home.join("agents/default.toml").exists());
    let constraints_dir = marker.parent().unwrap().join(CODEY_CONSTRAINTS_DIR);
    assert!(constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE).exists());
    assert!(constraints_dir.join(CODEY_SUBAGENT_SOURCE_FILE).exists());

    assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
    assert!(!home.join("AGENTS.md").exists());
    assert!(!home.join("agents/default.toml").exists());
    assert!(!home.join("agents").exists());
    assert!(constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE).exists());
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
    current["model_providers"][GLOBAL_PROVIDER_ID]["base_url"] = value("https://user.example/v1");
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
    assert!(restored.get("hooks").is_none());
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
fn cc_switch_chat_route_is_restored_on_disk_after_startup() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex-home");
    let marker = temp.path().join("codey/codex-lease.json");
    let backup_root = temp.path().join("codey/codex-backups");
    fs::create_dir_all(&home).unwrap();
    let original = r#"model_provider = "cc-switch"

[model_providers.cc-switch]
name = "CC Switch Chat"
base_url = "https://chat.example/v1"
wire_api = "responses"
experimental_bearer_token = "original-secret"

[model_providers.cc-switch.http_headers]
X-Route = "original"
"#;
    fs::write(home.join("config.toml"), original).unwrap();
    let mut profile = direct_profile(RelayProtocol::ChatCompletions);
    profile.base_url = "https://chat.example/v1".to_string();
    profile.cc_switch_provider_id = Some("chat-route".to_string());

    apply_runtime_provider_config_at_mode(
        &home,
        &profile,
        "cc-switch",
        ProviderApplyOptions {
            protocol_proxy_base_url: Some("http://127.0.0.1:43123/v1"),
            ..ProviderApplyOptions::for_test(&marker, &backup_root)
        },
    )
    .unwrap();
    let temporary = fs::read_to_string(home.join("config.toml")).unwrap();
    assert_eq!(
        provider_base_url(&temporary, "cc-switch").as_deref(),
        Some("http://127.0.0.1:43123/v1")
    );

    assert!(restore_runtime_cc_switch_provider_config_at(&home, &marker).unwrap());
    let visible = fs::read_to_string(home.join("config.toml")).unwrap();
    let original = parse_document(original).unwrap();
    let visible = parse_document(&visible).unwrap();
    assert!(tables_semantically_equal(
        original["model_providers"]["cc-switch"].as_table().unwrap(),
        visible["model_providers"]["cc-switch"].as_table().unwrap()
    ));
    assert_eq!(
        visible["service_tier"].as_str(),
        Some("default"),
        "Codey 的其他运行时增强应继续留在租约内"
    );
    assert!(marker.exists());

    assert!(!restore_runtime_provider_config_at(&home, &marker).unwrap());
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        original.to_string()
    );
    assert!(!marker.exists());
}

#[test]
fn cc_switch_provider_restore_does_not_overwrite_a_concurrent_route_switch() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex-home");
    let marker = temp.path().join("codey/codex-lease.json");
    let backup_root = temp.path().join("codey/codex-backups");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("config.toml"),
        r#"model_provider = "cc-switch"

[model_providers.cc-switch]
base_url = "https://chat.example/v1"
wire_api = "responses"
"#,
    )
    .unwrap();
    let mut profile = direct_profile(RelayProtocol::ChatCompletions);
    profile.base_url = "https://chat.example/v1".to_string();
    profile.cc_switch_provider_id = Some("chat-route".to_string());
    apply_runtime_provider_config_at_mode(
        &home,
        &profile,
        "cc-switch",
        ProviderApplyOptions {
            protocol_proxy_base_url: Some("http://127.0.0.1:43123/v1"),
            ..ProviderApplyOptions::for_test(&marker, &backup_root)
        },
    )
    .unwrap();

    let switched = r#"model_provider = "other-route"

[model_providers.other-route]
base_url = "https://other.example/v1"
wire_api = "responses"
"#;
    fs::write(home.join("config.toml"), switched).unwrap();

    assert!(!restore_runtime_cc_switch_provider_config_at(&home, &marker).unwrap());
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        switched
    );
    assert!(marker.exists());
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
            subagent_roles: None,
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
            subagent_roles: None,
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
    let restored = parse_document(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
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
            .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
    );
    assert!(
        applied_namespaces
            .iter()
            .any(|entry| entry.as_str() == Some("mcp__existing"))
    );
    current["model"] = value("gpt-new");
    current["service_tier"] = value("fast");
    current["developer_instructions"] = value("Keep concurrent guidance.");
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
fn restore_removes_only_codey_gate_hooks_after_concurrent_hook_edits() {
    let original = r#"
[[hooks.PreToolUse]]
matcher = "Shell"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/user/pre-tool"
timeout = 5
"#;
    let applied = format!(
        r#"{original}
[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "'/tmp/codey' {hook_argument}"
commandWindows = "C:\\Codey\\codey.exe {hook_argument}"
timeout = 5

[hooks.state."/tmp/config.toml:pre_tool_use:1:0"]
trusted_hash = "sha256:codey"

[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "'/tmp/codey' {route_hook_argument}"
commandWindows = "C:\\Codey\\codey.exe {route_hook_argument}"
timeout = 5

[hooks.state."/tmp/config.toml:pre_tool_use:2:0"]
trusted_hash = "sha256:fastctx-route"
"#,
        hook_argument = crate::subagent_gate::HOOK_ARGUMENT,
        route_hook_argument = crate::fastctx_route_gate::HOOK_ARGUMENT,
    );
    let current = format!(
        r#"{applied}
[[hooks.PreToolUse]]
matcher = "MCP"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/user/concurrent"
timeout = 7

[hooks.state."user:pre_tool_use:2:0"]
trusted_hash = "sha256:user"
"#,
    )
    .replacen(
        "timeout = 5\n\n[hooks.state",
        "timeout = 30\nstatusMessage = \"concurrently edited gate\"\n\n[hooks.state",
        1,
    );

    let restored = restore_owned_config_changes(original, &applied, &current)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let groups = restored["hooks"]["PreToolUse"]
        .as_array_of_tables()
        .unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups.get(0).unwrap()["hooks"]
            .as_array_of_tables()
            .unwrap()
            .get(0)
            .unwrap()["command"]
            .as_str(),
        Some("/user/pre-tool")
    );
    assert_eq!(
        groups.get(1).unwrap()["hooks"]
            .as_array_of_tables()
            .unwrap()
            .get(0)
            .unwrap()["command"]
            .as_str(),
        Some("/user/concurrent")
    );
    assert!(
        restored["hooks"]["state"]
            .as_table()
            .unwrap()
            .get("/tmp/config.toml:pre_tool_use:1:0")
            .is_none()
    );
    assert_eq!(
        restored["hooks"]["state"]["user:pre_tool_use:2:0"]["trusted_hash"].as_str(),
        Some("sha256:user")
    );
    assert!(
        !restored
            .to_string()
            .contains(crate::subagent_gate::HOOK_ARGUMENT)
    );
    assert!(
        !restored
            .to_string()
            .contains(crate::fastctx_route_gate::HOOK_ARGUMENT)
    );
}

#[test]
fn restore_removes_codey_gate_from_inline_hook_arrays() {
    let original = r#"
[hooks]
PreToolUse = []
"#;
    let applied = format!(
        r#"
[hooks]
PreToolUse = [{{ matcher = "*", hooks = [{{ type = "command", command = "'/tmp/codey' {hook_argument}", commandWindows = "C:\\Codey\\codey.exe {hook_argument}", timeout = 5 }}] }}]
"#,
        hook_argument = crate::subagent_gate::HOOK_ARGUMENT,
    );
    let current = format!(
        r#"
[hooks]
PreToolUse = [{{ matcher = "*", hooks = [{{ type = "command", command = "'/tmp/codey' {hook_argument}", commandWindows = "C:\\Codey\\codey.exe {hook_argument}", timeout = 30, statusMessage = "concurrently edited gate" }}] }}, {{ matcher = "MCP", hooks = [{{ type = "command", command = "/user/concurrent", timeout = 7 }}] }}]
"#,
        hook_argument = crate::subagent_gate::HOOK_ARGUMENT,
    );

    let restored = restore_owned_config_changes(original, &applied, &current)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let groups = restored["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups.get(0).unwrap().as_inline_table().unwrap()["hooks"]
            .as_array()
            .unwrap()
            .get(0)
            .unwrap()
            .as_inline_table()
            .unwrap()["command"]
            .as_str(),
        Some("/user/concurrent")
    );
    assert!(
        !restored
            .to_string()
            .contains(crate::subagent_gate::HOOK_ARGUMENT)
    );
}

#[test]
fn restore_reverts_inline_fastctx_namespace_changes_after_concurrent_edits() {
    let original = r#"
features = { code_mode = { direct_only_tool_namespaces = ["mcp__existing", "mcp__codey_fastctx"] }, user_flag = "original" }
"#;
    let applied = r#"
features = { code_mode = { direct_only_tool_namespaces = ["mcp__existing"] }, user_flag = "original" }
"#;
    let current = r#"
features = { code_mode = { direct_only_tool_namespaces = ["mcp__existing", "mcp__concurrent"] }, user_flag = "concurrent" }
"#;

    let restored = restore_owned_config_changes(original, applied, current)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let namespaces = direct_only_tool_namespaces(&restored).unwrap();

    assert!(
        namespaces
            .iter()
            .any(|entry| { entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE) })
    );
    assert!(
        namespaces
            .iter()
            .any(|entry| { entry.as_str() == Some("mcp__concurrent") })
    );
    assert_eq!(
        restored["features"]["user_flag"].as_str(),
        Some("concurrent")
    );
}

#[test]
fn restore_removes_dynamic_user_fastctx_guidance_after_concurrent_edits() {
    let user_fastctx_guidance = codey_fastctx_guidance_for_namespace("mcp__fastctx");
    let original = r#"developer_instructions = "User guidance""#;
    let applied = format!(r#"developer_instructions = "User guidance\n\n{user_fastctx_guidance}""#);
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

#[test]
fn isolated_fastctx_installs_route_hook_without_subagent_optimization() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex-home");
    let state_dir = temp.path().join("codey-state");
    let marker = state_dir.join("codex-lease.json");
    let backup_root = state_dir.join("codex-backups");
    fs::create_dir_all(&home).unwrap();
    let original_config = br#"model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
"#;
    fs::write(home.join("config.toml"), original_config).unwrap();

    let applied = apply_isolated_cc_switch_runtime_config(
        &home,
        &direct_profile(RelayProtocol::Responses),
        "relay",
        Some(Path::new("/opt/codey/codey-fastctx")),
        false,
        DEFAULT_SUBAGENT_MODEL,
        DEFAULT_SUBAGENT_REASONING_EFFORT,
        None,
        None,
        Some(original_config),
        &marker,
        &backup_root,
    )
    .unwrap();

    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry == "features.hooks=true")
    );
    assert_eq!(
        applied
            .runtime_config_overrides
            .iter()
            .filter(|entry| entry.starts_with("hooks.state."))
            .count(),
        FASTCTX_ROUTE_HOOKS.len()
    );
    assert!(
        !applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry.starts_with("features.multi_agent_v2."))
    );
    let hooks: serde_json::Value =
        serde_json::from_slice(&fs::read(home.join("hooks.json")).unwrap()).unwrap();
    let groups = hooks["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0]["matcher"].as_str(),
        Some(crate::fastctx_route_gate::HOOK_MATCHER)
    );
    assert!(
        groups[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(crate::fastctx_route_gate::HOOK_ARGUMENT)
    );

    assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), original_config);
    assert!(!home.join("hooks.json").exists());
}

#[test]
fn cc_switch_runtime_constraints_stay_out_of_config_and_restore_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex-home");
    let state_dir = temp.path().join("codey-state");
    let marker = state_dir.join("codex-lease.json");
    let backup_root = state_dir.join("codex-backups");
    fs::create_dir_all(&home).unwrap();
    let original_config = br#"model_provider = "relay"
model_catalog_json = "/cc-switch/catalog.json"
developer_instructions = "Keep the user's instructions."

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
"#;
    let original_hooks = br#"{
  "description": "User hooks",
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "/usr/bin/true", "timeout": 2 }
        ]
      }
    ]
  }
}
"#;
    fs::write(home.join("config.toml"), original_config).unwrap();
    fs::write(home.join("hooks.json"), original_hooks).unwrap();
    let model_catalog_path = home.join(crate::model_catalog::relative_path());
    fs::create_dir_all(model_catalog_path.parent().unwrap()).unwrap();
    fs::write(
        &model_catalog_path,
        serde_json::to_vec(&serde_json::json!({
            "models": [{
                "slug": "gpt-5.6-luna",
                "description": "Luna test model",
                "base_instructions": "Test instructions",
                "multi_agent_version": "v2"
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let seeded_constraints_dir = state_dir.join(CODEY_CONSTRAINTS_DIR);
    fs::create_dir_all(&seeded_constraints_dir).unwrap();
    fs::write(
        seeded_constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE),
        PREVIOUS_SUBAGENT_GUIDANCE_V2,
    )
    .unwrap();
    fs::write(
        seeded_constraints_dir.join(CODEY_FASTCTX_INSTRUCTIONS_FILE),
        PREVIOUS_CODEY_FASTCTX_GUIDANCE_V5,
    )
    .unwrap();
    fs::write(
        seeded_constraints_dir.join(CODEY_COLLABORATION_HINT_FILE),
        PREVIOUS_ROOT_AGENT_COLLABORATION_USAGE_HINT,
    )
    .unwrap();
    let profile = direct_profile(RelayProtocol::Responses);

    let applied = apply_isolated_cc_switch_runtime_config(
        &home,
        &profile,
        "relay",
        Some(Path::new("/opt/codey/codey-fastctx")),
        true,
        "gpt-5.6-mini",
        "high",
        None,
        None,
        Some(original_config),
        &marker,
        &backup_root,
    )
    .unwrap();

    assert_eq!(applied.config_contents, original_config);
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), original_config);
    assert!(!home.join("AGENTS.md").exists());
    assert!(!home.join("agents/default.toml").exists());
    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry.starts_with("developer_instructions="))
    );
    let model_catalog_override = applied
        .runtime_config_overrides
        .iter()
        .find(|entry| entry.starts_with("model_catalog_json="))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let expected_model_catalog_path = model_catalog_path.to_string_lossy().into_owned();
    assert_eq!(
        model_catalog_override["model_catalog_json"].as_str(),
        Some(expected_model_catalog_path.as_str())
    );
    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry == "agents.enabled=true")
    );
    assert!(applied.runtime_config_overrides.iter().any(|entry| entry
        == &format!(
            "agents.max_concurrent_threads_per_session={DEFAULT_SUBAGENT_MAX_CONCURRENCY}"
        )));
    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry == "agents.default_subagent_model=\"gpt-5.6-mini\"")
    );
    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry.starts_with("agents.default.config_file="))
    );
    for role in SUBAGENT_ROLE_IDS {
        for field in ["config_file", "description"] {
            let key = format!("agents.{role}.{field}");
            assert!(
                applied
                    .runtime_config_overrides
                    .iter()
                    .any(|entry| entry.starts_with(&format!("{key}="))),
                "missing runtime override {key}"
            );
        }
    }
    let pre_tool_state_key = format!("{}:pre_tool_use:1:0", home.join("hooks.json").display());
    let pre_tool_prefix = format!(
        "hooks.state.{}.trusted_hash=",
        toml_string_literal(&pre_tool_state_key)
    );
    let hook_commands =
        crate::subagent_gate::hook_commands_for(crate::subagent_gate::COMBINED_HOOK_ARGUMENT)
            .unwrap();
    let selected_command = if cfg!(windows) {
        hook_commands.command_windows.as_str()
    } else {
        hook_commands.command.as_str()
    };
    let expected_pre_tool_hash = crate::subagent_gate::hook_trust_hash(
        "pre_tool_use",
        Some("*"),
        selected_command,
        crate::subagent_gate::HOOK_TIMEOUT_SECONDS,
    );
    assert!(applied.runtime_config_overrides.iter().any(|entry| {
        entry.starts_with(&pre_tool_prefix) && entry.contains(&expected_pre_tool_hash)
    }));
    assert!(
        applied
            .runtime_config_overrides
            .iter()
            .any(|entry| entry.starts_with("mcp_servers.codey_fastctx.command="))
    );
    for required_key in [
        "model_catalog_json",
        "desktop.enabled-reasoning-efforts",
        "service_tier",
        "developer_instructions",
        "mcp_servers.codey_fastctx.command",
        "mcp_servers.codey_fastctx.args",
        "mcp_servers.codey_fastctx.startup_timeout_sec",
        "mcp_servers.codey_fastctx.tool_timeout_sec",
        "mcp_servers.codey_fastctx.env.FASTCTX_TOKEN_BUDGET",
        "mcp_servers.codey_fastctx.env.FASTCTX_GREP_TOKEN_BUDGET",
        "mcp_servers.codey_fastctx.env.FASTCTX_GLOB_TOKEN_BUDGET",
        "tool_output_token_limit",
        "agents.enabled",
        "agents.max_concurrent_threads_per_session",
        "agents.default_subagent_model",
        "agents.default_subagent_reasoning_effort",
        "agents.default.config_file",
        "agents.default.description",
        "features.multi_agent_v2.enabled",
        "features.multi_agent_v2.wait_agent_enabled",
        "features.multi_agent_v2.hide_spawn_agent_metadata",
        "features.multi_agent_v2.expose_spawn_agent_model_overrides",
        "features.multi_agent_v2.tool_namespace",
        "features.multi_agent_v2.min_wait_timeout_ms",
        "features.multi_agent_v2.default_wait_timeout_ms",
        "features.multi_agent_v2.max_wait_timeout_ms",
        "features.multi_agent_v2.root_agent_usage_hint_text",
        "features.multi_agent_v2.multi_agent_mode_hint_text",
        "features.multi_agent_v2.subagent_developer_instructions",
        "features.hooks",
    ] {
        assert!(
            applied
                .runtime_config_overrides
                .iter()
                .any(|entry| entry.starts_with(&format!("{required_key}="))),
            "missing runtime override {required_key}"
        );
    }
    assert_eq!(
        applied
            .runtime_config_overrides
            .iter()
            .filter(|entry| entry.starts_with("hooks.state."))
            .count(),
        SUBAGENT_GATE_HOOKS.len()
    );
    assert_eq!(
        applied
            .runtime_config_overrides
            .iter()
            .filter(|entry| entry.starts_with(CODEY_WSL_ONLY_OVERRIDE_PREFIX))
            .count(),
        if cfg!(windows) {
            SUBAGENT_GATE_HOOKS.len()
        } else {
            0
        }
    );
    for runtime_override in &applied.runtime_config_overrides {
        let runtime_override = runtime_override
            .strip_prefix(CODEY_WSL_ONLY_OVERRIDE_PREFIX)
            .unwrap_or(runtime_override);
        runtime_override
            .parse::<DocumentMut>()
            .unwrap_or_else(|error| {
                panic!("invalid runtime override {runtime_override:?}: {error}")
            });
    }

    let hooks: serde_json::Value =
        serde_json::from_slice(&fs::read(home.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(hooks["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    assert_eq!(
        hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str(),
        Some("/usr/bin/true")
    );
    assert_eq!(
        hooks["hooks"]["PreToolUse"][1]["matcher"].as_str(),
        Some("*")
    );
    assert!(
        hooks["hooks"]["PreToolUse"][1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(crate::subagent_gate::COMBINED_HOOK_ARGUMENT)
    );
    let windows_command = hooks["hooks"]["PreToolUse"][1]["hooks"][0]["commandWindows"]
        .as_str()
        .unwrap();
    assert!(windows_command.starts_with("& '"), "{windows_command}");
    assert!(windows_command.contains(crate::subagent_gate::COMBINED_HOOK_ARGUMENT));
    for event in [
        "PostToolUse",
        "SubagentStart",
        "SubagentStop",
        "Stop",
        "SessionEnd",
    ] {
        assert_eq!(
            hooks["hooks"][event].as_array().unwrap().len(),
            1,
            "{event}"
        );
    }
    assert_eq!(
        hooks["hooks"]["PostToolUse"][0]["matcher"].as_str(),
        Some(crate::subagent_orchestrator::POST_TOOL_HOOK_MATCHER)
    );
    let constraints_dir = state_dir.join(CODEY_CONSTRAINTS_DIR);
    assert_eq!(
        fs::read_to_string(constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE)).unwrap(),
        SUBAGENT_GUIDANCE
    );
    assert!(constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE).exists());
    assert!(
        constraints_dir
            .join(CODEY_FASTCTX_INSTRUCTIONS_FILE)
            .exists()
    );
    assert_eq!(
        fs::read_to_string(constraints_dir.join(CODEY_FASTCTX_INSTRUCTIONS_FILE)).unwrap(),
        CODEY_FASTCTX_GUIDANCE
    );
    assert_eq!(
        fs::read_to_string(constraints_dir.join(CODEY_COLLABORATION_HINT_FILE)).unwrap(),
        ROOT_AGENT_COLLABORATION_USAGE_HINT
    );
    assert!(constraints_dir.join(CODEY_SUBAGENT_SOURCE_FILE).exists());
    assert!(
        constraints_dir
            .join(CODEY_RUNTIME_DEFAULT_AGENT_FILE)
            .exists()
    );
    for role in SUBAGENT_ROLE_IDS {
        let runtime_path = if role == SUBAGENT_ROLE_DEFAULT {
            constraints_dir.join(CODEY_RUNTIME_DEFAULT_AGENT_FILE)
        } else {
            assert!(
                constraints_dir
                    .join(CODEY_SUBAGENT_SOURCES_DIR)
                    .join(format!("{role}.toml"))
                    .exists(),
                "missing editable source for {role}"
            );
            constraints_dir
                .join(CODEY_RUNTIME_AGENTS_DIR)
                .join(format!("{role}.toml"))
        };
        let runtime = fs::read_to_string(runtime_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(runtime["name"].as_str(), Some(role));
        assert_eq!(runtime["model"].as_str(), Some("gpt-5.6-mini"));
        assert_eq!(runtime["model_reasoning_effort"].as_str(), Some("high"));
    }

    let switched_config = [
        original_config.as_slice(),
        b"\n# CC Switch changed route state\n",
    ]
    .concat();
    fs::write(home.join("config.toml"), &switched_config).unwrap();
    assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), switched_config);
    assert_eq!(fs::read(home.join("hooks.json")).unwrap(), original_hooks);
    assert!(!marker.exists());
    fs::write(home.join("config.toml"), original_config).unwrap();

    fs::write(
        constraints_dir.join(CODEY_ROOT_INSTRUCTIONS_FILE),
        "CUSTOM ROOT CONSTRAINT",
    )
    .unwrap();
    fs::write(
        constraints_dir.join(CODEY_FASTCTX_INSTRUCTIONS_FILE),
        "CUSTOM FASTCTX CONSTRAINT",
    )
    .unwrap();
    fs::write(
        constraints_dir.join(CODEY_COLLABORATION_HINT_FILE),
        "CUSTOM COLLABORATION HINT",
    )
    .unwrap();
    fs::write(
        constraints_dir.join(CODEY_SUBAGENT_SOURCE_FILE),
        r#"name = "default"
description = "Custom editable subagent"
developer_instructions = "CUSTOM SUBAGENT CONSTRAINT"
"#,
    )
    .unwrap();

    let reapplied = apply_isolated_cc_switch_runtime_config(
        &home,
        &profile,
        "relay",
        Some(Path::new("/opt/codey/codey-fastctx")),
        true,
        "gpt-5.6-mini",
        "high",
        None,
        None,
        Some(original_config),
        &marker,
        &backup_root,
    )
    .unwrap();
    let developer_override = reapplied
        .runtime_config_overrides
        .iter()
        .find(|entry| entry.starts_with("developer_instructions="))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let developer_instructions = developer_override["developer_instructions"]
        .as_str()
        .unwrap();
    assert!(developer_instructions.contains("CUSTOM ROOT CONSTRAINT"));
    assert!(developer_instructions.contains("CUSTOM FASTCTX CONSTRAINT"));
    assert!(!developer_instructions.contains(SUBAGENT_GUIDANCE));
    assert!(!developer_instructions.contains(CODEY_FASTCTX_GUIDANCE));
    let collaboration_override = reapplied
        .runtime_config_overrides
        .iter()
        .find(|entry| entry.starts_with("features.multi_agent_v2.root_agent_usage_hint_text="))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(
        collaboration_override["features"]["multi_agent_v2"]["root_agent_usage_hint_text"].as_str(),
        Some("CUSTOM COLLABORATION HINT")
    );
    let runtime_agent =
        fs::read_to_string(constraints_dir.join(CODEY_RUNTIME_DEFAULT_AGENT_FILE)).unwrap();
    assert!(runtime_agent.contains("CUSTOM SUBAGENT CONSTRAINT"));
    assert!(runtime_agent.contains("CUSTOM FASTCTX CONSTRAINT"));
    assert!(restore_runtime_provider_config_at(&home, &marker).unwrap());
}
