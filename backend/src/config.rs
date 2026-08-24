use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::notifications::WebhookConfig;
use crate::{local_router, model_catalog, model_id};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_upstream_protocol")]
    pub upstream_protocol: String,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    #[serde(default)]
    pub api_key_configured: bool,
    #[serde(default, skip_serializing)]
    pub clear_api_key: bool,
    /// Request-only provider headers loaded from the active Codex/CC Switch
    /// source. They may contain credentials, so they are never serialized into
    /// Codey's store or exposed to the renderer.
    #[serde(skip)]
    pub model_request_headers: BTreeMap<String, String>,
    /// Stable id of the Codex provider in cc-switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc_switch_provider_id: Option<String>,
    #[serde(default)]
    pub cc_switch_read_only: bool,
    /// Preserve the exact Codex provider identity required for remote
    /// compaction when it was explicitly enabled by the source configuration.
    #[serde(default)]
    pub supports_remote_compaction: bool,
}

pub const DERIVED_OFFICIAL_PROFILE_ID: &str = "codey-official-account";

impl ProviderProfile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            base_url: String::new(),
            api_key: String::new(),
            upstream_protocol: default_upstream_protocol(),
            auth_mode: default_auth_mode(),
            api_key_configured: false,
            clear_api_key: false,
            model_request_headers: BTreeMap::new(),
            cc_switch_provider_id: None,
            cc_switch_read_only: false,
            supports_remote_compaction: false,
        }
    }

    pub fn normalized_base_url(&self) -> String {
        self.base_url.trim().trim_end_matches('/').to_string()
    }

    /// The provider id passed to Codex and used by every route-scoped model
    /// map. CC Switch routes keep their source provider identity; Codey-owned
    /// routes use the profile id directly.
    pub fn provider_id(&self) -> &str {
        self.cc_switch_provider_id
            .as_deref()
            .unwrap_or(self.id.as_str())
    }

    pub(crate) fn runtime_wire_api(&self) -> Result<&'static str, String> {
        match self.upstream_protocol.as_str() {
            UPSTREAM_PROTOCOL_OFFICIAL
            | UPSTREAM_PROTOCOL_OPENAI_RESPONSES
            | UPSTREAM_PROTOCOL_OPENAI_COMPATIBLE => Ok("responses"),
            protocol => Err(format!(
                "线路「{}」使用了不支持的上游协议：{protocol}",
                self.name
            )),
        }
    }

    pub(crate) fn is_unconfigured_default(&self) -> bool {
        self.name == "默认配置"
            && self.base_url.trim().is_empty()
            && self.api_key.trim().is_empty()
            && !self.api_key_configured
            && !self.cc_switch_read_only
    }

    pub(crate) fn normalize(&mut self) {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = "未命名线路".to_string();
        }
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.api_key = self.api_key.trim().to_string();
        self.cc_switch_provider_id = self
            .cc_switch_provider_id
            .take()
            .map(|provider_id| provider_id.trim().to_string())
            .filter(|provider_id| !provider_id.is_empty());
        self.upstream_protocol = normalize_upstream_protocol(&self.upstream_protocol);
        self.auth_mode = normalize_auth_mode(&self.auth_mode, self.cc_switch_read_only);
        if self.auth_mode == AUTH_MODE_OFFICIAL_ACCOUNT {
            self.cc_switch_read_only = true;
            self.api_key.clear();
            self.supports_remote_compaction = true;
            self.upstream_protocol = UPSTREAM_PROTOCOL_OFFICIAL.to_string();
            self.auth_mode = AUTH_MODE_OFFICIAL_ACCOUNT.to_string();
        } else {
            self.cc_switch_read_only = false;
            if self.upstream_protocol == UPSTREAM_PROTOCOL_OFFICIAL {
                self.upstream_protocol = UPSTREAM_PROTOCOL_OPENAI_RESPONSES.to_string();
            }
        }
        self.api_key_configured = !self.api_key.is_empty();
        self.clear_api_key = false;
    }

    pub fn merge_redacted_secret(&mut self, previous: Option<&Self>) {
        if self.clear_api_key {
            self.api_key.clear();
            self.api_key_configured = false;
            return;
        }
        if !self.api_key.trim().is_empty() || !self.api_key_configured {
            return;
        }
        if let Some(previous) = previous {
            self.api_key = previous.api_key.clone();
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("线路 ID 不能为空".to_string());
        }
        if self.provider_id() == local_router::ROUTER_PROVIDER_ID {
            return Err(format!(
                "线路不能使用 Codey 内部 Provider ID「{}」",
                local_router::ROUTER_PROVIDER_ID
            ));
        }
        let name = self.name.trim();
        if name.is_empty() {
            return Err("线路名称不能为空".to_string());
        }
        if self.auth_mode == AUTH_MODE_OFFICIAL_ACCOUNT || self.cc_switch_read_only {
            return Ok(());
        }
        let base_url = self.base_url.trim();
        if base_url.is_empty() {
            return Err(format!("线路「{name}」缺少 API URL"));
        }
        let url = reqwest::Url::parse(base_url)
            .map_err(|_| format!("线路「{name}」的 API URL 格式无效"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(format!("线路「{name}」的 API URL 必须是 HTTP(S) 地址"));
        }
        self.runtime_wire_api()?;
        if self.api_key.trim().is_empty() {
            return Err(format!("线路「{name}」缺少第三方 API Key"));
        }
        Ok(())
    }
}

/// Prompt-optimization settings. The API key follows the notification-channel
/// credential pattern: redacted to the renderer, restored on save, cleared
/// only on explicit request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptOptimizationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_configured: bool,
    #[serde(default, skip_serializing)]
    pub clear_api_key: bool,
    #[serde(default)]
    pub model: String,
    /// Optional custom optimizer instructions. When empty the built-in
    /// default system prompt is used.
    #[serde(default)]
    pub instruction: String,
}

impl PromptOptimizationConfig {
    pub(crate) fn normalize(&mut self) {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.api_key = self.api_key.trim().to_string();
        self.api_key_configured = !self.api_key.is_empty();
        self.clear_api_key = false;
        self.model = self.model.trim().to_string();
        self.instruction = self.instruction.trim().to_string();
    }

