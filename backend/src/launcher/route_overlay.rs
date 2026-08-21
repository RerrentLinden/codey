use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::oneshot;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::error_log;

const ROUTE_OVERLAY_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const ROUTE_OVERLAY_STABLE_READS: u8 = 2;
const ROUTE_OVERLAY_FULL_VALIDATION_TICKS: u8 = 30;
const ROUTE_OVERLAY_ERROR_LOG_INITIAL_INTERVAL: Duration = Duration::from_secs(2);
const ROUTE_OVERLAY_ERROR_LOG_MAX_INTERVAL: Duration = Duration::from_secs(30);

pub(super) fn spawn_route_overlay_watcher(
    home: PathBuf,
    applied: RouteFilesSnapshot,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
    oneshot::Receiver<()>,
) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let (route_changed_tx, route_changed_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(ROUTE_OVERLAY_WATCH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let applied_fingerprint = route_change_fingerprint(&applied);
        let mut pending_external: Option<RouteChangeFingerprint> = None;
        let mut missing_config_streak = 0_u8;
        let mut route_changed_tx = Some(route_changed_tx);
        let mut error_limiter = RouteWatchErrorLimiter::new(Instant::now());
        let mut observed_stamps = None;
        let mut ticks_since_full_validation = 0_u8;

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {}
            }
            ticks_since_full_validation = ticks_since_full_validation.saturating_add(1);
            let current_stamps = match read_route_file_stamps(&home).await {
                Ok(stamps) => stamps,
                Err(error) => {
                    missing_config_streak = 0;
                    let message = error.to_string();
                    let error_key = format!("{:?}:{message}", error.kind());
                    if let Some(suppressed_count) =
                        error_limiter.should_log(&error_key, Instant::now())
                    {
                        error_log::record_failure_async(
                            "route_overlay_watch_failed",
                            "stat_cc_switch_live_route",
                            message,
                            serde_json::json!({
                                "codexHome": home,
                                "suppressedCount": suppressed_count,
                            }),
                        )
                        .await;
                    }
                    continue;
                }
            };
            let metadata_unchanged = current_stamps.is_some() && current_stamps == observed_stamps;
            if pending_external.is_none()
                && metadata_unchanged
                && ticks_since_full_validation < ROUTE_OVERLAY_FULL_VALIDATION_TICKS
            {
                error_limiter.reset();
                continue;
            }
            let current = match read_route_files(&home).await {
                Ok(Some(current)) => {
                    error_limiter.reset();
                    missing_config_streak = 0;
                    observed_stamps = current_stamps;
                    ticks_since_full_validation = 0;
                    current
                }
                Ok(None) => {
                    error_limiter.reset();
                    pending_external = None;
                    observed_stamps = None;
                    ticks_since_full_validation = 0;
                    if !observe_missing_route_config(&mut missing_config_streak) {
                        continue;
                    }
                    if let Some(sender) = route_changed_tx.take() {
                        let _ = sender.send(());
                    }
                    break;
                }
                Err(error) => {
                    missing_config_streak = 0;
                    let message = error.to_string();
                    let error_key = format!("{:?}:{message}", error.kind());
                    if let Some(suppressed_count) =
                        error_limiter.should_log(&error_key, Instant::now())
                    {
                        error_log::record_failure_async(
                            "route_overlay_watch_failed",
                            "read_cc_switch_live_route",
                            message,
                            serde_json::json!({
                                "codexHome": home,
                                "suppressedCount": suppressed_count,
                            }),
                        )
                        .await;
                    }
                    continue;
                }
            };
            let current_fingerprint = route_change_fingerprint(&current);
            if applied_fingerprint == current_fingerprint {
                pending_external = None;
                continue;
            }
            if pending_external.as_ref() != Some(&current_fingerprint) {
                pending_external = Some(current_fingerprint);
                continue;
            }

            if let Some(sender) = route_changed_tx.take() {
                let _ = sender.send(());
            }
            break;
        }
    });
    (shutdown_tx, task, route_changed_rx)
}

