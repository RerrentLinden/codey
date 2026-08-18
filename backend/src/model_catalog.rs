use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::fs_util::atomic_write_private_with_parent as atomic_write;
use crate::model_id;

const MODEL_CATALOG_RELATIVE_PATH: &str = "model-catalogs/codey-official.json";
pub(crate) const THIRD_PARTY_REASONING_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
pub(crate) const THIRD_PARTY_DEFAULT_REASONING_EFFORT: &str = "low";
const REASONING_LEVEL_DESCRIPTIONS: [(&str, &str); 4] = [
    ("low", "Fast responses with lighter reasoning"),
    (
        "medium",
        "Balances speed and reasoning depth for everyday tasks",
    ),
    ("high", "Greater reasoning depth for complex problems"),
    ("xhigh", "Extra high reasoning depth for complex problems"),
];
const FAST_SERVICE_TIER_ID: &str = "priority";
const FAST_SPEED_TIER_ID: &str = "fast";
const PERSONALITY_PLACEHOLDER: &str = "{{ personality }}";
const OFFICIAL_MODELS: [(&str, &str); 7] = [
    ("gpt-5.6-sol", "GPT-5.6-Sol"),
    ("gpt-5.6-terra", "GPT-5.6-Terra"),
    ("gpt-5.6-luna", "GPT-5.6-Luna"),
    ("gpt-5.5", "GPT-5.5"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.4-mini", "GPT-5.4-Mini"),
    ("gpt-5.3-codex-spark", "GPT-5.3-Codex-Spark"),
];

#[derive(Debug)]
struct RuntimeModelCacheUnavailable;

impl fmt::Display for RuntimeModelCacheUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "本机 Codex 模型缓存缺少运行时必需字段；请先直接启动官方 Codex 完成模型缓存刷新",
        )
    }
}

impl std::error::Error for RuntimeModelCacheUnavailable {}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialModel {
    pub slug: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialModelAvailability {
    pub slug: String,
    pub display_name: String,
    pub supported: bool,
    pub supported_reasoning_efforts: Vec<String>,
    pub default_reasoning_effort: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionState {
    pub official_models: Vec<OfficialModelAvailability>,
    pub official_model_ids: Vec<String>,
    pub third_party_models: Vec<String>,
    pub manual_third_party_models: Vec<String>,
    pub upstream_models: Vec<String>,
    pub default_model: String,
}

impl ModelSelectionState {
    pub fn available_model(&self, requested: &str) -> Option<&str> {
        let requested = requested.trim();
        if requested.is_empty() {
            return None;
        }
        self.official_models
            .iter()
            .find(|model| model.supported && model.slug.eq_ignore_ascii_case(requested))
            .map(|model| model.slug.as_str())
            .or_else(|| {
                self.third_party_models
                    .iter()
                    .find(|model| model.eq_ignore_ascii_case(requested))
                    .map(String::as_str)
            })
    }

    pub fn first_available_model(&self) -> Option<&str> {
        self.official_models
            .iter()
            .find(|model| model.supported)
            .map(|model| model.slug.as_str())
            .or_else(|| self.third_party_models.first().map(String::as_str))
    }
}

pub fn relative_path() -> &'static str {
    MODEL_CATALOG_RELATIVE_PATH
}

#[derive(Debug)]
pub(crate) struct CatalogSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

pub(crate) fn snapshot(home: &Path) -> Result<CatalogSnapshot> {
    let path = home.join(relative_path());
    let contents = match fs::read(&path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取现有 Codey 模型目录失败：{}", path.display()));
        }
    };
    Ok(CatalogSnapshot { path, contents })
}

pub(crate) fn restore_snapshot(snapshot: CatalogSnapshot) -> Result<()> {
    match snapshot.contents {
        Some(contents) => atomic_write(&snapshot.path, &contents),
        None => match fs::remove_file(&snapshot.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!("移除新建的 Codey 模型目录失败：{}", snapshot.path.display())
            }),
        },
    }
}

pub fn default_official_model_slugs() -> Vec<String> {
    OFFICIAL_MODELS
        .iter()
        .map(|(slug, _)| (*slug).to_string())
        .collect()
}

pub fn refresh_for_provider(
    home: &Path,
    official_provider: bool,
    upstream_models: Option<&[String]>,
    selected_models: &[String],
) -> Result<usize> {
    let official_models = read_official_entries(home)?;
    ensure_runtime_compatible_models(&official_models)?;
    let official_slugs = official_models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(model_id::key)
        .collect::<HashSet<_>>();
    let provider_models_synced = official_provider || upstream_models.is_some();
    let upstream = upstream_models
        .unwrap_or_default()
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let mut catalog_models = official_models
        .iter()
        .filter(|model| {
            official_provider
                || !provider_models_synced
                || model
                    .get("slug")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| upstream.contains(&model_id::key(slug)))
        })
        .cloned()
        .collect::<Vec<_>>();

    for model in &mut catalog_models {
        let declares_fast_support = declares_fast_speed_support(model);
        ensure_catalog_compatibility(model);
        expose_supported_model(model);
        if declares_fast_support {
            add_fast_speed_controls(model);
        }
    }

    if !official_provider {
        let template = official_models
            .iter()
            .find(|model| model.get("visibility").and_then(Value::as_str) == Some("list"))
            .or_else(|| official_models.first())
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("官方账号模型缓存为空，请先使用官方账号启动一次 Codex")
            })?;
        let mut seen = HashSet::new();
        for (index, model_id) in selected_models.iter().enumerate() {
            let model_id = model_id.trim();
            let model_key = model_id::key(model_id);
            if model_id.is_empty()
                || official_slugs.contains(&model_key)
                || (provider_models_synced && !upstream.contains(&model_key))
                || !seen.insert(model_key)
            {
                continue;
            }
            catalog_models.push(synthetic_model(&template, model_id, index));
        }
    }
    write_catalog(home, &catalog_models)?;
    let written_models = read_runtime_catalog_models(home)?;
    let expected_model_keys = catalog_models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(model_id::key)
        .collect::<Vec<_>>();
    let written_model_keys = written_models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(model_id::key)
        .collect::<Vec<_>>();
    if written_model_keys != expected_model_keys {
        bail!("写入后的 Codey 模型目录与本次生成结果不一致");
    }
    Ok(catalog_models.len())
}

#[cfg(test)]
pub fn selection_state(
    home: &Path,
    official_provider: bool,
    upstream_models: Option<&[String]>,
    selected_models: &[String],
    requested_default_model: Option<&str>,
) -> Result<ModelSelectionState> {
    selection_state_with_manual_models(
        home,
        official_provider,
        upstream_models,
        selected_models,
        &[],
        requested_default_model,
    )
}