    pub fn merge_redacted_secrets(&mut self, previous: &Self) {
        if self.clear_api_key {
            self.api_key.clear();
            self.api_key_configured = false;
            return;
        }
        if !self.api_key.trim().is_empty() || !self.api_key_configured {
            return;
        }
        self.api_key = previous.api_key.clone();
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        let base_url = self.base_url.trim();
        if base_url.is_empty() {
            return Ok(());
        }
        let url = reqwest::Url::parse(base_url)
            .map_err(|_| "提示词优化 API 地址不是有效的 HTTP(S) 地址".to_string())?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("提示词优化 API 地址必须是有效的 HTTP(S) 地址".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum GpuLaunchMode {
    #[default]
    Off,
    DisableGpu,
    DisableGpuRasterization,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRoleConfig {
    #[serde(default = "default_subagent_model")]
    pub model: String,
    #[serde(default = "default_subagent_reasoning_effort")]
    pub reasoning_effort: String,
}

impl SubagentRoleConfig {
    pub fn new(model: impl Into<String>, reasoning_effort: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            reasoning_effort: reasoning_effort.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentProviderConfig {
    #[serde(default = "default_subagent_model")]
    pub model: String,
    #[serde(default = "default_subagent_reasoning_effort")]
    pub reasoning_effort: String,
    #[serde(default)]
    pub roles: BTreeMap<String, SubagentRoleConfig>,
}

impl Default for SubagentProviderConfig {
    fn default() -> Self {
        Self {
            model: default_subagent_model(),
            reasoning_effort: default_subagent_reasoning_effort(),
            roles: default_subagent_roles(),
        }
    }
}

impl SubagentProviderConfig {
    fn normalize(&mut self) {
        normalize_subagent_config(&mut self.model, &mut self.reasoning_effort, &mut self.roles);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeyConfig {
    #[serde(default)]
    pub settings_revision: u64,
    #[serde(default)]
    pub active_profile_id: String,
    #[serde(default)]
    pub profiles: Vec<ProviderProfile>,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub prompt_optimization: PromptOptimizationConfig,
    #[serde(default)]
    pub codex_app_path: String,
    #[serde(default)]
    pub user_scripts: Vec<String>,
    /// Codey-owned model selections. Provider connection data remains owned
    /// by cc-switch (or the local Codex configuration).
    #[serde(default)]
    pub selected_models_by_provider: BTreeMap<String, Vec<String>>,
    /// Third-party model IDs that were explicitly typed by the user. Synced
    /// provider models are intentionally excluded so only manual entries can be
    /// deleted from Codey's saved support list.
    #[serde(default)]
    pub manual_third_party_models_by_provider: BTreeMap<String, Vec<String>>,
    /// Official model IDs that the user explicitly confirmed as supported by
    /// each third-party provider. Kept separate from synchronized results so a
    /// later model-list refresh cannot erase the user's declaration.
    #[serde(default)]
    pub declared_official_models_by_provider: BTreeMap<String, Vec<String>>,
    /// Effective provider model support after combining the last synchronized
    /// result with user-confirmed model declarations.
    #[serde(default)]
    pub upstream_models_by_provider: BTreeMap<String, Vec<String>>,
    /// Codey-owned default model selection per provider. Empty or unavailable
    /// values fall back to the first selectable official model.
    #[serde(default)]
    pub default_model_by_provider: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub disable_trace_log_writes: bool,
    /// Keeps Codex/ChatGPT Crashpad pending reports below a bounded disk
    /// budget. The guard only manages validated report files on macOS.
    #[serde(default = "default_true")]
    pub protect_crashpad_pending: bool,
    #[serde(default = "default_true")]
    pub slim_codex_pet: bool,
    /// Selects at most one Chromium GPU diagnostic argument for the next
    /// Codey-managed Codex launch. Disabled by default and ignored on macOS.
    #[serde(default)]
    pub gpu_launch_mode: GpuLaunchMode,
    /// Publishes Codey's embedded FastCtx file tools to Codex for the next
    /// runtime. Disabled by default so existing tool behavior is unchanged.
    #[serde(default)]
    pub fast_context_tools: bool,
    /// Temporarily enables Codey's opinionated Codex multi-agent V2 setup for
    /// the next runtime. Disabled by default and restored on shutdown.
    #[serde(default)]
    pub subagent_optimization: bool,
    /// Default model used by newly spawned subagents while Codey's
    /// multi-agent optimization is enabled.
    #[serde(default = "default_subagent_model")]
    pub subagent_model: String,
    /// Default reasoning effort used by newly spawned subagents.
    #[serde(default = "default_subagent_reasoning_effort")]
    pub subagent_reasoning_effort: String,
    /// Per-task agent selections. The legacy scalar defaults above mirror the
    /// `default` role so older Codey stores and Codex builds remain readable.
    #[serde(default)]
    pub subagent_roles: BTreeMap<String, SubagentRoleConfig>,
    /// Per-provider subagent selections. The scalar and role fields above are
    /// the active provider's compatibility representation.
    #[serde(default)]
    pub subagent_config_by_provider: BTreeMap<String, SubagentProviderConfig>,
    /// Tracks the one-time migration that turns existing non-default role
    /// selections into provider-scoped official-model declarations.
    #[serde(default)]
    pub subagent_role_model_support_migrated: bool,
    /// Tracks whether Codey has already consumed the one-time default route
    /// import window. Existing non-empty configs are treated as already
    /// initialized so later launches never overwrite saved third-party routes
    /// from the ambient Codex configuration.
    #[serde(default)]
    pub initial_route_import_completed: bool,
    /// Automatically dismisses Codex's full-access safety notice in the
    /// renderer. Opt-in so the native warning remains visible by default.
    #[serde(default)]
    pub hide_full_access_warning: bool,
    /// Shows the current ChatGPT account rate-limit windows in the Codex
    /// header. The renderer only activates this for an official login route.
    #[serde(default = "default_true")]
    pub show_account_usage_in_header: bool,
    /// Launch-scoped authentication capability captured from Codex before
    /// Codey's temporary provider overrides are applied. It is intentionally
    /// never persisted or exposed as part of the editable configuration.
    #[serde(skip)]
    pub official_account_available_this_launch: bool,
    /// Public HTTPS endpoint for the version manifest published to Cloudflare R2.
    /// This is build-time configuration, not a user setting.
    #[serde(
        default = "default_update_manifest_url",
        skip_serializing,
        skip_deserializing
    )]
    pub update_manifest_url: String,
}

impl Default for CodeyConfig {
    fn default() -> Self {
        let profile = ProviderProfile::new("默认配置");
        Self {
            settings_revision: 0,
            active_profile_id: profile.id.clone(),
            profiles: vec![profile],
            webhook: WebhookConfig::default(),
            prompt_optimization: PromptOptimizationConfig::default(),
            codex_app_path: String::new(),
            user_scripts: Vec::new(),
            selected_models_by_provider: BTreeMap::new(),
            manual_third_party_models_by_provider: BTreeMap::new(),
            declared_official_models_by_provider: BTreeMap::new(),
            upstream_models_by_provider: BTreeMap::new(),
            default_model_by_provider: BTreeMap::new(),
            disable_trace_log_writes: true,
            protect_crashpad_pending: true,
            slim_codex_pet: true,
            gpu_launch_mode: GpuLaunchMode::Off,
            fast_context_tools: false,
            subagent_optimization: false,
            subagent_model: default_subagent_model(),
            subagent_reasoning_effort: default_subagent_reasoning_effort(),
            subagent_roles: default_subagent_roles(),
            subagent_config_by_provider: BTreeMap::new(),
            subagent_role_model_support_migrated: true,
            initial_route_import_completed: false,
            hide_full_access_warning: false,
            show_account_usage_in_header: true,
            official_account_available_this_launch: false,
            update_manifest_url: default_update_manifest_url(),
        }
    }
}

impl CodeyConfig {
    pub fn normalize(mut self) -> Self {
        self.update_manifest_url = default_update_manifest_url();
        self.profiles
            .retain(|profile| !profile.id.trim().is_empty());
        for profile in &mut self.profiles {
            profile.normalize();
        }
        if self.profiles.is_empty() {
            let profile = ProviderProfile::new("默认配置");
            self.active_profile_id = profile.id.clone();
            self.profiles.push(profile);
        }
        if !self
            .profiles
            .iter()
            .any(|profile| profile.id == self.active_profile_id)
        {
            self.active_profile_id = self.profiles[0].id.clone();
        }
        let official_provider_ids = self
            .profiles
            .iter()
            .filter(|profile| profile.cc_switch_read_only)
            .map(|profile| profile.provider_id().to_string())
            .collect::<BTreeSet<_>>();
        normalize_model_lists(&mut self.selected_models_by_provider);
        normalize_model_lists(&mut self.manual_third_party_models_by_provider);
        normalize_model_lists(&mut self.declared_official_models_by_provider);
        migrate_legacy_official_model_selections(
            &mut self.selected_models_by_provider,
            &mut self.manual_third_party_models_by_provider,
            &mut self.declared_official_models_by_provider,
            &official_provider_ids,
        );
        normalize_upstream_model_lists(&mut self.upstream_models_by_provider);
        merge_declared_official_models_into_upstream(
            &self.declared_official_models_by_provider,
            &mut self.upstream_models_by_provider,
        );
        normalize_model_map(&mut self.default_model_by_provider);
        normalize_subagent_config(
            &mut self.subagent_model,
            &mut self.subagent_reasoning_effort,
            &mut self.subagent_roles,
        );
        self.subagent_config_by_provider
            .retain(|provider_id, selection| {
                selection.normalize();
                !provider_id.trim().is_empty()
            });
        if let Some(provider_id) = self.current_provider_id().map(ToString::to_string) {
            let active = self.active_subagent_config();
            self.subagent_config_by_provider
                .entry(provider_id)
                .or_insert(active);
        }
        if !self.subagent_role_model_support_migrated {
            self.migrate_custom_subagent_role_model_support();
            self.subagent_role_model_support_migrated = true;
        }
        if !self.initial_route_import_completed && !self.looks_like_empty_default_route() {
            self.initial_route_import_completed = true;
        }
        self.webhook.normalize();
        self.prompt_optimization.normalize();
        self
    }

    pub(crate) fn apply_launch_official_profile(
        &mut self,
        official_profile: Option<ProviderProfile>,
    ) {
        self.remember_current_subagent_config();
        let previous_active_id = self.active_profile_id.clone();
        let placeholder_provider_id = self
            .looks_like_empty_default_route()
            .then(|| self.profiles[0].provider_id().to_string());
        if self.looks_like_empty_default_route() {
            self.profiles.clear();
        } else {
            self.profiles.retain(|profile| !profile.cc_switch_read_only);
        }
        // The launch-derived official profile may disappear on an API-key
        // launch and return on a later official launch. Keep its provider-scoped
        // model/default/subagent preferences across that temporary absence.
        // Only the disposable empty placeholder owns data that can be removed.
        if let Some(provider_id) = placeholder_provider_id {
            self.selected_models_by_provider.remove(&provider_id);
            self.manual_third_party_models_by_provider
                .remove(&provider_id);
            self.declared_official_models_by_provider
                .remove(&provider_id);
            self.upstream_models_by_provider.remove(&provider_id);
            self.default_model_by_provider.remove(&provider_id);
            self.subagent_config_by_provider.remove(&provider_id);
        }
        if let Some(mut official_profile) = official_profile {
            official_profile.id = DERIVED_OFFICIAL_PROFILE_ID.to_string();
            official_profile.normalize();
            let official_provider_id = official_profile.provider_id().to_string();
            if let Some(existing) = self
                .profiles
                .iter_mut()
                .find(|profile| profile.id == DERIVED_OFFICIAL_PROFILE_ID)
            {
                *existing = official_profile;
            } else {
                self.profiles.insert(0, official_profile);
            }
            self.selected_models_by_provider
                .entry(official_provider_id)
                .or_insert_with(model_catalog::default_official_model_slugs);
        }
        if self
            .profiles
            .iter()
            .any(|profile| profile.id == previous_active_id)
        {
            self.active_profile_id = previous_active_id;
        } else if let Some(profile) = self.profiles.first() {
            self.active_profile_id = profile.id.clone();
        }
        self.restore_current_subagent_config();
    }

    pub fn active_profile(&self) -> Option<ProviderProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id == self.active_profile_id)
            .cloned()
            .or_else(|| self.profiles.first().cloned())
    }

    pub fn current_provider_id(&self) -> Option<&str> {
        self.profiles
            .iter()
            .find(|profile| profile.id == self.active_profile_id)
            .map(ProviderProfile::provider_id)
    }

    pub fn selected_models(&self) -> &[String] {
        self.current_provider_id()
            .and_then(|provider_id| self.selected_models_by_provider.get(provider_id))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Models enabled on an API-key route. Legacy official-looking model IDs
    /// are stored separately for backward compatibility, but they still belong
    /// to this route and must be routed by provenance rather than by name.
    pub(crate) fn enabled_route_models(&self, provider_id: &str) -> Vec<String> {
        let selected = self
            .selected_models_by_provider
            .get(provider_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let declared = self
            .declared_official_models_by_provider
            .get(provider_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        model_id::dedupe_preserving_first(
            selected.iter().chain(declared.iter()).map(String::as_str),
        )
    }

    pub fn manual_third_party_models(&self) -> &[String] {
        self.current_provider_id()
            .and_then(|provider_id| self.manual_third_party_models_by_provider.get(provider_id))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn declared_official_models(&self) -> &[String] {
        self.current_provider_id()
            .and_then(|provider_id| self.declared_official_models_by_provider.get(provider_id))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn upstream_models_snapshot(&self) -> Option<&[String]> {
        self.current_provider_id()
            .and_then(|provider_id| self.upstream_models_by_provider.get(provider_id))
            .map(Vec::as_slice)
    }

    pub fn default_model(&self) -> Option<&str> {
        self.current_provider_id()
            .and_then(|provider_id| self.default_model_by_provider.get(provider_id))
            .map(String::as_str)
    }

    pub fn has_third_party_route(&self) -> bool {
        self.profiles
            .iter()
            .any(|profile| !profile.cc_switch_read_only)
    }

    pub(crate) fn looks_like_empty_default_route(&self) -> bool {
        let Some(profile) = self.profiles.first() else {
            return true;
        };
        self.profiles.len() == 1
            && profile.name == "默认配置"
            && profile.base_url.trim().is_empty()
            && profile.api_key.trim().is_empty()
            && !profile.api_key_configured
            && !profile.cc_switch_read_only
            && self.selected_models_by_provider.is_empty()
            && self.manual_third_party_models_by_provider.is_empty()
            && self.declared_official_models_by_provider.is_empty()
            && self.upstream_models_by_provider.is_empty()
            && self.default_model_by_provider.is_empty()
    }

    pub(crate) fn needs_initial_route_import(&self) -> bool {
        !self.initial_route_import_completed && self.looks_like_empty_default_route()
    }

    /// Build one model catalog for all routes registered in the current Codex
    /// process. Third-party entries use local-router aliases so Codex can send
    /// requests through one stable provider while Codey restores upstream ids.
    pub fn runtime_catalog_models(&self) -> (Vec<String>, Vec<String>) {
        let official_models = model_catalog::default_official_model_slugs();
        let include_all_official = self.official_account_available_this_launch
            && self
                .profiles
                .iter()
                .any(|profile| profile.cc_switch_read_only);
        let include_official_in_selected = include_all_official && !self.has_third_party_route();
        let mut upstream = Vec::new();
        let mut selected = Vec::new();
        for profile in &self.profiles {
            if profile.cc_switch_read_only {
                if include_all_official {
                    let enabled = self
                        .selected_models_by_provider
                        .get(profile.provider_id())
                        .filter(|models| !models.is_empty())
                        .cloned()
                        .unwrap_or_else(|| official_models.clone());
                    upstream.extend(enabled.iter().cloned());
                    if include_official_in_selected {
                        selected.extend(enabled);
                    }
                }
                continue;
            }
            let provider_id = profile.provider_id();
            if let Some(models) = self.upstream_models_by_provider.get(profile.provider_id()) {
                upstream.extend(
                    models
                        .iter()
                        .map(|model| local_router::model_alias(provider_id, model)),
                );
            }
            let enabled_models = self.enabled_route_models(provider_id);
            if !enabled_models.is_empty() {
                let aliases = enabled_models
                    .iter()
                    .map(|model| local_router::model_alias(provider_id, model))
                    .collect::<Vec<_>>();
                upstream.extend(aliases.iter().cloned());
                selected.extend(aliases);
            }
        }
        (
            model_id::dedupe_preserving_first(upstream.iter().map(String::as_str)),
            model_id::dedupe_preserving_first(selected.iter().map(String::as_str)),
        )
    }

    pub(crate) fn remember_current_subagent_config(&mut self) {
        let Some(provider_id) = self.current_provider_id().map(ToString::to_string) else {
            return;
        };
        let mut active = self.active_subagent_config();
        active.normalize();
        self.subagent_config_by_provider.insert(provider_id, active);
    }

    pub(crate) fn restore_current_subagent_config(&mut self) {
        let Some(provider_id) = self.current_provider_id().map(ToString::to_string) else {
            return;
        };
        let Some(mut saved) = self.subagent_config_by_provider.get(&provider_id).cloned() else {
            self.remember_current_subagent_config();
            return;
        };
        saved.normalize();
        self.subagent_model = saved.model;
        self.subagent_reasoning_effort = saved.reasoning_effort;
        self.subagent_roles = saved.roles;
    }

    pub(crate) fn remember_current_provider_official_model_support(
        &mut self,
        models: impl IntoIterator<Item = String>,
    ) {
        let Some(provider_id) = self.current_provider_id().map(ToString::to_string) else {
            return;
        };
        self.remember_provider_official_model_support(&provider_id, models);
    }

    fn remember_provider_official_model_support(
        &mut self,
        provider_id: &str,
        models: impl IntoIterator<Item = String>,
    ) {
        if provider_id.trim().is_empty() || self.provider_is_official(provider_id) {
            return;
        }
        let official_models_by_key = official_models_by_key();
        let canonical_models =
            model_id::dedupe_preserving_first(models.into_iter().filter_map(|model| {
                official_models_by_key
                    .get(&model_id::key(&model))
                    .map(String::as_str)
            }));
        if canonical_models.is_empty() {
            return;
        }

        let declared_models = self
            .declared_official_models_by_provider
            .entry(provider_id.to_string())
            .or_default();
        declared_models.extend(canonical_models.iter().cloned());
        normalize_model_list(declared_models);

        let upstream_models = self
            .upstream_models_by_provider
            .entry(provider_id.to_string())
            .or_default();
        upstream_models.extend(canonical_models);
        normalize_model_list(upstream_models);
    }

    fn migrate_custom_subagent_role_model_support(&mut self) {
        let defaults = default_subagent_roles();
        let legacy_uniform_defaults =
            uniform_subagent_roles(DEFAULT_SUBAGENT_MODEL, DEFAULT_SUBAGENT_REASONING_EFFORT);
        let migrations = self
            .subagent_config_by_provider
            .iter()
            .filter_map(|(provider_id, config)| {
                if config.roles == defaults || config.roles == legacy_uniform_defaults {
                    return None;
                }
                let models = config
                    .roles
                    .iter()
                    .filter(|(role, selection)| {
                        defaults
                            .get(*role)
                            .is_none_or(|default| default != *selection)
                    })
                    .map(|(_, selection)| selection.model.clone())
                    .collect::<Vec<_>>();
                (!models.is_empty()).then(|| (provider_id.clone(), models))
            })
            .collect::<Vec<_>>();
        for (provider_id, models) in migrations {
            self.remember_provider_official_model_support(&provider_id, models);
        }
    }

    fn provider_is_official(&self, provider_id: &str) -> bool {
        self.profiles
            .iter()
            .any(|profile| profile.cc_switch_read_only && profile.provider_id() == provider_id)
    }

    fn active_subagent_config(&self) -> SubagentProviderConfig {
        SubagentProviderConfig {
            model: self.subagent_model.clone(),
            reasoning_effort: self.subagent_reasoning_effort.clone(),
            roles: self.subagent_roles.clone(),
        }
    }
}

pub(crate) fn validate_provider_profiles(profiles: &[ProviderProfile]) -> Result<(), String> {
    if profiles.is_empty() {
        return Err("至少需要保留一条线路".to_string());
    }
    let mut profile_ids = BTreeSet::new();
    let mut provider_ids = BTreeSet::new();
    let allows_empty_default = profiles.len() == 1 && profiles[0].is_unconfigured_default();
    for profile in profiles {
        if !allows_empty_default {
            profile.validate()?;
        }
        if !profile_ids.insert(profile.id.clone()) {
            return Err(format!("线路 ID 重复：{}", profile.id));
        }
        let provider_id = profile.provider_id().trim();
        if provider_id.is_empty() {
            return Err(format!("线路「{}」缺少 Codex Provider ID", profile.name));
        }
        if !provider_ids.insert(provider_id.to_string()) {
            return Err(format!(
                "多条线路使用了相同的 Codex Provider ID：{provider_id}"
            ));
        }
    }
    Ok(())
}

fn normalize_model_lists(lists: &mut BTreeMap<String, Vec<String>>) {
    lists.retain(|provider_id, models| {
        normalize_model_list(models);
        !provider_id.trim().is_empty() && !models.is_empty()
    });
}

fn normalize_upstream_model_lists(lists: &mut BTreeMap<String, Vec<String>>) {
    lists.retain(|provider_id, models| {
        normalize_model_list(models);
        !provider_id.trim().is_empty()
    });
}

fn normalize_model_list(models: &mut Vec<String>) {
    *models = model_id::dedupe_preserving_first(models.iter().map(String::as_str));
}

fn official_models_by_key() -> BTreeMap<String, String> {
    model_catalog::default_official_model_slugs()
        .into_iter()
        .map(|model| (model_id::key(&model), model))
        .collect()
}

fn migrate_legacy_official_model_selections(
    selected_models_by_provider: &mut BTreeMap<String, Vec<String>>,
    manual_third_party_models_by_provider: &mut BTreeMap<String, Vec<String>>,
    declared_official_models_by_provider: &mut BTreeMap<String, Vec<String>>,
    official_provider_ids: &BTreeSet<String>,
) {
    let official_models_by_key = official_models_by_key();
    let provider_ids = selected_models_by_provider
        .keys()
        .chain(manual_third_party_models_by_provider.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    for provider_id in provider_ids {
        if official_provider_ids.contains(&provider_id) {
            continue;
        }
        let mut migrated_models = Vec::new();
        if let Some(models) = selected_models_by_provider.get_mut(&provider_id) {
            take_official_models(models, &official_models_by_key, &mut migrated_models);
        }
        if let Some(models) = manual_third_party_models_by_provider.get_mut(&provider_id) {
            take_official_models(models, &official_models_by_key, &mut migrated_models);
        }
        if migrated_models.is_empty() {
            continue;
        }

        let declared_models = declared_official_models_by_provider
            .entry(provider_id)
            .or_default();
        declared_models.extend(migrated_models);
        normalize_model_list(declared_models);
    }

    selected_models_by_provider.retain(|_, models| !models.is_empty());
    manual_third_party_models_by_provider.retain(|_, models| !models.is_empty());
}

fn merge_declared_official_models_into_upstream(
    declared_official_models_by_provider: &BTreeMap<String, Vec<String>>,
    upstream_models_by_provider: &mut BTreeMap<String, Vec<String>>,
) {
    let official_models_by_key = official_models_by_key();
    for (provider_id, declared_models) in declared_official_models_by_provider {
        let upstream_models = upstream_models_by_provider
            .entry(provider_id.clone())
            .or_default();
        upstream_models.extend(
            declared_models
                .iter()
                .filter_map(|model| official_models_by_key.get(&model_id::key(model)).cloned()),
        );
        normalize_model_list(upstream_models);
    }
}

fn take_official_models(
    models: &mut Vec<String>,
    official_models_by_key: &BTreeMap<String, String>,
    migrated_models: &mut Vec<String>,
) {
    models.retain(|model| {
        let Some(canonical_model) = official_models_by_key.get(&model_id::key(model)) else {
            return true;
        };
        migrated_models.push(canonical_model.clone());
        false
    });
}

fn normalize_model_map(models_by_provider: &mut BTreeMap<String, String>) {
    models_by_provider.retain(|provider_id, model| {
        *model = model.trim().to_string();
        !provider_id.trim().is_empty() && !model.is_empty()
    });
}

fn default_true() -> bool {
    true
}

pub const DEFAULT_SUBAGENT_MODEL: &str = "gpt-5.6-terra";
pub const DEFAULT_SUBAGENT_REASONING_EFFORT: &str = "low";
pub const UPSTREAM_PROTOCOL_OFFICIAL: &str = "official";
pub const UPSTREAM_PROTOCOL_OPENAI_RESPONSES: &str = "openaiResponses";
pub const UPSTREAM_PROTOCOL_OPENAI_COMPATIBLE: &str = "openaiCompatible";
pub const AUTH_MODE_OFFICIAL_ACCOUNT: &str = "officialAccount";
pub const AUTH_MODE_API_KEY: &str = "apiKey";
pub const SUBAGENT_REASONING_EFFORTS: [&str; 6] =
    ["low", "medium", "high", "xhigh", "max", "ultra"];
pub const SUBAGENT_ROLE_QUICK_SCAN: &str = "codey_quick_scan";
pub const SUBAGENT_ROLE_DEEP_RESEARCH: &str = "codey_deep_research";
pub const SUBAGENT_ROLE_VISUAL_ANALYSIS: &str = "codey_visual_analysis";
pub const SUBAGENT_ROLE_WORKER: &str = "codey_worker";
pub const SUBAGENT_ROLE_VISUAL_WORKER: &str = "codey_visual_worker";
pub const SUBAGENT_ROLE_DEFAULT: &str = "default";
pub const SUBAGENT_ROLE_IDS: [&str; 6] = [
    SUBAGENT_ROLE_QUICK_SCAN,
    SUBAGENT_ROLE_DEEP_RESEARCH,
    SUBAGENT_ROLE_VISUAL_ANALYSIS,
    SUBAGENT_ROLE_WORKER,
    SUBAGENT_ROLE_VISUAL_WORKER,
    SUBAGENT_ROLE_DEFAULT,
];

pub fn default_subagent_roles() -> BTreeMap<String, SubagentRoleConfig> {
    [
        (SUBAGENT_ROLE_QUICK_SCAN, "low"),
        (SUBAGENT_ROLE_DEEP_RESEARCH, "high"),
        (SUBAGENT_ROLE_VISUAL_ANALYSIS, "high"),
        (SUBAGENT_ROLE_WORKER, "medium"),
        (SUBAGENT_ROLE_VISUAL_WORKER, "high"),
        (SUBAGENT_ROLE_DEFAULT, DEFAULT_SUBAGENT_REASONING_EFFORT),
    ]
    .into_iter()
    .map(|(role, effort)| {
        (
            role.to_string(),
            SubagentRoleConfig::new(DEFAULT_SUBAGENT_MODEL, effort),
        )
    })
    .collect()
}

pub fn uniform_subagent_roles(
    model: &str,
    reasoning_effort: &str,
) -> BTreeMap<String, SubagentRoleConfig> {
    SUBAGENT_ROLE_IDS
        .into_iter()
        .map(|role| {
            (
                role.to_string(),
                SubagentRoleConfig::new(model, reasoning_effort),
            )
        })
        .collect()
}

fn normalize_subagent_selection(model: &mut String, reasoning_effort: &mut String) {
    *model = model.trim().to_string();
    if model.is_empty() {
        *model = default_subagent_model();
    }
    *reasoning_effort = reasoning_effort.trim().to_ascii_lowercase();
    if !SUBAGENT_REASONING_EFFORTS.contains(&reasoning_effort.as_str()) {
        *reasoning_effort = default_subagent_reasoning_effort();
    }
}

fn default_upstream_protocol() -> String {
    UPSTREAM_PROTOCOL_OPENAI_RESPONSES.to_string()
}

fn default_auth_mode() -> String {
    AUTH_MODE_API_KEY.to_string()
}

fn normalize_upstream_protocol(value: &str) -> String {
    match value.trim() {
        UPSTREAM_PROTOCOL_OFFICIAL => UPSTREAM_PROTOCOL_OFFICIAL,
        UPSTREAM_PROTOCOL_OPENAI_COMPATIBLE => UPSTREAM_PROTOCOL_OPENAI_COMPATIBLE,
        _ => UPSTREAM_PROTOCOL_OPENAI_RESPONSES,
    }
    .to_string()
}

fn normalize_auth_mode(value: &str, official: bool) -> String {
    if official || value.trim() == AUTH_MODE_OFFICIAL_ACCOUNT {
        AUTH_MODE_OFFICIAL_ACCOUNT
    } else {
        AUTH_MODE_API_KEY
    }
    .to_string()
}

fn normalize_subagent_config(
    model: &mut String,
    reasoning_effort: &mut String,
    roles: &mut BTreeMap<String, SubagentRoleConfig>,
) {
    normalize_subagent_selection(model, reasoning_effort);
    roles.retain(|role, _| SUBAGENT_ROLE_IDS.contains(&role.as_str()));
    if roles.is_empty() {
        *roles = uniform_subagent_roles(model, reasoning_effort);
    } else {
        let fallback = roles
            .get(SUBAGENT_ROLE_DEFAULT)
            .cloned()
            .unwrap_or_else(|| SubagentRoleConfig::new(model.clone(), reasoning_effort.clone()));
        for role in SUBAGENT_ROLE_IDS {
            roles
                .entry(role.to_string())
                .or_insert_with(|| fallback.clone());
        }
        for selection in roles.values_mut() {
            normalize_subagent_selection(&mut selection.model, &mut selection.reasoning_effort);
        }
    }
    if let Some(default_role) = roles.get(SUBAGENT_ROLE_DEFAULT) {
        model.clone_from(&default_role.model);
        reasoning_effort.clone_from(&default_role.reasoning_effort);
    }
}

fn default_subagent_model() -> String {
    DEFAULT_SUBAGENT_MODEL.to_string()
}

fn default_subagent_reasoning_effort() -> String {
    DEFAULT_SUBAGENT_REASONING_EFFORT.to_string()
}

const DEFAULT_UPDATE_BASE_URL: &str = "https://pub-2d17a6a8bc22426a92e297a59f55ccc3.r2.dev";

fn update_manifest_url_from_base(configured_base_url: Option<&str>) -> String {
    let base_url = configured_base_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(DEFAULT_UPDATE_BASE_URL)
        .trim_end_matches('/');
    format!("{base_url}/latest.json")
}

pub fn default_update_manifest_url() -> String {
    update_manifest_url_from_base(option_env!("CODEY_UPDATE_BASE_URL"))
}

pub fn default_config_path() -> PathBuf {
    ProjectDirs::from("com", "Codey", "Codey")
        .map(|dirs| dirs.config_dir().join("config.json"))
        .unwrap_or_else(|| PathBuf::from(".codey").join("config.json"))
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new(default_config_path())
    }
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<CodeyConfig> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => {
                let raw = serde_json::from_str::<serde_json::Value>(&contents)
                    .with_context(|| format!("解析 Codey 配置失败：{}", self.path.display()))?;
                let has_initial_import_marker = raw
                    .as_object()
                    .is_some_and(|object| object.contains_key("initialRouteImportCompleted"));
                let mut config = serde_json::from_value::<CodeyConfig>(raw)
                    .with_context(|| format!("解析 Codey 配置失败：{}", self.path.display()))?;
                if !has_initial_import_marker && !config.looks_like_empty_default_route() {
                    config.initial_route_import_completed = true;
                }
                Ok(config.normalize())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(CodeyConfig::default())
            }
            Err(error) => {
                Err(error).with_context(|| format!("读取 Codey 配置失败：{}", self.path.display()))
            }
        }
    }

    pub fn save(&self, config: &CodeyConfig) -> Result<()> {
        let config = config.clone().normalize();
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Codey 配置路径无父目录"))?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(&config)?;
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Codey 配置路径缺少文件名"))?
            .to_string_lossy();
        let temp = parent.join(format!(
            ".{file_name}.codey-{}.tmp",
            Uuid::new_v4().simple()
        ));
        let replace_result = write_private_temp(&temp, &bytes).and_then(|()| {
            crate::fs_util::persist_temp_file(&temp, &self.path)
                .with_context(|| format!("替换 Codey 配置失败：{}", self.path.display()))
        });
        if replace_result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        replace_result?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

fn write_private_temp(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("创建 Codey 配置临时文件失败：{}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("写入 Codey 配置临时文件失败：{}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("同步 Codey 配置临时文件失败：{}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_profiles_cannot_shadow_the_internal_router_provider() {
        let mut profile = ProviderProfile::new("Reserved route");
        profile.id = local_router::ROUTER_PROVIDER_ID.to_string();
        profile.base_url = "https://relay.example/v1".into();
        profile.api_key = "sk-test".into();
        profile.normalize();

        assert!(
            profile
                .validate()
                .unwrap_err()
                .contains("Codey 内部 Provider ID")
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_temp_files_are_private_before_atomic_replace() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".config.json.codey-test.tmp");

        write_private_temp(&path, b"private-secret").unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn config_save_does_not_leave_a_plaintext_temp_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json"));

        store.save(&CodeyConfig::default()).unwrap();

        let names = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, [std::ffi::OsString::from("config.json")]);
    }

    #[test]
    fn request_only_provider_headers_are_never_serialized() {
        let mut profile = ProviderProfile::new("Private Relay");
        profile
            .model_request_headers
            .insert("Authorization".to_string(), "secret".to_string());

        let serialized = serde_json::to_value(profile).unwrap();

        assert!(serialized.get("modelRequestHeaders").is_none());
        assert!(!serialized.to_string().contains("secret"));
    }

    #[test]
    fn deprecated_provider_protocol_fields_are_ignored() {
        let profile = serde_json::from_value::<ProviderProfile>(serde_json::json!({
            "id": "legacy-provider",
            "name": "Legacy Provider",
            "baseUrl": "https://gateway.example/v1",
            "apiKey": "",
            "protocol": "chatCompletions",
            "chatCompletionsModels": ["legacy-model"]
        }))
        .unwrap();

        let serialized = serde_json::to_value(profile).unwrap();

        assert!(serialized.get("protocol").is_none());
        assert!(serialized.get("chatCompletionsModels").is_none());
    }

    #[test]
    fn redacted_third_party_route_requires_its_saved_secret_before_validation() {
        let mut saved = ProviderProfile::new("Relay");
        saved.id = "relay".into();
        saved.base_url = "https://relay.example/v1".into();
        saved.api_key = "secret-token".into();
        saved.normalize();

        let mut redacted = saved.clone();
        redacted.api_key.clear();
        redacted.api_key_configured = true;
        assert!(redacted.validate().is_err());

        redacted.merge_redacted_secret(Some(&saved));
        redacted.normalize();
        assert!(redacted.validate().is_ok());
        assert_eq!(redacted.api_key, "secret-token");
    }

    #[test]
    fn route_validation_rejects_duplicate_runtime_provider_ids() {
        let mut first = ProviderProfile::new("First");
        first.id = "first".into();
        first.base_url = "https://first.example/v1".into();
        first.api_key = "first-key".into();
        first.cc_switch_provider_id = Some("shared-provider".into());
        first.normalize();

        let mut second = ProviderProfile::new("Second");
        second.id = "second".into();
        second.base_url = "https://second.example/v1".into();
        second.api_key = "second-key".into();
        second.cc_switch_provider_id = Some("shared-provider".into());
        second.normalize();

        let error = validate_provider_profiles(&[first, second]).unwrap_err();
        assert!(error.contains("shared-provider"));
    }

    #[test]
    fn runtime_catalog_combines_models_from_every_registered_route() {
        let mut official = ProviderProfile::new("Official");
        official.id = "official-profile".into();
        official.cc_switch_provider_id = Some("openai".into());
        official.auth_mode = AUTH_MODE_OFFICIAL_ACCOUNT.into();
        official.normalize();

        let mut relay = ProviderProfile::new("Relay");
        relay.id = "relay".into();
        relay.base_url = "https://relay.example/v1".into();
        relay.api_key = "relay-key".into();
        relay.normalize();

        let mut config = CodeyConfig {
            active_profile_id: official.id.clone(),
            profiles: vec![official, relay],
            official_account_available_this_launch: true,
            ..CodeyConfig::default()
        };
        config.upstream_models_by_provider.insert(
            "relay".into(),
            vec!["relay-a".into(), "shared-model".into()],
        );
        config.selected_models_by_provider.insert(
            "relay".into(),
            vec!["shared-model".into(), "manual-model".into()],
        );

        let (upstream, selected) = config.runtime_catalog_models();

        assert!(upstream.iter().any(|model| model == "gpt-5.6-sol"));
        assert!(upstream.iter().any(|model| model == "relay/relay-a"));
        assert!(upstream.iter().any(|model| model == "relay/manual-model"));
        assert_eq!(
            upstream
                .iter()
                .filter(|model| model.as_str() == "relay/shared-model")
                .count(),
            1
        );
        assert_eq!(selected, ["relay/shared-model", "relay/manual-model"]);
    }

    #[test]
    fn normalizes_missing_active_profile() {
        let config = CodeyConfig {
            active_profile_id: "missing".to_string(),
            ..CodeyConfig::default()
        };
        let normalized = config.normalize();
        assert_eq!(normalized.active_profile_id, normalized.profiles[0].id);
    }

    #[test]
    fn non_empty_legacy_configs_are_marked_as_imported() {
        let mut route = ProviderProfile::new("Relay");
        route.id = "relay".into();
        route.base_url = "https://relay.example/v1".into();
        route.api_key = "sk-relay".into();
        route.normalize();
        let config = CodeyConfig {
            active_profile_id: route.id.clone(),
            profiles: vec![route],
            initial_route_import_completed: false,
            ..CodeyConfig::default()
        }
        .normalize();

        assert!(config.initial_route_import_completed);
    }

    #[test]
    fn launch_official_profile_is_first_without_stealing_active_third_party_route() {
        let mut relay = ProviderProfile::new("Relay");
        relay.id = "relay".into();
        relay.base_url = "https://relay.example/v1".into();
        relay.api_key = "sk-relay".into();
        relay.normalize();
        let mut official = ProviderProfile::new("OpenAI 官方直登");
        official.id = "openai-source".into();
        official.cc_switch_provider_id = Some("openai".into());
        official.auth_mode = AUTH_MODE_OFFICIAL_ACCOUNT.into();
        official.normalize();
        let mut config = CodeyConfig {
            active_profile_id: relay.id.clone(),
            profiles: vec![relay],
            initial_route_import_completed: true,
            ..CodeyConfig::default()
        };
        config
            .default_model_by_provider
            .insert("openai".into(), "gpt-5.6-sol".into());

        config.apply_launch_official_profile(Some(official));
        config = config.normalize();

        assert_eq!(config.profiles[0].id, DERIVED_OFFICIAL_PROFILE_ID);
        assert_eq!(config.active_profile_id, "relay");
        assert_eq!(config.profiles[0].provider_id(), "openai");
        assert_eq!(config.default_model_by_provider["openai"], "gpt-5.6-sol");
        assert_eq!(
            config.selected_models_by_provider["openai"],
            model_catalog::default_official_model_slugs(),
        );

        config
            .selected_models_by_provider
            .insert("openai".into(), vec!["gpt-5.6-sol".into()]);
        config = config.normalize();
        assert_eq!(
            config.selected_models_by_provider["openai"],
            ["gpt-5.6-sol"],
        );
    }

    #[test]
    fn api_key_launch_removes_derived_official_route_and_falls_back_to_saved_route() {
        let mut official = ProviderProfile::new("OpenAI 官方直登");
        official.id = DERIVED_OFFICIAL_PROFILE_ID.into();
        official.cc_switch_provider_id = Some("openai".into());
        official.auth_mode = AUTH_MODE_OFFICIAL_ACCOUNT.into();
        official.normalize();
        let mut relay = ProviderProfile::new("Relay");
        relay.id = "relay".into();
        relay.base_url = "https://relay.example/v1".into();
        relay.api_key = "sk-relay".into();
        relay.normalize();
        let mut config = CodeyConfig {
            active_profile_id: official.id.clone(),
            profiles: vec![official, relay],
            initial_route_import_completed: true,
            ..CodeyConfig::default()
        };
        config
            .default_model_by_provider
            .insert("openai".into(), "gpt-5.6-sol".into());

        config.apply_launch_official_profile(None);
        config = config.normalize();

        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].id, "relay");
        assert_eq!(config.active_profile_id, "relay");
        assert_eq!(config.default_model_by_provider["openai"], "gpt-5.6-sol");
    }

    #[test]
    fn preserves_an_empty_upstream_snapshot_as_a_successful_sync() {
        let mut config = CodeyConfig::default();
        let provider_id = config.current_provider_id().unwrap().to_string();
        config
            .upstream_models_by_provider
            .insert(provider_id, Vec::new());

        let normalized = config.normalize();

        assert_eq!(normalized.upstream_models_snapshot(), Some([].as_slice()));
    }

    #[test]
    fn model_lists_trim_and_dedupe_case_insensitively() {
        let mut config = CodeyConfig::default();
        let provider_id = config.current_provider_id().unwrap().to_string();
        config.selected_models_by_provider.insert(
            provider_id.clone(),
            vec![
                " Provider-A ".to_string(),
                "provider-a".to_string(),
                "Provider-B".to_string(),
            ],
        );
        config.upstream_models_by_provider.insert(
            provider_id.clone(),
            vec!["UPSTREAM-A".to_string(), "upstream-a".to_string()],
        );
        config.declared_official_models_by_provider.insert(
            provider_id.clone(),
            vec![" GPT-5.6-SOL ".to_string(), "gpt-5.6-sol".to_string()],
        );

        let normalized = config.normalize();

        assert_eq!(
            normalized.selected_models_by_provider[&provider_id],
            ["Provider-A", "Provider-B"]
        );
        assert_eq!(
            normalized.upstream_models_by_provider[&provider_id],
            ["UPSTREAM-A", "gpt-5.6-sol"]
        );
        assert_eq!(
            normalized.declared_official_models_by_provider[&provider_id],
            ["GPT-5.6-SOL"]
        );
    }

    #[test]
    fn legacy_official_models_are_reclassified_and_survive_persistence() {
        let mut config = CodeyConfig::default();
        let provider_id = config.current_provider_id().unwrap().to_string();
        config.selected_models_by_provider.insert(
            provider_id.clone(),
            vec![
                "GPT-5.6-Luna".into(),
                "provider-custom".into(),
                "gpt-5.6-sol".into(),
            ],
        );
        config.manual_third_party_models_by_provider.insert(
            provider_id.clone(),
            vec![
                "GPT-5.6-Terra".into(),
                "provider-custom".into(),
                "manual-only".into(),
            ],
        );
        config
            .declared_official_models_by_provider
            .insert(provider_id.clone(), vec!["GPT-5.6-SOL".into()]);
        config
            .upstream_models_by_provider
            .insert(provider_id.clone(), Vec::new());

        let normalized = config.normalize();

        assert_eq!(
            normalized.selected_models_by_provider[&provider_id],
            ["provider-custom"]
        );
        assert_eq!(
            normalized.manual_third_party_models_by_provider[&provider_id],
            ["provider-custom", "manual-only"]
        );
        assert_eq!(
            normalized.declared_official_models_by_provider[&provider_id],
            ["GPT-5.6-SOL", "gpt-5.6-luna", "gpt-5.6-terra"]
        );
        assert_eq!(
            normalized.upstream_models_by_provider[&provider_id],
            ["gpt-5.6-sol", "gpt-5.6-luna", "gpt-5.6-terra"]
        );

        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json"));
        store.save(&normalized).unwrap();
        let reloaded = store.load().unwrap();
        assert_eq!(
            reloaded.declared_official_models_by_provider[&provider_id],
            ["GPT-5.6-SOL", "gpt-5.6-luna", "gpt-5.6-terra"]
        );
        assert_eq!(
            reloaded.upstream_models_by_provider[&provider_id],
            ["gpt-5.6-sol", "gpt-5.6-luna", "gpt-5.6-terra"]
        );
    }

    #[test]
    fn diagnostic_guards_can_be_disabled_explicitly() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"disableTraceLogWrites":false,"protectCrashpadPending":false}"#,
        )
        .unwrap()
        .normalize();
        let serialized = serde_json::to_value(&config).unwrap();

        assert!(!config.disable_trace_log_writes);
        assert!(!config.protect_crashpad_pending);
        assert_eq!(
            serialized.get("disableTraceLogWrites"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            serialized.get("protectCrashpadPending"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn legacy_webhook_is_migrated_to_a_feishu_channel_without_the_old_secret() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"webhook":{"enabled":true,"url":"https://open.feishu.cn/example","secret":"legacy-sign-key"}}"#,
        )
        .unwrap()
        .normalize();
        let serialized = serde_json::to_value(&config).unwrap();

        assert!(!config.webhook.enabled);
        assert!(config.webhook.url.is_empty());
        assert_eq!(config.webhook.channels.len(), 1);
        let channel = &config.webhook.channels[0];
        assert_eq!(channel.id, "legacy-feishu");
        assert_eq!(
            channel.kind,
            crate::notifications::NotificationChannelKind::Feishu
        );
        assert!(channel.enabled);
        assert_eq!(channel.url, "https://open.feishu.cn/example");
        assert!(serialized["webhook"].get("enabled").is_none());
        assert!(serialized["webhook"].get("url").is_none());
        assert!(serialized["webhook"].get("secret").is_none());
    }

    #[test]
    fn trace_log_guard_defaults_to_enabled_for_existing_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(config.disable_trace_log_writes);
        assert!(config.protect_crashpad_pending);
    }

    #[test]
    fn user_update_manifest_url_is_ignored_and_not_persisted() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"updateManifestUrl":"https://example.com/latest.json"}"#,
        )
        .unwrap()
        .normalize();
        let serialized = serde_json::to_value(&config).unwrap();

        assert_eq!(config.update_manifest_url, default_update_manifest_url());
        assert!(serialized.get("updateManifestUrl").is_none());
    }

    #[test]
    fn update_manifest_url_defaults_to_the_public_source_for_local_builds() {
        let expected = format!("{DEFAULT_UPDATE_BASE_URL}/latest.json");

        assert_eq!(update_manifest_url_from_base(None), expected);
        assert_eq!(update_manifest_url_from_base(Some("  ")), expected);
        assert_eq!(
            update_manifest_url_from_base(Some("https://updates.example.com/codey/")),
            "https://updates.example.com/codey/latest.json"
        );
    }

    #[test]
    fn pet_slim_mode_defaults_to_enabled_for_existing_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(config.slim_codex_pet);
    }