struct RouteWatchErrorLimiter {
    error_key: Option<String>,
    interval: Duration,
    next_log_at: Instant,
    suppressed_count: u64,
}

impl RouteWatchErrorLimiter {
    fn new(now: Instant) -> Self {
        Self {
            error_key: None,
            interval: ROUTE_OVERLAY_ERROR_LOG_INITIAL_INTERVAL,
            next_log_at: now,
            suppressed_count: 0,
        }
    }

    fn reset(&mut self) {
        self.error_key = None;
        self.interval = ROUTE_OVERLAY_ERROR_LOG_INITIAL_INTERVAL;
        self.suppressed_count = 0;
    }

    fn should_log(&mut self, error_key: &str, now: Instant) -> Option<u64> {
        if self.error_key.as_deref() != Some(error_key) {
            self.error_key = Some(error_key.to_string());
            self.interval = ROUTE_OVERLAY_ERROR_LOG_INITIAL_INTERVAL;
            self.next_log_at = now + self.interval;
            self.suppressed_count = 0;
            return Some(0);
        }
        if now < self.next_log_at {
            self.suppressed_count = self.suppressed_count.saturating_add(1);
            return None;
        }
        let suppressed_count = std::mem::take(&mut self.suppressed_count);
        self.next_log_at = now + self.interval;
        self.interval = self
            .interval
            .saturating_mul(2)
            .min(ROUTE_OVERLAY_ERROR_LOG_MAX_INTERVAL);
        Some(suppressed_count)
    }
}