pub fn selection_state_with_manual_models(
    home: &Path,
    official_provider: bool,
    upstream_models: Option<&[String]>,
    selected_models: &[String],
    manual_third_party_models: &[String],
    requested_default_model: Option<&str>,
) -> Result<ModelSelectionState> {
    let official_entries = read_official_entries(home)?;
    let official_model_ids = official_entries
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let official_slugs = official_model_ids
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let provider_models_synced = official_provider || upstream_models.is_some();
    let upstream_models = upstream_models.unwrap_or_default();
    let upstream = upstream_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let official_models: Vec<OfficialModelAvailability> = official_entries
        .iter()
        .filter_map(|model| {
            let supported_reasoning_efforts = reasoning_efforts_from_value(model);
            let default_reasoning_effort =
                default_reasoning_effort_from_value(model, &supported_reasoning_efforts);
            let model = official_model_from_value(model)?;
            let supported = official_provider
                || !provider_models_synced
                || upstream.contains(&model_id::key(&model.slug));
            Some(OfficialModelAvailability {
                slug: model.slug,
                display_name: model.display_name,
                supported,
                supported_reasoning_efforts,
                default_reasoning_effort,
            })
        })
        .collect();
    let third_party_models = if official_provider {
        Vec::new()
    } else {
        let mut seen = HashSet::new();
        selected_models
            .iter()
            .filter_map(|model| {
                let model = model.trim();
                let key = model_id::key(model);
                if key.is_empty()
                    || official_slugs.contains(&key)
                    || (provider_models_synced && !upstream.contains(&key))
                    || !seen.insert(key)
                {
                    return None;
                }
                Some(model.to_string())
            })
            .collect()
    };
    let manual_model_keys = manual_third_party_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let manual_third_party_models = if official_provider {
        Vec::new()
    } else {
        third_party_models
            .iter()
            .filter(|model| manual_model_keys.contains(&model_id::key(model)))
            .cloned()
            .collect()
    };
    let default_model = effective_default_model(
        &official_models,
        &third_party_models,
        requested_default_model,
    );
    Ok(ModelSelectionState {
        official_models,
        official_model_ids,
        third_party_models,
        manual_third_party_models,
        upstream_models: if official_provider {
            Vec::new()
        } else {
            upstream_models.to_vec()
        },
        default_model,
    })
}

fn effective_default_model(
    official_models: &[OfficialModelAvailability],
    third_party_models: &[String],
    requested_default_model: Option<&str>,
) -> String {
    let requested = requested_default_model
        .map(str::trim)
        .filter(|model| !model.is_empty());
    if let Some(requested) = requested {
        if let Some(model) = official_models
            .iter()
            .find(|model| model.supported && model_id::equal(&model.slug, requested))
        {
            return model.slug.clone();
        }
        if let Some(model) = third_party_models
            .iter()
            .find(|model| model_id::equal(model, requested))
        {
            return model.clone();
        }
    }
    official_models
        .iter()
        .find(|model| model.supported)
        .map(|model| model.slug.clone())
        .or_else(|| third_party_models.first().cloned())
        .unwrap_or_default()
}

pub fn is_available(home: &Path) -> bool {
    read_catalog_value(&home.join(relative_path())).is_some_and(|value| {
        let models = catalog_models_from_value(&value);
        runtime_compatible_models(&models)
    })
}

/// Repairs catalogs written by older Codey versions that copied model-cache
/// entries without Codex's now-required `description` fields on models and
/// their reasoning levels.
pub(crate) fn repair_missing_descriptions(home: &Path) -> Result<bool> {
    let path = home.join(relative_path());
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取待修复的 Codey 模型目录失败：{}", path.display()));
        }
    };
    let mut catalog: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("解析待修复的 Codey 模型目录失败：{}", path.display()))?;
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("待修复的 Codey 模型目录缺少 models 数组"))?;
    if !models.iter().any(model_needs_description_repair) {
        return Ok(false);
    }
    let mut repaired = false;
    for model in models.iter_mut() {
        if !model_has_runtime_description(model) {
            let description = model
                .get("display_name")
                .and_then(Value::as_str)
                .or_else(|| model.get("slug").and_then(Value::as_str))
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(ToString::to_string);
            if let Some(description) = description {
                model["description"] = json!(description);
                repaired = true;
            }
        }
        if let Some(levels) = model
            .get_mut("supported_reasoning_levels")
            .and_then(Value::as_array_mut)
        {
            for level in levels {
                if level_has_runtime_description(level) {
                    continue;
                }
                let effort = level.get("effort").and_then(Value::as_str);
                if let Some(effort) = effort {
                    level["description"] = json!(reasoning_level_description(effort));
                    repaired = true;
                }
            }
        }
    }
    if models.iter().any(model_needs_description_repair) {
        bail!("旧版 Codey 模型目录存在无法自动补全 description 的条目");
    }
    debug_assert!(repaired);

    let mut contents =
        serde_json::to_vec_pretty(&catalog).context("序列化已修复的 Codey 模型目录失败")?;
    contents.push(b'\n');
    atomic_write(&path, &contents)?;
    Ok(true)
}

pub fn is_runtime_model_cache_unavailable(error: &anyhow::Error) -> bool {
    error.is::<RuntimeModelCacheUnavailable>()
}

/// Signature of the catalog source files, used to reuse a parse across the
/// back-to-back `refresh_for_provider` + `selection_state` calls on every
/// launch and across repeated config-page lookups. The paths are part of the
/// key so entries can never leak between Codex homes.
type CatalogSignature = Vec<(PathBuf, u64, Option<std::time::SystemTime>)>;
type OfficialEntriesCache =
    std::sync::Mutex<Option<(CatalogSignature, std::sync::Arc<Vec<Value>>)>>;

static OFFICIAL_ENTRIES_CACHE: std::sync::OnceLock<OfficialEntriesCache> =
    std::sync::OnceLock::new();

fn catalog_signature(paths: &[PathBuf]) -> CatalogSignature {
    paths
        .iter()
        .map(|path| match fs::metadata(path) {
            Ok(metadata) => (path.clone(), metadata.len(), metadata.modified().ok()),
            Err(_) => (path.clone(), 0, None),
        })
        .collect()
}

fn read_official_entries(home: &Path) -> Result<std::sync::Arc<Vec<Value>>> {
    let paths = vec![home.join("models_cache.json"), home.join(relative_path())];
    let signature = catalog_signature(&paths);
    let cache = OFFICIAL_ENTRIES_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(guard) = cache.lock()
        && let Some((cached_signature, entries)) = guard.as_ref()
        && *cached_signature == signature
    {
        // 缓存命中只递增引用计数；下游要么只读、要么本来就会拷贝出自己的
        // 工作副本，无需整目录深拷贝。
        return Ok(std::sync::Arc::clone(entries));
    }
    let entries = std::sync::Arc::new(read_official_entries_uncached(&paths)?);
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((signature, std::sync::Arc::clone(&entries)));
    }
    Ok(entries)
}

fn read_official_entries_uncached(paths: &[PathBuf]) -> Result<Vec<Value>> {
    let mut catalogs = Vec::new();
    let mut bundled_fast_model_slugs = HashSet::new();
    let mut last_error = None;
    for path in paths {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                last_error = Some(error.to_string());
                continue;
            }
        };
        let value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(error) => {
                last_error = Some(error.to_string());
                continue;
            }
        };
        let models = official_models_from_value(&value);
        if !models.is_empty() {
            catalogs.push(models);
        }
    }
    if let Some(value) = codey_runtime_core::model_suffix::bundled_model_catalog() {
        let models = official_models_from_value(&value);
        if !models.is_empty() {
            bundled_fast_model_slugs.extend(models.iter().filter_map(|model| {
                declares_fast_speed_support(model)
                    .then(|| model.get("slug").and_then(Value::as_str))
                    .flatten()
                    .map(ToString::to_string)
            }));
            catalogs.push(models);
        }
    }
    if catalogs.is_empty() {
        bail!(
            "{}",
            last_error.unwrap_or_else(|| "找不到可用的 Codex 模型模板".to_string())
        );
    }

    OFFICIAL_MODELS
        .iter()
        .enumerate()
        .map(|(priority, (slug, display_name))| {
            let mut matching_models = catalogs
                .iter()
                .flat_map(|models| models.iter())
                .filter(|model| model.get("slug").and_then(Value::as_str) == Some(*slug));
            let mut model = matching_models
                .next()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Codex 模型模板缺少固定官方模型 {slug}"))?;
            let fallbacks = matching_models.collect::<Vec<_>>();
            complete_reasoning_metadata(&mut model, &fallbacks);
            normalize_official_model(&mut model, slug, display_name, priority);
            remove_fast_speed_controls(&mut model);
            if bundled_fast_model_slugs.contains(*slug) {
                add_fast_speed_controls(&mut model);
            }
            Ok(model)
        })
        .collect()
}