    #[test]
    fn pet_slim_mode_can_be_disabled_explicitly() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"slimCodexPet":false}"#,
        )
        .unwrap()
        .normalize();

        assert!(!config.slim_codex_pet);
    }

    #[test]
    fn gpu_launch_mode_defaults_to_off() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();
        let serialized = serde_json::to_value(&config).unwrap();

        assert_eq!(config.gpu_launch_mode, GpuLaunchMode::Off);
        assert_eq!(serialized["gpuLaunchMode"], "off");
    }

    #[test]
    fn gpu_launch_modes_round_trip_as_mutually_exclusive_values() {
        for (wire_value, expected) in [
            ("off", GpuLaunchMode::Off),
            ("disableGpu", GpuLaunchMode::DisableGpu),
            (
                "disableGpuRasterization",
                GpuLaunchMode::DisableGpuRasterization,
            ),
        ] {
            let config = serde_json::from_value::<CodeyConfig>(serde_json::json!({
                "activeProfileId": "",
                "profiles": [],
                "gpuLaunchMode": wire_value,
            }))
            .unwrap()
            .normalize();

            assert_eq!(config.gpu_launch_mode, expected);
            assert_eq!(
                serde_json::to_value(&config).unwrap()["gpuLaunchMode"],
                wire_value
            );
        }
    }

    #[test]
    fn fast_context_tools_default_to_disabled_for_existing_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(!config.fast_context_tools);
    }

    #[test]
    fn retired_fast_startup_setting_is_ignored_and_removed_on_serialize() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"fastCodexStartup":true}"#,
        )
        .unwrap()
        .normalize();

        let serialized = serde_json::to_value(config).unwrap();
        assert!(serialized.get("fastCodexStartup").is_none());
    }

    #[test]
    fn subagent_optimization_defaults_to_disabled_for_existing_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(!config.subagent_optimization);
        assert_eq!(config.subagent_model, DEFAULT_SUBAGENT_MODEL);
        assert_eq!(
            config.subagent_reasoning_effort,
            DEFAULT_SUBAGENT_REASONING_EFFORT
        );
        assert_eq!(config.subagent_roles.len(), SUBAGENT_ROLE_IDS.len());
        assert!(
            config
                .subagent_roles
                .values()
                .all(|selection| selection.model == DEFAULT_SUBAGENT_MODEL)
        );
    }

    #[test]
    fn fresh_subagent_defaults_keep_the_original_role_preset() {
        let config = CodeyConfig::default();

        assert!(
            config
                .subagent_roles
                .values()
                .all(|selection| selection.model == DEFAULT_SUBAGENT_MODEL)
        );
        assert_eq!(
            config.subagent_roles[SUBAGENT_ROLE_WORKER].reasoning_effort,
            "medium"
        );
        assert_eq!(
            config.subagent_roles[SUBAGENT_ROLE_VISUAL_WORKER].reasoning_effort,
            "high"
        );
    }

    #[test]
    fn existing_custom_role_models_are_migrated_once_without_changing_defaults() {
        let mut profile = ProviderProfile::new("Third party");
        profile.id = "third-party".into();
        let mut roles = default_subagent_roles();
        roles.get_mut(SUBAGENT_ROLE_QUICK_SCAN).unwrap().model = "gpt-5.6-luna".into();
        roles.get_mut(SUBAGENT_ROLE_DEEP_RESEARCH).unwrap().model = "gpt-5.6-luna".into();
        roles
            .get_mut(SUBAGENT_ROLE_WORKER)
            .unwrap()
            .reasoning_effort = "max".into();
        roles
            .get_mut(SUBAGENT_ROLE_VISUAL_WORKER)
            .unwrap()
            .reasoning_effort = "max".into();
        let mut config = CodeyConfig {
            active_profile_id: profile.id.clone(),
            profiles: vec![profile],
            subagent_optimization: true,
            subagent_roles: roles.clone(),
            subagent_role_model_support_migrated: false,
            ..CodeyConfig::default()
        };
        config
            .upstream_models_by_provider
            .insert("third-party".into(), vec!["provider-custom-model".into()]);

        let normalized = config.normalize();

        assert_eq!(normalized.subagent_roles, roles);
        assert!(normalized.subagent_role_model_support_migrated);
        assert_eq!(
            normalized.declared_official_models_by_provider["third-party"],
            ["gpt-5.6-luna", "gpt-5.6-terra"]
        );
        assert_eq!(
            normalized.upstream_models_by_provider["third-party"],
            ["provider-custom-model", "gpt-5.6-luna", "gpt-5.6-terra"]
        );
        assert_eq!(
            default_subagent_roles()[SUBAGENT_ROLE_QUICK_SCAN],
            SubagentRoleConfig::new(DEFAULT_SUBAGENT_MODEL, "low")
        );

        let mut after_explicit_removal = normalized;
        after_explicit_removal
            .declared_official_models_by_provider
            .insert("third-party".into(), vec!["gpt-5.6-terra".into()]);
        after_explicit_removal.upstream_models_by_provider.insert(
            "third-party".into(),
            vec!["provider-custom-model".into(), "gpt-5.6-terra".into()],
        );
        let after_explicit_removal = after_explicit_removal.normalize();

        assert_eq!(
            after_explicit_removal.declared_official_models_by_provider["third-party"],
            ["gpt-5.6-terra"]
        );
        assert!(
            !after_explicit_removal.upstream_models_by_provider["third-party"]
                .iter()
                .any(|model| model_id::equal(model, "gpt-5.6-luna"))
        );
    }

    #[test]
    fn custom_roles_do_not_declare_models_for_an_official_provider() {
        let mut profile = ProviderProfile::new("Official");
        profile.id = "openai".into();
        profile.cc_switch_read_only = true;
        let mut roles = default_subagent_roles();
        roles.get_mut(SUBAGENT_ROLE_QUICK_SCAN).unwrap().model = "gpt-5.6-luna".into();
        let config = CodeyConfig {
            active_profile_id: profile.id.clone(),
            profiles: vec![profile],
            subagent_roles: roles.clone(),
            subagent_role_model_support_migrated: false,
            ..CodeyConfig::default()
        }
        .normalize();

        assert_eq!(config.subagent_roles, roles);
        assert!(config.declared_official_models_by_provider.is_empty());
        assert!(config.upstream_models_by_provider.is_empty());
    }

    #[test]
    fn subagent_defaults_preserve_models_and_invalid_effort_falls_back() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"subagentModel":"  provider-coder  ","subagentReasoningEffort":"unsupported"}"#,
        )
        .unwrap()
        .normalize();

        assert_eq!(config.subagent_model, "provider-coder");
        assert_eq!(
            config.subagent_reasoning_effort,
            DEFAULT_SUBAGENT_REASONING_EFFORT
        );
        assert!(config.subagent_roles.values().all(|selection| {
            selection.model == "provider-coder"
                && selection.reasoning_effort == DEFAULT_SUBAGENT_REASONING_EFFORT
        }));

        let empty = serde_json::from_str::<CodeyConfig>(
            r#"{"activeProfileId":"","profiles":[],"subagentModel":"   ","subagentReasoningEffort":"high"}"#,
        )
        .unwrap()
        .normalize();

        assert_eq!(empty.subagent_model, DEFAULT_SUBAGENT_MODEL);
        assert_eq!(empty.subagent_reasoning_effort, "high");
    }

    #[test]
    fn subagent_role_map_normalizes_independently_and_syncs_the_legacy_fallback() {
        let config = serde_json::from_str::<CodeyConfig>(
            r#"{
                "activeProfileId":"",
                "profiles":[],
                "subagentModel":"legacy-model",
                "subagentReasoningEffort":"low",
                "subagentRoles":{
                    "codey_quick_scan":{"model":" quick-model ","reasoningEffort":"MEDIUM"},
                    "default":{"model":" fallback-model ","reasoningEffort":"high"},
                    "unknown":{"model":"ignored","reasoningEffort":"low"}
                }
            }"#,
        )
        .unwrap()
        .normalize();

        assert_eq!(config.subagent_roles.len(), SUBAGENT_ROLE_IDS.len());
        assert!(!config.subagent_roles.contains_key("unknown"));
        assert_eq!(
            config.subagent_roles[SUBAGENT_ROLE_QUICK_SCAN],
            SubagentRoleConfig::new("quick-model", "medium")
        );
        assert_eq!(config.subagent_model, "fallback-model");
        assert_eq!(config.subagent_reasoning_effort, "high");
        assert_eq!(
            config.subagent_roles[SUBAGENT_ROLE_WORKER],
            SubagentRoleConfig::new("fallback-model", "high")
        );
    }

    #[test]
    fn subagent_config_is_remembered_per_provider_and_new_providers_inherit() {
        let mut provider_a = ProviderProfile::new("A");
        provider_a.id = "provider-a".into();
        let mut provider_b = ProviderProfile::new("B");
        provider_b.id = "provider-b".into();
        let mut config = CodeyConfig {
            active_profile_id: provider_a.id.clone(),
            profiles: vec![provider_a, provider_b],
            subagent_model: "model-a".into(),
            subagent_reasoning_effort: "high".into(),
            subagent_roles: uniform_subagent_roles("model-a", "high"),
            ..CodeyConfig::default()
        }
        .normalize();

        assert_eq!(
            config.subagent_config_by_provider["provider-a"].model,
            "model-a"
        );

        config.active_profile_id = "provider-b".into();
        config.restore_current_subagent_config();
        assert_eq!(config.subagent_model, "model-a");
        assert_eq!(
            config.subagent_config_by_provider["provider-b"].model,
            "model-a"
        );

        config.subagent_model = "model-b".into();
        config.subagent_reasoning_effort = "medium".into();
        config.subagent_roles = uniform_subagent_roles("model-b", "medium");
        config.remember_current_subagent_config();

        config.active_profile_id = "provider-a".into();
        config.restore_current_subagent_config();
        assert_eq!(config.subagent_model, "model-a");
        assert_eq!(config.subagent_reasoning_effort, "high");

        config.active_profile_id = "provider-b".into();
        config.restore_current_subagent_config();
        assert_eq!(config.subagent_model, "model-b");
        assert_eq!(config.subagent_reasoning_effort, "medium");
    }

    #[test]
    fn full_access_warning_shield_defaults_to_disabled_for_existing_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(!config.hide_full_access_warning);
    }

    #[test]
    fn header_account_usage_defaults_to_enabled_for_supported_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(config.show_account_usage_in_header);
    }

    #[test]
    fn prompt_optimization_defaults_to_disabled_for_existing_configs() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[]}"#)
            .unwrap()
            .normalize();

        assert!(!config.prompt_optimization.enabled);
        assert!(config.prompt_optimization.api_key.is_empty());
    }

    #[test]
    fn prompt_optimization_round_trips_without_persisting_clear_flag() {
        let config = serde_json::from_str::<CodeyConfig>(r#"{"activeProfileId":"","profiles":[],"promptOptimization":{"enabled":true,"baseUrl":" https://api.example.com/v1/ ","apiKey":"sk-secret","model":" gpt-x ","protocol":"responses","instruction":" 保持简洁 "}}"#)
            .unwrap()
            .normalize();
        let serialized = serde_json::to_value(&config).unwrap();

        assert!(config.prompt_optimization.enabled);
        assert_eq!(
            config.prompt_optimization.base_url,
            "https://api.example.com/v1"
        );
        assert_eq!(config.prompt_optimization.api_key, "sk-secret");
        assert!(config.prompt_optimization.api_key_configured);
        assert_eq!(config.prompt_optimization.model, "gpt-x");
        assert_eq!(config.prompt_optimization.instruction, "保持简洁");
        assert!(serialized["promptOptimization"].get("protocol").is_none());
        assert!(
            serialized["promptOptimization"]
                .get("clearApiKey")
                .is_none()
        );
    }

    #[test]
    fn redacted_prompt_optimization_key_is_restored_when_other_settings_are_saved() {
        let previous = CodeyConfig {
            prompt_optimization: PromptOptimizationConfig {
                enabled: true,
                base_url: "https://api.example.com/v1".to_string(),
                api_key: "sk-secret".to_string(),
                api_key_configured: true,
                model: "gpt-x".to_string(),
                ..PromptOptimizationConfig::default()
            },
            ..CodeyConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.prompt_optimization.api_key.clear();
        incoming
            .prompt_optimization
            .merge_redacted_secrets(&previous.prompt_optimization);

        assert_eq!(incoming.prompt_optimization.api_key, "sk-secret");
    }

    #[test]
    fn explicit_prompt_optimization_key_clear_does_not_restore_the_previous_secret() {
        let previous = CodeyConfig {
            prompt_optimization: PromptOptimizationConfig {
                api_key: "sk-secret".to_string(),
                api_key_configured: true,
                ..PromptOptimizationConfig::default()
            },
            ..CodeyConfig::default()
        };
        let mut incoming = previous.clone();
        incoming.prompt_optimization.api_key.clear();
        incoming.prompt_optimization.clear_api_key = true;
        incoming
            .prompt_optimization
            .merge_redacted_secrets(&previous.prompt_optimization);

        assert!(incoming.prompt_optimization.api_key.is_empty());
        assert!(!incoming.prompt_optimization.api_key_configured);
    }
}