fn observe_missing_route_config(missing_streak: &mut u8) -> bool {
    *missing_streak = missing_streak.saturating_add(1);
    *missing_streak >= ROUTE_OVERLAY_STABLE_READS
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RouteFilesSnapshot {
    pub(super) config: Vec<u8>,
    pub(super) auth: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteFileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteFilesStamps {
    config: RouteFileStamp,
    auth: Option<RouteFileStamp>,
}

async fn read_route_file_stamp(path: &std::path::Path) -> std::io::Result<Option<RouteFileStamp>> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(Some(RouteFileStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

async fn read_route_file_stamps(
    home: &std::path::Path,
) -> std::io::Result<Option<RouteFilesStamps>> {
    let Some(config) = read_route_file_stamp(&home.join("config.toml")).await? else {
        return Ok(None);
    };
    let auth = read_route_file_stamp(&home.join("auth.json")).await?;
    Ok(Some(RouteFilesStamps { config, auth }))
}

pub(super) async fn read_route_files(
    home: &std::path::Path,
) -> std::io::Result<Option<RouteFilesSnapshot>> {
    let Some(config) = codey_runtime_core::config_manager::ConfigManager::for_home(home)
        .read_raw()
        .map_err(std::io::Error::other)?
    else {
        return Ok(None);
    };
    let auth = match tokio::fs::read(home.join("auth.json")).await {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    Ok(Some(RouteFilesSnapshot {
        config: config.to_vec(),
        auth,
    }))
}

#[cfg(test)]
fn route_files_changed(applied: &RouteFilesSnapshot, current: &RouteFilesSnapshot) -> bool {
    route_change_fingerprint(applied) != route_change_fingerprint(current)
}

fn route_change_fingerprint(snapshot: &RouteFilesSnapshot) -> RouteChangeFingerprint {
    match (
        route_config_signature(&snapshot.config),
        route_auth_signature(snapshot.auth.as_deref()),
    ) {
        (Some(config), Some(auth)) => RouteChangeFingerprint::Semantic(RouteFilesSignature {
            provider_id: config.provider_id,
            base_url: config.base_url.clone(),
            wire_api: config.wire_api,
            api_key: config.api_key.or(auth.api_key.clone()),
            auth_uses_proxy_route: auth.api_key.as_deref() == Some("PROXY_MANAGED"),
            official_auth_mode: route_base_url_is_official(&config.base_url)
                .then_some(auth.auth_mode.as_deref() == Some("chatgpt")),
        }),
        _ => RouteChangeFingerprint::Raw(snapshot.clone()),
    }
}

#[cfg(test)]
fn route_config_changed(applied: &[u8], current: &[u8]) -> bool {
    match (
        route_config_signature(applied),
        route_config_signature(current),
    ) {
        (Some(applied), Some(current)) => applied != current,
        _ => applied != current,
    }
}

/// The overlay watcher must restart Codex only when the user actually switches
/// the CC Switch route, not when Codex rewrites `config.toml` while starting
/// up. Newer Codex builds normalise that file on launch (whitespace, field
/// order, default values), so a byte-level comparison treats the self-rewrite
/// as a route switch and restarts Codex in a loop. Compare only the active
/// provider, canonical endpoint, effective wire protocol, and route credential.
/// Unparseable snapshots fall back to raw bytes so malformed writes stay visible.
fn route_config_signature(config: &[u8]) -> Option<RouteConfigSignature> {
    let text = std::str::from_utf8(config).ok()?;
    let document = DocumentMut::from_str(text).ok()?;
    let provider_id = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())?;
    let provider = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like)?;
    let base_url = provider
        .get("base_url")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .trim_end_matches('/');
    let wire_api = provider
        .get("wire_api")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("responses");
    let wire_api = if wire_api.eq_ignore_ascii_case("responses") {
        "responses".to_string()
    } else if wire_api.to_ascii_lowercase().contains("chat") {
        "chat".to_string()
    } else {
        wire_api.to_ascii_lowercase()
    };
    Some(RouteConfigSignature {
        provider_id: provider_id.to_owned(),
        base_url: base_url.to_owned(),
        wire_api,
        api_key: route_config_api_key(&document, provider),
    })
}

fn route_config_api_key(document: &DocumentMut, provider: &dyn TableLike) -> Option<String> {
    const PROVIDER_KEYS: &[&str] = &[
        "experimental_bearer_token",
        "api_key",
        "apikey",
        "bearer_token",
        "token",
    ];
    PROVIDER_KEYS
        .iter()
        .find_map(|key| provider.get(key).and_then(Item::as_str))
        .or_else(|| {
            document
                .get("experimental_bearer_token")
                .and_then(Item::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[derive(Clone, PartialEq, Eq)]
struct RouteConfigSignature {
    provider_id: String,
    base_url: String,
    wire_api: String,
    api_key: Option<String>,
}

fn route_base_url_is_official(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    base_url.contains("chatgpt.com/backend-api/codex") || base_url.contains("api.openai.com")
}

/// Only fields consumed by Live route resolution belong in the route signature.
/// ChatGPT token refreshes and unrelated account metadata must not restart Codex.
fn route_auth_signature(auth: Option<&[u8]>) -> Option<RouteAuthSignature> {
    let Some(auth) = auth else {
        return Some(RouteAuthSignature::default());
    };
    let auth: serde_json::Value = serde_json::from_slice(auth).ok()?;
    if !auth.is_object() {
        return None;
    }
    let auth_mode = auth
        .get("auth_mode")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let api_key = auth
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Some(RouteAuthSignature { auth_mode, api_key })
}

#[cfg(test)]
fn route_auth_changed(applied: Option<&[u8]>, current: Option<&[u8]>) -> bool {
    match (route_auth_signature(applied), route_auth_signature(current)) {
        (Some(applied), Some(current)) => applied != current,
        _ => applied != current,
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct RouteAuthSignature {
    auth_mode: Option<String>,
    api_key: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct RouteFilesSignature {
    provider_id: String,
    base_url: String,
    wire_api: String,
    api_key: Option<String>,
    auth_uses_proxy_route: bool,
    official_auth_mode: Option<bool>,
}

#[derive(Clone, PartialEq, Eq)]
enum RouteChangeFingerprint {
    Semantic(RouteFilesSignature),
    Raw(RouteFilesSnapshot),
}

#[cfg(test)]
mod tests;
