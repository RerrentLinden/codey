use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use codey_runtime_data::{ProviderSyncResult, ProviderSyncStatus};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::default_config_path;
use crate::fs_util::timestamp_millis;
use crate::sqlite_util::table_columns;

const MARKER_VERSION: u32 = 1;
const MARKER_FILE: &str = "provider-sync-marker-v1.json";
const ROLLOUT_HEADER_CACHE_VERSION: u32 = 1;
const ROLLOUT_HEADER_CACHE_FILE: &str = "provider-sync-rollout-headers-v1.json";
const PROVIDER_SYNC_MANAGED_BY: &str = "Codey provider sync";
const SESSION_DIRS: [&str; 2] = ["sessions", "archived_sessions"];
const MAX_ROLLOUT_HEADER_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSyncPlan {
    Full,
    Cached,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderSyncMarker {
    version: u32,
    target_provider: String,
    validated_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutHeaderSignature {
    len: u64,
    modified_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutHeaderCacheEntry {
    path: PathBuf,
    signature: RolloutHeaderSignature,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutHeaderCache {
    version: u32,
    target_provider: String,
    entries: Vec<RolloutHeaderCacheEntry>,
    validated_at_ms: u128,
}

#[derive(Debug)]
struct RolloutFile {
    path: PathBuf,
    cache_key: PathBuf,
    signature: RolloutHeaderSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RolloutHeaderValidation {
    matches: bool,
    headers_read: usize,
}

pub fn provider_sync_plan(home: &Path, target_provider: &str) -> Result<ProviderSyncPlan> {
    provider_sync_plan_at(home, target_provider, &marker_path())
}

pub fn record_provider_sync_success(home: &Path, target_provider: &str) -> Result<()> {
    let validation =
        validate_rollout_headers_at(home, target_provider, &rollout_header_cache_path())?;
    if !validation.matches {
        anyhow::bail!("Provider 同步完成后会话头仍未全部匹配 {target_provider}");
    }
    write_marker(&marker_path(), target_provider)
}

pub fn cached_provider_sync_result(target_provider: &str) -> ProviderSyncResult {
    ProviderSyncResult {
        status: ProviderSyncStatus::Synced,
        message: "Provider sync cache is valid".to_string(),
        target_provider: target_provider.to_string(),
        backup_dir: None,
        changed_session_files: 0,
        skipped_locked_rollout_files: Vec::new(),
        sqlite_rows_updated: 0,
        sqlite_provider_rows_updated: 0,
        sqlite_user_event_rows_updated: 0,
        sqlite_cwd_rows_updated: 0,
        updated_workspace_roots: 0,
        encrypted_content_warning: None,
    }
}

fn marker_path() -> PathBuf {
    default_config_path()
        .parent()
        .unwrap_or_else(|| Path::new(".codey"))
        .join(MARKER_FILE)
}

fn rollout_header_cache_path() -> PathBuf {
    marker_path()
        .parent()
        .unwrap_or_else(|| Path::new(".codey"))
        .join(ROLLOUT_HEADER_CACHE_FILE)
}

fn rollout_header_cache_path_for_marker(marker: &Path) -> PathBuf {
    marker
        .parent()
        .unwrap_or_else(|| Path::new(".codey"))
        .join(ROLLOUT_HEADER_CACHE_FILE)
}

fn provider_sync_plan_at(
    home: &Path,
    target_provider: &str,
    marker: &Path,
) -> Result<ProviderSyncPlan> {
    let marker_matches = read_marker(marker).is_some_and(|saved| {
        saved.version == MARKER_VERSION && saved.target_provider == target_provider
    });
    let previous_sync_matches =
        marker_matches || has_legacy_provider_sync(home, target_provider).unwrap_or(false);
    let rollout_cache = rollout_header_cache_path_for_marker(marker);
    if !previous_sync_matches || !provider_state_matches(home, target_provider, &rollout_cache)? {
        return Ok(ProviderSyncPlan::Full);
    }
    if !marker_matches {
        write_marker(marker, target_provider)?;
    }
    Ok(ProviderSyncPlan::Cached)
}

fn read_marker(path: &Path) -> Option<ProviderSyncMarker> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write_marker(path: &Path, target_provider: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Provider 同步标记路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    let marker = ProviderSyncMarker {
        version: MARKER_VERSION,
        target_provider: target_provider.to_string(),
        validated_at_ms: timestamp_millis(),
    };
    crate::fs_util::atomic_write(path, &serde_json::to_vec_pretty(&marker)?)?;
    Ok(())
}

fn has_legacy_provider_sync(home: &Path, target_provider: &str) -> Result<bool> {
    let root = home.join("backups_state/provider-sync");
    if !root.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(bytes) = fs::read(path.join("metadata.json")) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if metadata.get("managedBy").and_then(Value::as_str) == Some(PROVIDER_SYNC_MANAGED_BY)
            && metadata.get("targetProvider").and_then(Value::as_str) == Some(target_provider)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn provider_state_matches(
    home: &Path,
    target_provider: &str,
    rollout_cache: &Path,
) -> Result<bool> {
    if !validate_rollout_headers_at(home, target_provider, rollout_cache)?.matches {
        return Ok(false);
    }
    sqlite_providers_match(home, target_provider)
}

const HEADER_VALIDATION_MAX_THREADS: usize = 4;

/// Enumerates rollout paths on every launch so additions and removals are
/// visible, but only opens files whose `(path, size, mtime)` signature changed.
fn validate_rollout_headers_at(
    home: &Path,
    target_provider: &str,
    cache_path: &Path,
) -> Result<RolloutHeaderValidation> {
    let mut files = Vec::new();
    for dirname in SESSION_DIRS {
        let root = home.join(dirname);
        if root.exists() {
            collect_rollout_files(home, &root, &mut files)?;
        }
    }
    files.sort_by(|left, right| left.cache_key.cmp(&right.cache_key));

    let cached = read_rollout_header_cache(cache_path, target_provider);
    let changed = files
        .iter()
        .filter(|file| {
            file.signature.modified_ns.is_none()
                || cached
                    .as_ref()
                    .and_then(|entries| entries.get(&file.cache_key))
                    != Some(&file.signature)
        })
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    if !rollout_file_headers_match(&changed, target_provider)? {
        return Ok(RolloutHeaderValidation {
            matches: false,
            headers_read: changed.len(),
        });
    }

    let entries = files
        .into_iter()
        .map(|file| RolloutHeaderCacheEntry {
            path: file.cache_key,
            signature: file.signature,
        })
        .collect::<Vec<_>>();
    let cache_matches = cached.as_ref().is_some_and(|cached| {
        cached.len() == entries.len()
            && entries
                .iter()
                .all(|entry| cached.get(&entry.path) == Some(&entry.signature))
    });
    let cache = RolloutHeaderCache {
        version: ROLLOUT_HEADER_CACHE_VERSION,
        target_provider: target_provider.to_string(),
        entries,
        validated_at_ms: timestamp_millis(),
    };
    // This cache only avoids repeat reads. Failing to persist it must not turn
    // a correct provider validation into a failed startup.
    if !cache_matches {
        let _ = write_rollout_header_cache(cache_path, &cache);
    }
    Ok(RolloutHeaderValidation {
        matches: true,
        headers_read: changed.len(),
    })
}

fn rollout_file_headers_match(files: &[PathBuf], target_provider: &str) -> Result<bool> {
    if files.is_empty() {
        return Ok(true);
    }
    let workers = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .min(HEADER_VALIDATION_MAX_THREADS)
        .min(files.len())
        .max(1);
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let mismatch = AtomicBool::new(false);
    let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(index) else {
                        break;
                    };
                    match rollout_header_matches(path, target_provider) {
                        Ok(Some(true)) | Ok(None) => {}
                        Ok(Some(false)) => {
                            mismatch.store(true, Ordering::Relaxed);
                            stop.store(true, Ordering::Relaxed);
                        }
                        Err(error) => {
                            let mut slot = failure.lock().unwrap_or_else(|slot| slot.into_inner());
                            if slot.is_none() {
                                *slot = Some(error);
                            }
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });

    if let Some(error) = failure
        .lock()
        .unwrap_or_else(|slot| slot.into_inner())
        .take()
    {
        return Err(error);
    }
    Ok(!mismatch.load(Ordering::Relaxed))
}

fn collect_rollout_files(home: &Path, root: &Path, files: &mut Vec<RolloutFile>) -> Result<()> {
    for entry in
        fs::read_dir(root).with_context(|| format!("扫描会话目录失败：{}", root.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            collect_rollout_files(home, &path, files)?;
            continue;
        }
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            continue;
        }
        let metadata = fs::metadata(&path)
            .with_context(|| format!("读取会话头元数据失败：{}", path.display()))?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_nanos()).ok());
        files.push(RolloutFile {
            cache_key: path.strip_prefix(home).unwrap_or(&path).to_path_buf(),
            path,
            signature: RolloutHeaderSignature {
                len: metadata.len(),
                modified_ns,
            },
        });
    }
    Ok(())
}

fn rollout_header_matches(path: &Path, target_provider: &str) -> Result<Option<bool>> {
    let file =
        fs::File::open(path).with_context(|| format!("读取会话头失败：{}", path.display()))?;
    let reader = BufReader::new(file).take(MAX_ROLLOUT_HEADER_BYTES);
    for line in reader.lines() {
        let line = line?;
        if !line.contains("session_meta") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let provider = record
            .pointer("/payload/model_provider")
            .and_then(Value::as_str);
        return Ok(Some(provider == Some(target_provider)));
    }
    // Provider sync deliberately ignores rollout files without a parseable
    // session_meta record. Treat the same files as outside validation so a
    // stale or partial rollout cannot force a full sync on every launch.
    Ok(None)
}

fn read_rollout_header_cache(
    path: &Path,
    target_provider: &str,
) -> Option<BTreeMap<PathBuf, RolloutHeaderSignature>> {
    let Ok(bytes) = fs::read(path) else {
        return None;
    };
    let Ok(cache) = serde_json::from_slice::<RolloutHeaderCache>(&bytes) else {
        return None;
    };
    if cache.version != ROLLOUT_HEADER_CACHE_VERSION || cache.target_provider != target_provider {
        return None;
    }
    Some(
        cache
            .entries
            .into_iter()
            .map(|entry| (entry.path, entry.signature))
            .collect(),
    )
}

fn write_rollout_header_cache(path: &Path, cache: &RolloutHeaderCache) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Provider 会话头缓存路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    crate::fs_util::atomic_write(path, &serde_json::to_vec_pretty(cache)?)?;
    Ok(())
}

fn sqlite_providers_match(home: &Path, target_provider: &str) -> Result<bool> {
    for path in codey_runtime_core::codex_sqlite::codex_session_db_paths_from_home(home) {
        if !path.exists() {
            continue;
        }
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("检查 Codex Provider 数据库失败：{}", path.display()))?;
        connection.busy_timeout(Duration::from_millis(250))?;
        if !table_columns(&connection, "threads")?.contains("model_provider") {
            continue;
        }
        let mismatch = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM threads
                WHERE COALESCE(model_provider, '') <> ?1
                LIMIT 1
            )",
            [target_provider],
            |row| row.get::<_, bool>(0),
        )?;
        if mismatch {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_rollout(home: &Path, name: &str, provider: &str) {
        let sessions = home.join("sessions/2026/07/20");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join(name),
            format!(
                "{}\n{}\n",
                json!({
                    "type": "session_meta",
                    "payload": {"id": "thread-1", "model_provider": provider}
                }),
                json!({"type": "response_item", "payload": "history"})
            ),
        )
        .unwrap();
    }

    fn write_legacy_sync(home: &Path, provider: &str) {
        let backup = home.join("backups_state/provider-sync/20260720180444");
        fs::create_dir_all(&backup).unwrap();
        fs::write(
            backup.join("metadata.json"),
            serde_json::to_vec(&json!({
                "managedBy": PROVIDER_SYNC_MANAGED_BY,
                "targetProvider": provider,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn first_run_without_previous_sync_requires_full_maintenance() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "codey_global");
        let marker = temp.path().join("codey/provider-sync.json");

        let plan = provider_sync_plan_at(temp.path(), "codey_global", &marker).unwrap();

        assert_eq!(plan, ProviderSyncPlan::Full);
        assert!(!marker.exists());
    }

    #[test]
    fn legacy_sync_is_adopted_after_fast_provider_validation() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "codey_global");
        write_legacy_sync(temp.path(), "codey_global");
        let marker = temp.path().join("codey/provider-sync.json");

        let plan = provider_sync_plan_at(temp.path(), "codey_global", &marker).unwrap();

        assert_eq!(plan, ProviderSyncPlan::Cached);
        assert_eq!(
            read_marker(&marker).unwrap().target_provider,
            "codey_global"
        );
    }

    #[test]
    fn provider_change_invalidates_cached_sync() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "openai");
        let marker = temp.path().join("codey/provider-sync.json");
        write_marker(&marker, "codey_global").unwrap();

        let plan = provider_sync_plan_at(temp.path(), "codey_global", &marker).unwrap();

        assert_eq!(plan, ProviderSyncPlan::Full);
    }

    #[test]
    fn cached_validation_does_not_read_conversation_bytes_after_session_meta() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions/2026/07/20");
        fs::create_dir_all(&sessions).unwrap();
        let mut rollout = serde_json::to_vec(&json!({
            "type": "session_meta",
            "payload": {"id": "thread-1", "model_provider": "codey_global"}
        }))
        .unwrap();
        rollout.extend_from_slice(b"\n\xff\xfeconversation-body");
        fs::write(sessions.join("rollout-thread-1.jsonl"), rollout).unwrap();
        let marker = temp.path().join("codey/provider-sync.json");
        write_marker(&marker, "codey_global").unwrap();

        let plan = provider_sync_plan_at(temp.path(), "codey_global", &marker).unwrap();

        assert_eq!(plan, ProviderSyncPlan::Cached);
    }

    #[test]
    fn cached_validation_uses_the_first_session_meta_as_the_rollout_header() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions/2026/07/20");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("rollout-thread-1.jsonl"),
            format!(
                "{}\n{}\n",
                json!({
                    "type": "session_meta",
                    "payload": {"id": "thread-1", "model_provider": "codey_global"}
                }),
                json!({
                    "type": "session_meta",
                    "payload": {"id": "thread-1", "model_provider": "openai"}
                }),
            ),
        )
        .unwrap();
        let marker = temp.path().join("codey/provider-sync.json");
        write_marker(&marker, "codey_global").unwrap();

        let plan = provider_sync_plan_at(temp.path(), "codey_global", &marker).unwrap();

        assert_eq!(plan, ProviderSyncPlan::Cached);
    }

    #[test]
    fn rollout_without_parseable_session_meta_does_not_force_repeated_sync() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions/2026/07/20");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("rollout-partial.jsonl"),
            "{\"type\":\"response_item\",\"payload\":\"partial\"}\n",
        )
        .unwrap();
        let cache = temp.path().join("codey/rollout-headers.json");

        let first = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();
        let second = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();

        assert!(first.matches);
        assert_eq!(first.headers_read, 1);
        assert!(second.matches);
        assert_eq!(second.headers_read, 0);
    }

    #[test]
    fn unchanged_rollout_headers_reuse_the_metadata_cache() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "codey_global");
        let cache = temp.path().join("codey/rollout-headers.json");

        let first = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();
        let first_cache = fs::read(&cache).unwrap();
        let second = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();
        let second_cache = fs::read(&cache).unwrap();

        assert_eq!(
            first,
            RolloutHeaderValidation {
                matches: true,
                headers_read: 1
            }
        );
        assert_eq!(
            second,
            RolloutHeaderValidation {
                matches: true,
                headers_read: 0
            }
        );
        assert_eq!(second_cache, first_cache);
    }

    #[test]
    fn removed_rollouts_update_the_metadata_cache() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "codey_global");
        write_rollout(temp.path(), "rollout-thread-2.jsonl", "codey_global");
        let cache = temp.path().join("codey/rollout-headers.json");
        validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();

        fs::remove_file(
            temp.path()
                .join("sessions/2026/07/20/rollout-thread-2.jsonl"),
        )
        .unwrap();
        let validation = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();
        let entries = read_rollout_header_cache(&cache, "codey_global").unwrap();

        assert_eq!(validation.headers_read, 0);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn changed_and_added_rollouts_are_revalidated() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "codey_global");
        let cache = temp.path().join("codey/rollout-headers.json");
        assert!(
            validate_rollout_headers_at(temp.path(), "codey_global", &cache)
                .unwrap()
                .matches
        );
        let sessions = temp.path().join("sessions/2026/07/20");
        fs::write(
            sessions.join("rollout-thread-1.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                json!({
                    "type": "session_meta",
                    "payload": {"id": "thread-1", "model_provider": "codey_global"}
                }),
                json!({"type": "response_item", "payload": "changed-history"}),
                json!({"type": "response_item", "payload": "more-history"}),
            ),
        )
        .unwrap();
        write_rollout(temp.path(), "rollout-thread-2.jsonl", "codey_global");

        let validation = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();

        assert!(validation.matches);
        assert_eq!(validation.headers_read, 2);
    }

    #[test]
    fn corrupt_header_cache_falls_back_to_validation() {
        let temp = tempfile::tempdir().unwrap();
        write_rollout(temp.path(), "rollout-thread-1.jsonl", "codey_global");
        let cache = temp.path().join("codey/rollout-headers.json");
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(&cache, b"not-json").unwrap();

        let validation = validate_rollout_headers_at(temp.path(), "codey_global", &cache).unwrap();

        assert!(validation.matches);
        assert_eq!(validation.headers_read, 1);
    }
}