fn complete_reasoning_metadata(model: &mut Value, fallbacks: &[&Value]) {
    let current_efforts = reasoning_efforts_from_value(model);
    // Older Codex caches can omit this capability list or reduce it to the
    // default `low` entry. Preserve richer local lists, but repair these two
    // incomplete shapes from the best later catalog.
    let current_is_incomplete =
        current_efforts.is_empty() || (current_efforts.len() == 1 && current_efforts[0] == "low");
    if current_is_incomplete {
        let mut best_effort_count = current_efforts.len();
        let mut best_levels = None;
        for fallback in fallbacks {
            let fallback_efforts = reasoning_efforts_from_value(fallback);
            if fallback_efforts.len() > best_effort_count
                && let Some(levels) = fallback
                    .get("supported_reasoning_levels")
                    .and_then(Value::as_array)
            {
                best_effort_count = fallback_efforts.len();
                best_levels = Some(levels.clone());
            }
        }
        if let Some(levels) = best_levels {
            model["supported_reasoning_levels"] = Value::Array(levels);
        }
    }

    let supported = reasoning_efforts_from_value(model);
    let configured_default_is_valid = model
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|effort| supported.iter().any(|candidate| candidate == effort));
    if configured_default_is_valid {
        return;
    }

    let fallback_default = fallbacks.iter().find_map(|fallback| {
        fallback
            .get("default_reasoning_level")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|effort| supported.iter().any(|candidate| candidate == effort))
            .map(ToString::to_string)
    });
    if let Some(object) = model.as_object_mut() {
        object.remove("default_reasoning_level");
        if let Some(default) = fallback_default {
            object.insert("default_reasoning_level".to_string(), json!(default));
        }
    }
}

fn normalize_official_model(model: &mut Value, slug: &str, display_name: &str, priority: usize) {
    model["slug"] = json!(slug);
    model["display_name"] = json!(display_name);
    if model
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_none_or(|description| description.is_empty())
    {
        model["description"] = json!(display_name);
    }
    model["visibility"] = json!("list");
    model["priority"] = json!(priority);
    model["supported_in_api"] = json!(true);
    if model
        .get("multi_agent_version")
        .and_then(Value::as_str)
        .is_none()
    {
        match slug {
            "gpt-5.6-sol" | "gpt-5.6-terra" => {
                model["multi_agent_version"] = json!("v2");
            }
            "gpt-5.6-luna" => {
                model["multi_agent_version"] = json!("v1");
            }
            _ => {}
        }
    }
    if let Some(object) = model.as_object_mut() {
        object.remove("availability_nux");
        object.remove("upgrade");
    }
}

fn official_model_from_value(model: &Value) -> Option<OfficialModel> {
    let slug = model.get("slug")?.as_str()?.trim();
    if slug.is_empty() {
        return None;
    }
    let display_name = model
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(slug);
    Some(OfficialModel {
        slug: slug.to_string(),
        display_name: display_name.to_string(),
    })
}

fn reasoning_efforts_from_value(model: &Value) -> Vec<String> {
    model
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .or_else(|| level.as_str())
        })
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .fold(Vec::<String>::new(), |mut efforts, effort| {
            if !efforts.iter().any(|existing| existing == effort) {
                efforts.push(effort.to_string());
            }
            efforts
        })
}

fn default_reasoning_effort_from_value(model: &Value, supported: &[String]) -> String {
    let configured = model
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| supported.iter().any(|candidate| candidate == effort));
    configured
        .map(ToString::to_string)
        .or_else(|| {
            supported
                .iter()
                .find(|effort| effort.as_str() == "low")
                .cloned()
        })
        .or_else(|| supported.first().cloned())
        .unwrap_or_else(|| "low".to_string())
}

fn official_models_from_value(value: &Value) -> Vec<Value> {
    catalog_models_from_value(value)
        .into_iter()
        .filter(|model| model.get("codey_source").and_then(Value::as_str) != Some("third_party"))
        .collect()
}

fn catalog_models_from_value(value: &Value) -> Vec<Value> {
    let Some(models) = value.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    models
        .iter()
        .filter_map(|model| {
            let slug = model.get("slug")?.as_str()?.trim();
            if slug.is_empty() || !model.is_object() || !seen.insert(model_id::key(slug)) {
                return None;
            }
            let mut model = model.clone();
            model["slug"] = json!(slug);
            Some(model)
        })
        .collect()
}

fn ensure_runtime_compatible_models(models: &[Value]) -> Result<()> {
    if source_models_are_runtime_compatible(models) {
        return Ok(());
    }
    Err(RuntimeModelCacheUnavailable.into())
}

fn source_models_are_runtime_compatible(models: &[Value]) -> bool {
    !models.is_empty()
        && models.iter().all(|model| {
            model_instruction_source(model).is_some() && model_has_runtime_description(model)
        })
}

fn runtime_compatible_models(models: &[Value]) -> bool {
    !models.is_empty()
        && models.iter().all(|model| {
            model
                .get("base_instructions")
                .and_then(Value::as_str)
                .is_some()
                && model_has_runtime_description(model)
        })
}

fn model_instruction_source(model: &Value) -> Option<&str> {
    model
        .get("base_instructions")
        .and_then(Value::as_str)
        .or_else(|| {
            model
                .get("model_messages")
                .and_then(|messages| messages.get("instructions_template"))
                .and_then(Value::as_str)
        })
}

fn legacy_base_instructions(model: &Value) -> Option<String> {
    if let Some(base_instructions) = model.get("base_instructions").and_then(Value::as_str) {
        return Some(base_instructions.to_owned());
    }
    let messages = model.get("model_messages")?;
    let template = messages.get("instructions_template")?.as_str()?;
    let Some(variables) = messages
        .get("instructions_variables")
        .filter(|variables| !variables.is_null())
    else {
        return Some(template.to_owned());
    };
    let personality = variables
        .get("personality_default")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(template.replace(PERSONALITY_PLACEHOLDER, personality))
}

fn model_has_runtime_description(model: &Value) -> bool {
    model
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|description| !description.is_empty())
}

fn reasoning_level_description(effort: &str) -> String {
    REASONING_LEVEL_DESCRIPTIONS
        .iter()
        .find(|(known_effort, _)| *known_effort == effort)
        .map(|(_, description)| (*description).to_string())
        .unwrap_or_else(|| format!("{effort} reasoning"))
}

fn level_has_runtime_description(level: &Value) -> bool {
    level
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|description| !description.is_empty())
}

fn model_needs_description_repair(model: &Value) -> bool {
    !model_has_runtime_description(model)
        || model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .is_some_and(|levels| {
                levels
                    .iter()
                    .any(|level| !level_has_runtime_description(level))
            })
}

fn clamp_reasoning_efforts(model: &mut Value) {
    if let Some(levels) = model
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    {
        levels.retain(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .is_some_and(|effort| THIRD_PARTY_REASONING_EFFORTS.contains(&effort))
        });
    }
    let default = model
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !THIRD_PARTY_REASONING_EFFORTS.contains(&default) {
        model["default_reasoning_level"] = json!("xhigh");
    }
}

fn ensure_catalog_compatibility(model: &mut Value) {
    if model
        .get("base_instructions")
        .and_then(Value::as_str)
        .is_none()
    {
        let instructions = legacy_base_instructions(model);
        if let Some(instructions) = instructions {
            model["base_instructions"] = json!(instructions);
        }
    }
    if !model
        .get("supports_reasoning_summaries")
        .is_some_and(Value::is_boolean)
    {
        let supports_reasoning_summaries = model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .is_some_and(|levels| !levels.is_empty());
        model["supports_reasoning_summaries"] = json!(supports_reasoning_summaries);
    }
}

fn expose_supported_model(model: &mut Value) {
    if model.get("visibility").and_then(Value::as_str) == Some("list") {
        model["supported_in_api"] = json!(true);
    }
}

fn declares_fast_speed_support(model: &Value) -> bool {
    model
        .get("service_tiers")
        .and_then(Value::as_array)
        .is_some_and(|tiers| {
            tiers
                .iter()
                .any(|tier| tier.get("id").and_then(Value::as_str) == Some(FAST_SERVICE_TIER_ID))
        })
        || model
            .get("additional_speed_tiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers
                    .iter()
                    .any(|tier| tier.as_str() == Some(FAST_SPEED_TIER_ID))
            })
}

fn add_fast_speed_controls(model: &mut Value) {
    let service_tiers = model.get_mut("service_tiers").and_then(Value::as_array_mut);
    if let Some(service_tiers) = service_tiers {
        if !service_tiers
            .iter()
            .any(|tier| tier.get("id").and_then(Value::as_str) == Some(FAST_SERVICE_TIER_ID))
        {
            service_tiers.push(json!({
                "id": FAST_SERVICE_TIER_ID,
                "name": "Fast",
                "description": "1.5x speed, increased usage"
            }));
        }
    } else {
        model["service_tiers"] = json!([{
            "id": FAST_SERVICE_TIER_ID,
            "name": "Fast",
            "description": "1.5x speed, increased usage"
        }]);
    }

    let speed_tiers = model
        .get_mut("additional_speed_tiers")
        .and_then(Value::as_array_mut);
    if let Some(speed_tiers) = speed_tiers {
        if !speed_tiers
            .iter()
            .any(|tier| tier.as_str() == Some(FAST_SPEED_TIER_ID))
        {
            speed_tiers.push(json!(FAST_SPEED_TIER_ID));
        }
    } else {
        model["additional_speed_tiers"] = json!([FAST_SPEED_TIER_ID]);
    }
}

fn remove_fast_speed_controls(model: &mut Value) {
    if let Some(service_tiers) = model.get_mut("service_tiers").and_then(Value::as_array_mut) {
        service_tiers
            .retain(|tier| tier.get("id").and_then(Value::as_str) != Some(FAST_SERVICE_TIER_ID));
    }
    if let Some(speed_tiers) = model
        .get_mut("additional_speed_tiers")
        .and_then(Value::as_array_mut)
    {
        speed_tiers.retain(|tier| tier.as_str() != Some(FAST_SPEED_TIER_ID));
    }
}

fn synthetic_model(template: &Value, model_id: &str, index: usize) -> Value {
    let mut model = template.clone();
    model["slug"] = json!(model_id);
    model["display_name"] = json!(model_id);
    model["description"] = json!("Third-party API model");
    model["visibility"] = json!("list");
    model["priority"] = json!(1000 + index);
    model["supported_in_api"] = json!(true);
    model["codey_source"] = json!("third_party");
    model["default_reasoning_level"] = json!(THIRD_PARTY_DEFAULT_REASONING_EFFORT);
    model["supported_reasoning_levels"] = Value::Array(
        THIRD_PARTY_REASONING_EFFORTS
            .iter()
            .map(|effort| {
                json!({ "effort": effort, "description": reasoning_level_description(effort) })
            })
            .collect(),
    );
    if let Some(object) = model.as_object_mut() {
        object.remove("availability_nux");
        object.remove("upgrade");
        object.remove("default_service_tier");
        // Provider-defined models are leaf candidates in current Codex releases,
        // but they must not inherit the template model's coordinator capability.
        object.remove("multi_agent_version");
    }
    model["service_tiers"] = json!([]);
    model["additional_speed_tiers"] = json!([]);
    ensure_catalog_compatibility(&mut model);
    clamp_reasoning_efforts(&mut model);
    add_fast_speed_controls(&mut model);
    model
}

fn write_catalog(home: &Path, models: &[Value]) -> Result<()> {
    let mut catalog = serde_json::to_vec_pretty(&json!({ "models": models }))
        .context("序列化 Codey 模型目录失败")?;
    catalog.push(b'\n');
    let path = home.join(relative_path());
    if fs::read(&path).is_ok_and(|current| current == catalog) {
        protect_catalog_file(&path)?;
        return Ok(());
    }
    atomic_write(&path, &catalog)
}

fn read_catalog_value(path: &Path) -> Option<Value> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
}

fn read_runtime_catalog_models(home: &Path) -> Result<Vec<Value>> {
    let path = home.join(relative_path());
    let bytes = fs::read(&path)
        .with_context(|| format!("读取 Codey 运行时模型目录失败：{}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("解析 Codey 运行时模型目录失败：{}", path.display()))?;
    if value.get("models").and_then(Value::as_array).is_none() {
        bail!("Codey 运行时模型目录缺少 models 数组");
    }
    let models = catalog_models_from_value(&value);
    if !models.is_empty() && !runtime_compatible_models(&models) {
        bail!("Codey 运行时模型目录缺少 Codex 必需字段");
    }
    Ok(models)
}

fn protect_catalog_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("保护本地模型目录失败：{}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn official_cache() -> Value {
        let mut cache = json!({
            "client_version": "test-client",
            "models": [
                {
                    "slug": "gpt-5.6-sol",
                    "display_name": "GPT-5.6-Sol",
                    "visibility": "list",
                    "priority": 1,
                    "default_reasoning_level": "low",
                    "supported_reasoning_levels": [
                        {"effort": "low"}, {"effort": "medium"}, {"effort": "high"},
                        {"effort": "xhigh"}, {"effort": "max"}, {"effort": "ultra"}
                    ],
                    "service_tiers": [{"id": "priority"}],
                    "additional_speed_tiers": ["fast"]
                },
                {
                    "slug": "gpt-5.5",
                    "display_name": "GPT-5.5",
                    "visibility": "list",
                    "priority": 7,
                    "default_reasoning_level": "medium",
                    "supported_reasoning_levels": [{"effort": "low"}, {"effort": "xhigh"}],
                    "additional_speed_tiers": ["fast"]
                },
                {
                    "slug": "gpt-5.4",
                    "display_name": "GPT-5.4",
                    "visibility": "hide",
                    "priority": 16,
                    "default_reasoning_level": "medium",
                    "supported_reasoning_levels": [
                        {"effort": "low"}, {"effort": "medium"}, {"effort": "high"},
                        {"effort": "xhigh"}, {"effort": "max"}
                    ],
                    "service_tiers": [{"id": "priority"}],
                    "additional_speed_tiers": ["fast"],
                    "upgrade": {"model": "gpt-5.6-sol"}
                },
                {
                    "slug": "gpt-5.3-codex-spark",
                    "display_name": "GPT-5.3-Codex-Spark",
                    "visibility": "list",
                    "priority": 30,
                    "supported_in_api": false,
                    "default_reasoning_level": "high",
                    "supported_reasoning_levels": [
                        {"effort": "low"}, {"effort": "medium"},
                        {"effort": "high"}, {"effort": "xhigh"}
                    ],
                    "service_tiers": [],
                    "additional_speed_tiers": []
                },
                {"slug": "codex-auto-review", "visibility": "hide", "priority": 43}
            ]
        });
        let bundled = codey_runtime_core::model_suffix::bundled_model_catalog().unwrap();
        for (slug, _) in OFFICIAL_MODELS {
            let exists = cache["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["slug"] == slug);
            if exists {
                continue;
            }
            let model = bundled["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|model| model["slug"] == slug)
                .unwrap()
                .clone();
            cache["models"].as_array_mut().unwrap().push(model);
        }
        for model in cache["models"].as_array_mut().unwrap() {
            let slug = model["slug"].as_str().unwrap_or("test-model").to_string();
            match slug.as_str() {
                "gpt-5.6-sol" | "gpt-5.6-terra" => {
                    model["multi_agent_version"] = json!("v2");
                }
                "gpt-5.6-luna" => {
                    model["multi_agent_version"] = json!("v1");
                }
                "gpt-5.4" => {
                    model["multi_agent_version"] = json!("disabled");
                }
                _ => {
                    model.as_object_mut().unwrap().remove("multi_agent_version");
                }
            }
            model["base_instructions"] = json!(format!("test-only instructions for {slug}"));
            model["model_messages"] = json!({
                "instructions_template": "test-only template"
            });
        }
        cache
    }

    fn write_cache(home: &Path) {
        fs::write(
            home.join("models_cache.json"),
            serde_json::to_vec(&official_cache()).unwrap(),
        )
        .unwrap();
    }

    fn write_cache_with_template_only(home: &Path) {
        let mut cache = official_cache();
        for model in cache["models"].as_array_mut().unwrap() {
            let slug = model["slug"].as_str().unwrap_or("test-model").to_owned();
            model.as_object_mut().unwrap().remove("base_instructions");
            model["model_messages"] = json!({
                "instructions_template": "test-only prefix {{ personality }} suffix",
                "instructions_variables": {
                    "personality_default": format!("test-only default personality for {slug}"),
                    "personality_friendly": "test-only friendly personality",
                    "personality_pragmatic": "test-only pragmatic personality"
                }
            });
        }
        fs::write(
            home.join("models_cache.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
    }

    fn write_cache_without_fast_metadata(home: &Path) {
        let mut cache = official_cache();
        for model in cache["models"].as_array_mut().unwrap() {
            let object = model.as_object_mut().unwrap();
            object.remove("service_tiers");
            object.remove("additional_speed_tiers");
        }
        fs::write(
            home.join("models_cache.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
    }

    fn write_cache_with_prompt_fields(home: &Path) {
        let mut cache = official_cache();
        let model = &mut cache["models"][0];
        model["base_instructions"] = json!("runtime-cache-only base instructions");
        model["model_messages"] = json!({
            "instructions_template": "runtime-cache-only template",
            "instructions_variables": {
                "developer": "runtime-cache-only variable"
            }
        });
        model["compatibility"] = json!({
            "instructions_template": "runtime-cache-only nested template",
            "instructions_variables": {
                "nested": "runtime-cache-only nested variable"
            }
        });
        fs::write(
            home.join("models_cache.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
    }

    fn write_cache_with_sol_reasoning_metadata(
        home: &Path,
        supported_reasoning_levels: Option<Value>,
        default_reasoning_level: Option<&str>,
    ) {
        let mut cache = official_cache();
        let sol = cache["models"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|model| model["slug"] == "gpt-5.6-sol")
            .unwrap();
        let object = sol.as_object_mut().unwrap();
        match supported_reasoning_levels {
            Some(levels) => {
                object.insert("supported_reasoning_levels".to_string(), levels);
            }
            None => {
                object.remove("supported_reasoning_levels");
            }
        }
        match default_reasoning_level {
            Some(default) => {
                object.insert("default_reasoning_level".to_string(), json!(default));
            }
            None => {
                object.remove("default_reasoning_level");
            }
        }
        fs::write(
            home.join("models_cache.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
    }

    fn assert_native_fast(model: &Value) {
        assert!(
            model["service_tiers"]
                .as_array()
                .is_some_and(|tiers| tiers.iter().any(|tier| tier["id"] == FAST_SERVICE_TIER_ID))
        );
        assert!(
            model["additional_speed_tiers"]
                .as_array()
                .is_some_and(|tiers| tiers.iter().any(|tier| tier == FAST_SPEED_TIER_ID))
        );
    }

    fn assert_no_native_fast(model: &Value) {
        assert!(!declares_fast_speed_support(model));
    }

    #[test]
    fn official_catalog_keeps_the_fixed_order_and_native_fast_metadata() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());

        assert_eq!(
            refresh_for_provider(home.path(), true, None, &[]).unwrap(),
            OFFICIAL_MODELS.len()
        );
        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model["slug"].as_str().unwrap())
                .collect::<Vec<_>>(),
            OFFICIAL_MODELS
                .iter()
                .map(|(slug, _)| *slug)
                .collect::<Vec<_>>()
        );
        assert!(models.iter().all(|model| model["visibility"] == "list"));
        let sol = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(sol["multi_agent_version"], "v2");
        let efforts = sol["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["effort"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(efforts, ["low", "medium", "high", "xhigh", "max", "ultra"]);
        assert_eq!(sol["service_tiers"][0]["id"], "priority");
        assert_eq!(sol["supports_reasoning_summaries"], true);
        let gpt_55 = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.5")
            .unwrap();
        assert_eq!(gpt_55["service_tiers"][0]["id"], "priority");
        assert!(gpt_55.get("multi_agent_version").is_none());
        let luna = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.6-luna")
            .unwrap();
        assert_eq!(luna["multi_agent_version"], "v1");
        let gpt_54 = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.4")
            .unwrap();
        assert_eq!(gpt_54["multi_agent_version"], "disabled");
        let spark = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.3-codex-spark")
            .unwrap();
        assert_eq!(spark["supported_in_api"], true);
        assert_eq!(spark["supports_reasoning_summaries"], true);
        assert_no_native_fast(spark);
        let mini = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.4-mini")
            .unwrap();
        assert_no_native_fast(mini);
        assert_eq!(
            models
                .iter()
                .filter(|model| declares_fast_speed_support(model))
                .map(|model| model["slug"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.4",
            ]
        );
    }

    #[test]
    fn generated_catalog_preserves_official_multi_agent_markers() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        let marker = |slug: &str| {
            models
                .iter()
                .find(|model| model["slug"] == slug)
                .and_then(|model| model.get("multi_agent_version"))
                .and_then(Value::as_str)
        };

        assert_eq!(marker("gpt-5.6-sol"), Some("v2"));
        assert_eq!(marker("gpt-5.6-terra"), Some("v2"));
        assert_eq!(marker("gpt-5.6-luna"), Some("v1"));
        assert_eq!(marker("gpt-5.4"), Some("disabled"));
        assert_eq!(marker("gpt-5.5"), None);
    }

    #[test]
    fn generated_catalog_keeps_leaf_models_without_v2_coordinator_markers() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = vec![
            "gpt-5.6-luna".into(),
            "gpt-5.4".into(),
            "provider-custom-model".into(),
        ];

        refresh_for_provider(home.path(), false, Some(&upstream), &upstream).unwrap();

        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        let luna = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.6-luna")
            .unwrap();
        assert_eq!(luna["multi_agent_version"], "v1");
        let gpt_54 = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.4")
            .unwrap();
        assert_eq!(gpt_54["multi_agent_version"], "disabled");
        let custom = models
            .iter()
            .find(|model| model["slug"] == "provider-custom-model")
            .unwrap();
        assert!(custom.get("multi_agent_version").is_none());
    }

    #[test]
    fn generated_catalog_preserves_required_fields_from_the_local_native_cache() {
        let home = tempfile::tempdir().unwrap();
        write_cache_with_prompt_fields(home.path());

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let model = &catalog["models"][0];
        assert_eq!(
            model["base_instructions"],
            "runtime-cache-only base instructions"
        );
        assert_eq!(
            model["model_messages"]["instructions_template"],
            "runtime-cache-only template"
        );
        assert_eq!(
            model["compatibility"]["instructions_variables"]["nested"],
            "runtime-cache-only nested variable"
        );
        assert!(is_available(home.path()));
    }

    #[test]
    fn generated_catalog_derives_base_instructions_from_the_local_template() {
        let home = tempfile::tempdir().unwrap();
        write_cache_with_template_only(home.path());

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert!(models.iter().all(|model| {
            let template = model["model_messages"]["instructions_template"]
                .as_str()
                .unwrap();
            let personality =
                model["model_messages"]["instructions_variables"]["personality_default"]
                    .as_str()
                    .unwrap();
            model["base_instructions"] == template.replace(PERSONALITY_PLACEHOLDER, personality)
        }));
        assert!(is_available(home.path()));
    }

    #[test]
    fn generated_catalog_fills_missing_or_empty_official_descriptions() {
        let home = tempfile::tempdir().unwrap();
        let mut cache = official_cache();
        let models = cache["models"].as_array_mut().unwrap();
        models
            .iter_mut()
            .find(|model| model["slug"] == "gpt-5.6-sol")
            .unwrap()["description"] = json!("Local Sol description");
        models
            .iter_mut()
            .find(|model| model["slug"] == "gpt-5.5")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("description");
        models
            .iter_mut()
            .find(|model| model["slug"] == "gpt-5.4")
            .unwrap()["description"] = json!("   ");
        fs::write(
            home.path().join("models_cache.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert!(models.iter().all(|model| {
            model
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|description| !description.is_empty())
        }));
        let sol = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(sol["description"], "Local Sol description");
        for slug in ["gpt-5.5", "gpt-5.4"] {
            let model = models.iter().find(|model| model["slug"] == slug).unwrap();
            assert_eq!(model["description"], model["display_name"]);
        }
    }

    #[test]
    fn incomplete_local_reasoning_metadata_is_completed_from_fallback_catalog() {
        let cases = [
            (None, None, "low"),
            (None, Some("xhigh"), "xhigh"),
            (
                Some(json!([{"effort": "low", "description": "local low"}])),
                Some("low"),
                "low",
            ),
        ];

        for (supported_reasoning_levels, default_reasoning_level, expected_default) in cases {
            let home = tempfile::tempdir().unwrap();
            write_cache_with_sol_reasoning_metadata(
                home.path(),
                supported_reasoning_levels,
                default_reasoning_level,
            );

            let state = selection_state(home.path(), true, None, &[], None).unwrap();
            let sol_state = state
                .official_models
                .iter()
                .find(|model| model.slug == "gpt-5.6-sol")
                .unwrap();
            assert_eq!(
                sol_state.supported_reasoning_efforts,
                ["low", "medium", "high", "xhigh", "max", "ultra"]
            );
            assert_eq!(sol_state.default_reasoning_effort, expected_default);

            refresh_for_provider(home.path(), true, None, &[]).unwrap();
            let catalog: Value = serde_json::from_slice(
                &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
            )
            .unwrap();
            let sol = catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|model| model["slug"] == "gpt-5.6-sol")
                .unwrap();
            assert_eq!(
                reasoning_efforts_from_value(sol),
                ["low", "medium", "high", "xhigh", "max", "ultra"]
            );
            assert_eq!(sol["default_reasoning_level"], expected_default);
            assert_eq!(
                sol["base_instructions"],
                "test-only instructions for gpt-5.6-sol"
            );
            assert_eq!(
                sol["model_messages"]["instructions_template"],
                "test-only template"
            );
        }
    }

    #[test]
    fn explicit_nontrivial_local_reasoning_metadata_remains_authoritative() {
        let home = tempfile::tempdir().unwrap();
        write_cache_with_sol_reasoning_metadata(
            home.path(),
            Some(json!([
                {"effort": "low", "description": "local low"},
                {"effort": "xhigh", "description": "local xhigh"}
            ])),
            Some("xhigh"),
        );

        let state = selection_state(home.path(), true, None, &[], None).unwrap();
        let sol = state
            .official_models
            .iter()
            .find(|model| model.slug == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(sol.supported_reasoning_efforts, ["low", "xhigh"]);
        assert_eq!(sol.default_reasoning_effort, "xhigh");
    }

    #[cfg(unix)]
    #[test]
    fn generated_catalog_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let mode = fs::metadata(home.path().join(MODEL_CATALOG_RELATIVE_PATH))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn native_cache_without_fast_metadata_inherits_bundled_official_capabilities() {
        let home = tempfile::tempdir().unwrap();
        write_cache_without_fast_metadata(home.path());

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(
            models
                .iter()
                .filter(|model| declares_fast_speed_support(model))
                .map(|model| model["slug"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.4",
            ]
        );
        for slug in ["gpt-5.4-mini", "gpt-5.3-codex-spark"] {
            let model = models.iter().find(|model| model["slug"] == slug).unwrap();
            assert_no_native_fast(model);
        }
    }

    #[test]
    fn third_party_catalog_keeps_supported_official_models_before_configured_models() {
        let home = tempfile::tempdir().unwrap();
        write_cache_with_template_only(home.path());
        let upstream = vec![
            "gpt-5.6-sol".into(),
            "gpt-5.4".into(),
            "gpt-5.3-codex-spark".into(),
            "claude-sonnet".into(),
        ];
        let selected = upstream.clone();

        assert_eq!(
            refresh_for_provider(home.path(), false, Some(&upstream), &selected,).unwrap(),
            4
        );
        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model["slug"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "gpt-5.6-sol",
                "gpt-5.4",
                "gpt-5.3-codex-spark",
                "claude-sonnet",
            ]
        );
        assert_eq!(
            models[0]["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|level| level["effort"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        let gpt_54 = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.4")
            .unwrap();
        assert_eq!(gpt_54["visibility"], "list");
        assert!(gpt_54.get("upgrade").is_none());
        let spark = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.3-codex-spark")
            .unwrap();
        assert_eq!(spark["supported_in_api"], true);
        assert_no_native_fast(spark);
        let custom = models.last().unwrap();
        assert_eq!(custom["slug"], "claude-sonnet");
        assert_eq!(custom["codey_source"], "third_party");
        assert!(custom.get("multi_agent_version").is_none());
        assert_eq!(
            custom["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|level| level["effort"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh"]
        );
        assert_eq!(custom["supports_reasoning_summaries"], true);
        let custom_template = custom["model_messages"]["instructions_template"]
            .as_str()
            .unwrap();
        let custom_personality =
            custom["model_messages"]["instructions_variables"]["personality_default"]
                .as_str()
                .unwrap();
        assert_eq!(
            custom["base_instructions"],
            custom_template.replace(PERSONALITY_PLACEHOLDER, custom_personality)
        );
        assert_native_fast(custom);
        assert_native_fast(
            models
                .iter()
                .find(|model| model["slug"] == "gpt-5.6-sol")
                .unwrap(),
        );
        assert_native_fast(
            models
                .iter()
                .find(|model| model["slug"] == "gpt-5.4")
                .unwrap(),
        );
    }

    #[test]
    fn third_party_catalog_does_not_inherit_a_high_only_template() {
        let home = tempfile::tempdir().unwrap();
        write_cache_with_sol_reasoning_metadata(
            home.path(),
            Some(json!([{"effort": "high"}])),
            Some("high"),
        );
        let selected = vec!["provider-fast-coder".into()];

        assert_eq!(
            refresh_for_provider(home.path(), false, Some(&selected), &selected,).unwrap(),
            1
        );
        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let model = &catalog["models"][0];
        assert_eq!(model["slug"], "provider-fast-coder");
        assert_eq!(
            reasoning_efforts_from_value(model),
            ["low", "medium", "high", "xhigh"]
        );
        assert_eq!(
            model["default_reasoning_level"],
            THIRD_PARTY_DEFAULT_REASONING_EFFORT
        );
        for level in model["supported_reasoning_levels"].as_array().unwrap() {
            assert!(
                level_has_runtime_description(level),
                "{} level lacks the runtime-required description",
                level["effort"]
            );
        }
    }

    #[test]
    fn configured_provider_model_survives_a_missing_upstream_snapshot() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let selected = vec!["provider-fast-coder".into()];

        assert_eq!(
            refresh_for_provider(home.path(), false, None, &selected,).unwrap(),
            OFFICIAL_MODELS.len() + 1
        );
        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        let models = catalog["models"].as_array().unwrap();
        let model = models.last().unwrap();
        assert_eq!(model["slug"], "provider-fast-coder");
        assert_eq!(model["codey_source"], "third_party");
        assert_eq!(model["visibility"], "list");
        assert_eq!(model["supported_in_api"], true);
        assert_native_fast(model);
        assert!(
            !model["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn prompt_free_cold_start_falls_back_instead_of_writing_an_invalid_catalog() {
        let home = tempfile::tempdir().unwrap();

        let error = refresh_for_provider(home.path(), true, None, &[]).unwrap_err();

        assert!(error.to_string().contains("模型缓存缺少运行时必需字段"));
        assert!(is_runtime_model_cache_unavailable(&error));
        assert!(!home.path().join(MODEL_CATALOG_RELATIVE_PATH).exists());
        assert!(!is_available(home.path()));
        let state = selection_state(home.path(), true, None, &[], None).unwrap();
        assert_eq!(state.official_models.len(), OFFICIAL_MODELS.len());
    }

    #[test]
    fn prompt_free_existing_catalog_is_not_reused_as_a_runtime_fallback() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(MODEL_CATALOG_RELATIVE_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec(&codey_runtime_core::model_suffix::bundled_model_catalog().unwrap())
                .unwrap(),
        )
        .unwrap();

        assert!(!is_available(home.path()));
    }

    #[test]
    fn existing_catalog_without_description_is_not_reused_as_a_runtime_fallback() {
        let home = tempfile::tempdir().unwrap();
        let mut catalog = official_cache();
        for model in catalog["models"].as_array_mut().unwrap() {
            model["description"] = json!(model["display_name"].as_str().unwrap_or("Model"));
        }
        catalog["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("description");
        let path = home.path().join(MODEL_CATALOG_RELATIVE_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();

        assert!(!is_available(home.path()));
    }

    #[test]
    fn startup_repair_fills_legacy_catalog_descriptions_once() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(MODEL_CATALOG_RELATIVE_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "models": [
                    {
                        "slug": "model-a",
                        "display_name": "Model A",
                        "base_instructions": "instructions",
                        "supported_reasoning_levels": [
                            { "effort": "low" },
                            { "effort": "high", "description": "Existing level" }
                        ]
                    },
                    {
                        "slug": "model-b",
                        "description": "   ",
                        "base_instructions": "instructions"
                    },
                    {
                        "slug": "model-c",
                        "display_name": "Model C",
                        "description": "Existing description",
                        "base_instructions": "instructions"
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(repair_missing_descriptions(home.path()).unwrap());

        let repaired: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(repaired["models"][0]["description"], "Model A");
        assert_eq!(
            repaired["models"][0]["supported_reasoning_levels"][0]["description"],
            "Fast responses with lighter reasoning"
        );
        assert_eq!(
            repaired["models"][0]["supported_reasoning_levels"][1]["description"],
            "Existing level"
        );
        assert_eq!(repaired["models"][1]["description"], "model-b");
        assert_eq!(repaired["models"][2]["description"], "Existing description");
        assert!(is_available(home.path()));
        let repaired_contents = fs::read(&path).unwrap();

        assert!(!repair_missing_descriptions(home.path()).unwrap());
        assert_eq!(fs::read(&path).unwrap(), repaired_contents);
    }

    #[test]
    fn startup_repair_leaves_unrepairable_catalog_untouched() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(MODEL_CATALOG_RELATIVE_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = serde_json::to_vec(&json!({
            "models": [{
                "base_instructions": "instructions"
            }]
        }))
        .unwrap();
        fs::write(&path, &original).unwrap();

        let error = repair_missing_descriptions(home.path()).unwrap_err();

        assert!(error.to_string().contains("无法自动补全 description"));
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn cold_start_replaces_stale_codey_fast_metadata() {
        let home = tempfile::tempdir().unwrap();
        let mut stale_catalog = official_cache();
        for model in stale_catalog["models"].as_array_mut().unwrap() {
            add_fast_speed_controls(model);
        }
        let catalog_path = home.path().join(MODEL_CATALOG_RELATIVE_PATH);
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(&catalog_path, serde_json::to_vec(&stale_catalog).unwrap()).unwrap();

        refresh_for_provider(home.path(), true, None, &[]).unwrap();

        let catalog: Value = serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
        for slug in ["gpt-5.4-mini", "gpt-5.3-codex-spark"] {
            let model = catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|model| model["slug"] == slug)
                .unwrap();
            assert_no_native_fast(model);
        }
    }

    #[test]
    fn synced_empty_provider_catalog_hides_every_model() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = Vec::<String>::new();
        assert_eq!(
            refresh_for_provider(home.path(), false, Some(&upstream), &[],).unwrap(),
            0
        );
        let catalog: Value = serde_json::from_slice(
            &fs::read(home.path().join(MODEL_CATALOG_RELATIVE_PATH)).unwrap(),
        )
        .unwrap();
        assert_eq!(catalog["models"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn unsynced_third_party_provider_shows_the_fixed_official_models() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());

        let state = selection_state(home.path(), false, None, &[], None).unwrap();

        assert_eq!(
            state
                .official_models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            OFFICIAL_MODELS
                .iter()
                .map(|(slug, _)| *slug)
                .collect::<Vec<_>>()
        );
        assert!(state.official_models.iter().all(|model| model.supported));
        assert_eq!(state.default_model, "gpt-5.6-sol");
    }

    #[test]
    fn synced_third_party_provider_marks_unsupported_official_models() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = vec!["gpt-5.6-sol".into(), "third-model".into()];
        let state = selection_state_with_manual_models(
            home.path(),
            false,
            Some(&upstream),
            &["gpt-5.6-sol".into(), "third-model".into()],
            &["third-model".into()],
            None,
        )
        .unwrap();

        assert_eq!(
            state
                .official_models
                .iter()
                .map(|model| model.slug.as_str())
                .collect::<Vec<_>>(),
            OFFICIAL_MODELS
                .iter()
                .map(|(slug, _)| *slug)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            state
                .official_models
                .iter()
                .map(|model| (model.slug.as_str(), model.supported))
                .collect::<Vec<_>>(),
            OFFICIAL_MODELS
                .iter()
                .map(|(slug, _)| (*slug, *slug == "gpt-5.6-sol"))
                .collect::<Vec<_>>()
        );
        assert_eq!(state.third_party_models, ["third-model"]);
        assert_eq!(state.manual_third_party_models, ["third-model"]);
        assert_eq!(state.default_model, "gpt-5.6-sol");
    }

    #[test]
    fn synced_empty_provider_marks_official_models_and_configured_models_unavailable() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = Vec::new();

        let state = selection_state(
            home.path(),
            false,
            Some(&upstream),
            &["provider-fast-coder".into()],
            None,
        )
        .unwrap();

        assert_eq!(state.official_models.len(), OFFICIAL_MODELS.len());
        assert!(!state.official_models.iter().any(|model| model.supported));
        assert!(state.third_party_models.is_empty());
        assert!(state.upstream_models.is_empty());
        assert!(state.default_model.is_empty());
    }

    #[test]
    fn selection_state_does_not_add_hidden_upstream_models_to_the_fixed_list() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = vec!["gpt-5.4".into(), "codex-auto-review".into()];
        let state = selection_state(home.path(), false, Some(&upstream), &[], None).unwrap();

        assert_eq!(state.official_models.len(), OFFICIAL_MODELS.len());
        assert!(
            state
                .official_models
                .iter()
                .any(|model| model.slug == "gpt-5.4" && model.supported)
        );
        assert!(
            !state
                .official_models
                .iter()
                .any(|model| model.slug == "codex-auto-review")
        );
        assert_eq!(state.default_model, "gpt-5.4");
    }

    #[test]
    fn synced_provider_keeps_spark_when_the_channel_supports_it() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = vec!["gpt-5.3-codex-spark".into()];

        let state = selection_state(home.path(), false, Some(&upstream), &[], None).unwrap();

        assert!(
            state
                .official_models
                .iter()
                .any(|model| model.slug == "gpt-5.3-codex-spark" && model.supported)
        );
        assert_eq!(state.default_model, "gpt-5.3-codex-spark");
    }

    #[test]
    fn selection_state_uses_requested_default_when_available() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = vec![
            "GPT-5.6-SOL".into(),
            "gpt-5.6-luna".into(),
            "Third-Model".into(),
        ];
        let state = selection_state(
            home.path(),
            false,
            Some(&upstream),
            &["third-model".into(), "THIRD-MODEL".into()],
            Some("THIRD-MODEL"),
        )
        .unwrap();

        assert_eq!(state.default_model, "third-model");
        assert_eq!(
            state.available_model(" GPT-5.6-LUNA "),
            Some("gpt-5.6-luna")
        );
        assert_eq!(state.available_model("third-model"), Some("third-model"));
        assert_eq!(state.available_model("gpt-5.4"), None);
        let sol = state
            .official_models
            .iter()
            .find(|model| model.slug == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(sol.default_reasoning_effort, "low");
        assert_eq!(
            sol.supported_reasoning_efforts,
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
    }

    #[test]
    fn selection_state_falls_back_from_unavailable_default_to_first_official() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());
        let upstream = vec!["gpt-5.6-sol".into(), "third-model".into()];
        let state = selection_state(
            home.path(),
            false,
            Some(&upstream),
            &["third-model".into()],
            Some("missing-model"),
        )
        .unwrap();

        assert_eq!(state.default_model, "gpt-5.6-sol");
    }

    #[test]
    fn selection_exposes_every_model_available_on_the_current_route() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("models_cache.json"),
            serde_json::to_vec(&official_cache()).unwrap(),
        )
        .unwrap();
        let upstream = vec![
            "gpt-5.6-sol".into(),
            "gpt-5.6-terra".into(),
            "gpt-5.6-luna".into(),
            "third-model".into(),
        ];

        let state = selection_state(
            home.path(),
            false,
            Some(&upstream),
            &["third-model".into()],
            None,
        )
        .unwrap();

        for model in &upstream {
            assert!(state.available_model(model).is_some(), "{model}");
        }
        assert_eq!(state.available_model("gpt-5.4"), None);
    }

    #[test]
    fn selection_ignores_a_stale_shared_cache_client_version() {
        let home = tempfile::tempdir().unwrap();
        let mut cache = official_cache();
        cache["client_version"] = json!("0.146.1");
        fs::write(
            home.path().join("models_cache.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
        let upstream = vec![
            "gpt-5.6-sol".into(),
            "gpt-5.6-luna".into(),
            "third-model".into(),
        ];

        let state = selection_state(
            home.path(),
            false,
            Some(&upstream),
            &["third-model".into()],
            None,
        )
        .unwrap();

        assert_eq!(state.available_model("gpt-5.6-luna"), Some("gpt-5.6-luna"));
        assert_eq!(state.available_model("third-model"), Some("third-model"));
    }

    #[test]
    fn selection_does_not_depend_on_a_generated_runtime_catalog() {
        let home = tempfile::tempdir().unwrap();
        write_cache(home.path());

        let state = selection_state(home.path(), true, None, &[], None).unwrap();

        assert!(!home.path().join(relative_path()).exists());
        for model in &state.official_models {
            assert_eq!(
                state.available_model(&model.slug),
                Some(model.slug.as_str())
            );
        }
        assert!(state.first_available_model().is_some());
    }

    #[test]
    fn catalog_snapshot_restores_existing_content_and_removes_new_content() {
        let existing_home = tempfile::tempdir().unwrap();
        let existing_path = existing_home.path().join(relative_path());
        fs::create_dir_all(existing_path.parent().unwrap()).unwrap();
        fs::write(&existing_path, b"original catalog\n").unwrap();
        let existing_snapshot = snapshot(existing_home.path()).unwrap();
        fs::write(&existing_path, b"replacement catalog\n").unwrap();

        restore_snapshot(existing_snapshot).unwrap();

        assert_eq!(fs::read(&existing_path).unwrap(), b"original catalog\n");

        let new_home = tempfile::tempdir().unwrap();
        let new_path = new_home.path().join(relative_path());
        let new_snapshot = snapshot(new_home.path()).unwrap();
        fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        fs::write(&new_path, b"new catalog\n").unwrap();

        restore_snapshot(new_snapshot).unwrap();

        assert!(!new_path.exists());
    }
}
