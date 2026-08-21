use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table, value};
use uuid::Uuid;

use crate::diagnostic_log::append_diagnostic_log;

/// Environment overrides intentionally use Codey-specific names so an
/// unrelated provider SDK cannot silently change Codex's effective routing.
pub const ENV_BASE_URL: &str = "CODEY_CONFIG_BASE_URL";
pub const ENV_ROUTING_ENABLED: &str = "CODEY_CONFIG_ROUTING_ENABLED";
pub const ENV_ACTIVE_ROUTE: &str = "CODEY_CONFIG_ACTIVE_ROUTE";
pub const ENV_ACTIVE_PROVIDER: &str = "CODEY_CONFIG_ACTIVE_PROVIDER";
pub const DEFAULT_BACKUP_LIMIT: usize = 5;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldSource {
    Cli,
    Environment,
    File,
    #[default]
    Default,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigLayer {
    pub base_url: Option<String>,
    pub routing_enabled: Option<bool>,
    pub active_route: Option<String>,
    pub active_provider: Option<String>,
}

impl ConfigLayer {
    pub fn from_process_environment() -> Result<Self> {
        let base_url = non_empty_env(ENV_BASE_URL);
        let active_route = non_empty_env(ENV_ACTIVE_ROUTE);
        let active_provider = non_empty_env(ENV_ACTIVE_PROVIDER);
        let routing_enabled = match non_empty_env(ENV_ROUTING_ENABLED) {
            Some(value) => Some(parse_bool(&value).with_context(|| {
                format!("环境变量 {ENV_ROUTING_ENABLED} 必须是 true/false 或 1/0")
            })?),
            None => None,
        };
        Ok(Self {
            base_url,
            routing_enabled,
            active_route,
            active_provider,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedField<T> {
    pub value: T,
    pub source: FieldSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedConfig {
    pub base_url: ResolvedField<Option<String>>,
    pub routing_enabled: ResolvedField<bool>,
    pub active_route: ResolvedField<Option<String>>,
    pub active_provider: ResolvedField<Option<String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoutingConfig {
    /// `None` means the file omitted the flag and the built-in default is used.
    /// Keeping omission distinct from an explicit `false` is required for
    /// accurate precedence/source reporting.
    pub enabled: Option<bool>,
    pub active_route: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NonRoutingConfig {
    pub active_provider: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RouteConfig {
    pub base_url: Option<String>,
    pub wire_api: Option<String>,
}

/// Strongly typed view of the fields ConfigManager validates. The original
/// `DocumentMut` remains authoritative for formatting, comments, and fields
/// introduced by Codex, plugins, or users that Codey does not understand yet.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodexConfigSchema {
    pub base_url: Option<String>,
    pub model_provider: Option<String>,
    pub model_catalog_json: Option<String>,
    pub routing: RoutingConfig,
    pub non_routing: NonRoutingConfig,
    pub routes: BTreeMap<String, RouteConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigRevision([u8; 32]);

impl ConfigRevision {
    pub fn as_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

#[derive(Clone, Debug)]
pub struct ConfigSnapshot {
    path: PathBuf,
    exists: bool,
    raw: Arc<[u8]>,
    document: Arc<DocumentMut>,
    schema: Arc<CodexConfigSchema>,
    resolved: Arc<ResolvedConfig>,
    revision: ConfigRevision,
}

impl ConfigSnapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.exists
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    pub fn document(&self) -> &DocumentMut {
        &self.document
    }

    pub fn schema(&self) -> &CodexConfigSchema {
        &self.schema
    }

    pub fn resolved(&self) -> &ResolvedConfig {
        &self.resolved
    }

    pub fn revision(&self) -> &ConfigRevision {
        &self.revision
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigAuditEvent {
    pub operation: String,
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub routing_enabled: bool,
    pub base_url_changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub trait ConfigAuditSink: Send + Sync {
    fn record(&self, event: &ConfigAuditEvent);
}

#[derive(Default)]
pub struct DiagnosticConfigAuditSink;

impl ConfigAuditSink for DiagnosticConfigAuditSink {
    fn record(&self, event: &ConfigAuditEvent) {
        let _ = append_diagnostic_log("config_manager", event);
    }
}

pub trait FileLockGuard: Send {}

pub trait ConfigFileSystem: Send + Sync {
    fn read_optional(&self, path: &Path) -> io::Result<Option<Vec<u8>>>;
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    fn write_new_synced(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn atomic_replace(&self, source: &Path, destination: &Path) -> io::Result<()>;
    fn rename(&self, source: &Path, destination: &Path) -> io::Result<()>;
    fn remove_optional(&self, path: &Path) -> io::Result<()>;
    fn sync_parent(&self, path: &Path) -> io::Result<()>;
    fn lock_exclusive(&self, path: &Path) -> io::Result<Box<dyn FileLockGuard>>;
}

#[derive(Default)]
pub struct OsConfigFileSystem;

struct OsFileLockGuard(File);
impl FileLockGuard for OsFileLockGuard {}

impl Drop for OsFileLockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl ConfigFileSystem for OsConfigFileSystem {
    fn read_optional(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)?;
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn write_new_synced(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()
    }

    fn atomic_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        atomic_replace(source, destination)
    }

    fn rename(&self, source: &Path, destination: &Path) -> io::Result<()> {
        fs::rename(source, destination)
    }

    fn remove_optional(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn sync_parent(&self, _path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(parent) = _path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    fn lock_exclusive(&self, path: &Path) -> io::Result<Box<dyn FileLockGuard>> {
        if let Some(parent) = path.parent() {
            self.create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path)?;
        file.lock_exclusive()?;
        Ok(Box::new(OsFileLockGuard(file)))
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(io::Error::other)
    }
}

#[derive(Clone)]
pub struct ConfigManager {
    inner: Arc<ConfigManagerInner>,
}

struct ConfigManagerInner {
    path: PathBuf,
    lock_path: PathBuf,
    backup_limit: usize,
    fs: Arc<dyn ConfigFileSystem>,
    audit: Arc<dyn ConfigAuditSink>,
    cli: ConfigLayer,
    environment: Option<ConfigLayer>,
    snapshot: RwLock<Option<Arc<ConfigSnapshot>>>,
    process_lock: Arc<Mutex<()>>,
}

static PROCESS_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

impl ConfigManager {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_components(
            path,
            Arc::new(OsConfigFileSystem),
            Arc::new(DiagnosticConfigAuditSink),
            DEFAULT_BACKUP_LIMIT,
            ConfigLayer::default(),
            None,
        )
    }

    pub fn for_home(home: &Path) -> Self {
        Self::new(home.join("config.toml"))
    }

    pub fn with_components(
        path: impl Into<PathBuf>,
        fs: Arc<dyn ConfigFileSystem>,
        audit: Arc<dyn ConfigAuditSink>,
        backup_limit: usize,
        cli: ConfigLayer,
        environment: Option<ConfigLayer>,
    ) -> Self {
        let path = path.into();
        let lock_path = lock_path_for(&path);
        let process_lock = process_lock_for(&lock_path);
        Self {
            inner: Arc::new(ConfigManagerInner {
                path,
                lock_path,
                backup_limit,
                fs,
                audit,
                cli,
                environment,
                snapshot: RwLock::new(None),
                process_lock,
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn lock_path(&self) -> &Path {
        &self.inner.lock_path
    }

    pub fn cached_snapshot(&self) -> Option<Arc<ConfigSnapshot>> {
        self.inner
            .snapshot
            .read()
            .ok()
            .and_then(|value| value.clone())
    }

    pub fn load(&self) -> Result<Arc<ConfigSnapshot>> {
        self.load_operation("load")
    }

    pub fn reload(&self) -> Result<Arc<ConfigSnapshot>> {
        self.load_operation("reload")
    }

    /// Reads an immutable byte snapshot without parsing it. This narrow API is
    /// reserved for observers (for example, a route-change watcher) that must
    /// notice a transient malformed external write but must never persist it.
    pub fn read_raw(&self) -> Result<Option<Arc<[u8]>>> {
        let result = self.with_lock(|| {
            self.inner
                .fs
                .read_optional(&self.inner.path)
                .with_context(|| format!("读取 {} 失败", self.inner.path.display()))
                .map(|bytes| bytes.map(Arc::from))
        });
        match &result {
            Ok(bytes) => self.record(ConfigAuditEvent {
                operation: "read_raw".to_string(),
                path: self.inner.path.to_string_lossy().into_owned(),
                status: "ok".to_string(),
                reason: Some("observe external config changes".to_string()),
                caller: None,
                revision: bytes
                    .as_deref()
                    .map(|bytes| revision_for(true, bytes).as_hex()),
                routing_enabled: false,
                base_url_changed: false,
                error: None,
            }),
            Err(error) => self.record_failure("read_raw", None, None, error),
        }
        result
    }

    fn load_operation(&self, operation: &str) -> Result<Arc<ConfigSnapshot>> {
        let result = self.with_lock(|| self.load_locked());
        match &result {
            Ok(snapshot) => self.record(ConfigAuditEvent {
                operation: operation.to_string(),
                path: self.inner.path.to_string_lossy().into_owned(),
                status: "ok".to_string(),
                reason: None,
                caller: None,
                revision: Some(snapshot.revision.as_hex()),
                routing_enabled: snapshot.resolved.routing_enabled.value,
                base_url_changed: false,
                error: None,
            }),
            Err(error) => self.record_failure(operation, None, None, error),
        }
        result
    }

    pub fn update<F>(
        &self,
        expected_revision: Option<&ConfigRevision>,
        reason: impl Into<String>,
        caller: impl Into<String>,
        mutate: F,
    ) -> Result<Arc<ConfigSnapshot>>
    where
        F: FnOnce(&mut ConfigEditor) -> Result<()>,
    {
        let reason = reason.into();
        let caller = caller.into();
        validate_audit_metadata(&reason, &caller)?;
        let result = self.with_lock(|| {
            let current = self.load_locked()?;
            if expected_revision.is_some_and(|expected| expected != current.revision()) {
                bail!("config.toml 已被其他写者修改；请 reload 后重试");
            }
            let mut editor = ConfigEditor::new((*current.document).clone());
            mutate(&mut editor)?;
            let base_url_changed = editor.base_url_changed();
            let routing_mode_changed = editor.routing_mode_changed();
            let document = editor.finish()?;
            let raw = render_document(&document);
            let candidate = build_snapshot(
                &self.inner.path,
                true,
                raw.into_bytes(),
                &self.inner.cli,
                &self.environment_layer()?,
            )?;
            self.commit_locked(&current, &candidate)?;
            let candidate = Arc::new(candidate);
            *self
                .inner
                .snapshot
                .write()
                .expect("config snapshot lock poisoned") = Some(candidate.clone());
            self.record(ConfigAuditEvent {
                operation: if routing_mode_changed {
                    "mode_switch".to_string()
                } else if base_url_changed {
                    "base_url_change".to_string()
                } else {
                    "save".to_string()
                },
                path: self.inner.path.to_string_lossy().into_owned(),
                status: "ok".to_string(),
                reason: Some(reason.clone()),
                caller: Some(caller.clone()),
                revision: Some(candidate.revision.as_hex()),
                routing_enabled: candidate.resolved.routing_enabled.value,
                base_url_changed,
                error: None,
            });
            Ok(candidate)
        });
        if let Err(error) = &result {
            self.record_failure("save", Some(reason), Some(caller), error);
        }
        result
    }

    /// Replaces the complete TOML document through the same validated,
    /// compare-and-swap transaction as incremental edits. This is the
    /// compatibility entry point for legacy flows that already construct a
    /// complete candidate document; it still centralizes every `base_url`
    /// delta in `ConfigEditor` and emits the normal reason/caller audit event.
    pub fn replace_document(
        &self,
        expected_revision: Option<&ConfigRevision>,
        document: DocumentMut,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        self.update(expected_revision, reason, caller, move |editor| {
            editor.set_complete_document(document)
        })
    }

    pub fn replace_text(
        &self,
        expected_revision: Option<&ConfigRevision>,
        text: &str,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        let document = parse_document_text(text, &self.inner.path)?;
        self.replace_document(expected_revision, document, reason, caller)
    }

    pub fn set_routing_enabled(
        &self,
        expected_revision: Option<&ConfigRevision>,
        enabled: bool,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        self.update(expected_revision, reason, caller, |editor| {
            editor.set_routing_enabled(enabled)
        })
    }

    pub fn set_root_base_url(
        &self,
        expected_revision: Option<&ConfigRevision>,
        base_url: Option<&str>,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        self.update(expected_revision, reason, caller, |editor| {
            editor.set_root_base_url(base_url)
        })
    }

    pub fn set_provider_base_url(
        &self,
        expected_revision: Option<&ConfigRevision>,
        provider_id: &str,
        base_url: Option<&str>,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        self.update(expected_revision, reason, caller, |editor| {
            editor.set_provider_base_url(provider_id, base_url)
        })
    }

    pub fn set_active_route(
        &self,
        expected_revision: Option<&ConfigRevision>,
        route: Option<&str>,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        self.update(expected_revision, reason, caller, |editor| {
            editor.set_active_route(route)
        })
    }

    pub fn set_non_routing_provider(
        &self,
        expected_revision: Option<&ConfigRevision>,
        provider: Option<&str>,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        self.update(expected_revision, reason, caller, |editor| {
            editor.set_non_routing_provider(provider)
        })
    }

    pub fn remove(
        &self,
        expected_revision: Option<&ConfigRevision>,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        let reason = reason.into();
        let caller = caller.into();
        validate_audit_metadata(&reason, &caller)?;
        let result = self.with_lock(|| {
            let current = self.load_locked()?;
            if expected_revision.is_some_and(|expected| expected != current.revision()) {
                bail!("config.toml 已被其他写者修改；请 reload 后重试");
            }
            if current.exists {
                if self.inner.backup_limit > 0 {
                    self.rotate_backups_locked()?;
                    self.write_atomic_copy_locked(
                        &backup_path(&self.inner.path, 0),
                        current.raw(),
                    )?;
                }
                self.inner.fs.remove_optional(&self.inner.path)?;
                self.inner.fs.sync_parent(&self.inner.path)?;
            }
            let environment = self.environment_layer()?;
            let candidate = Arc::new(build_snapshot(
                &self.inner.path,
                false,
                Vec::new(),
                &self.inner.cli,
                &environment,
            )?);
            *self
                .inner
                .snapshot
                .write()
                .expect("config snapshot lock poisoned") = Some(candidate.clone());
            self.record(ConfigAuditEvent {
                operation: "remove".to_string(),
                path: self.inner.path.to_string_lossy().into_owned(),
                status: "ok".to_string(),
                reason: Some(reason.clone()),
                caller: Some(caller.clone()),
                revision: Some(candidate.revision.as_hex()),
                routing_enabled: candidate.resolved.routing_enabled.value,
                base_url_changed: current.schema.base_url.is_some()
                    || current
                        .schema
                        .routes
                        .values()
                        .any(|route| route.base_url.is_some()),
                error: None,
            });
            Ok(candidate)
        });
        if let Err(error) = &result {
            self.record_failure("remove", Some(reason), Some(caller), error);
        }
        result
    }

    pub fn restore_latest_backup(
        &self,
        reason: impl Into<String>,
        caller: impl Into<String>,
    ) -> Result<Arc<ConfigSnapshot>> {
        let reason = reason.into();
        let caller = caller.into();
        validate_audit_metadata(&reason, &caller)?;
        let result = self.with_lock(|| {
            let backup = backup_path(&self.inner.path, 0);
            let bytes = self
                .inner
                .fs
                .read_optional(&backup)?
                .ok_or_else(|| anyhow::anyhow!("没有可恢复的 config.toml.bak"))?;
            let environment = self.environment_layer()?;
            let candidate =
                build_snapshot(&self.inner.path, true, bytes, &self.inner.cli, &environment)?;
            let current = self.load_locked()?;
            self.commit_locked(&current, &candidate)?;
            let candidate = Arc::new(candidate);
            *self
                .inner
                .snapshot
                .write()
                .expect("config snapshot lock poisoned") = Some(candidate.clone());
            Ok(candidate)
        });
        match &result {
            Ok(snapshot) => self.record(ConfigAuditEvent {
                operation: "restore_backup".to_string(),
                path: self.inner.path.to_string_lossy().into_owned(),
                status: "ok".to_string(),
                reason: Some(reason),
                caller: Some(caller),
                revision: Some(snapshot.revision.as_hex()),
                routing_enabled: snapshot.resolved.routing_enabled.value,
                base_url_changed: false,
                error: None,
            }),
            Err(error) => self.record_failure("restore_backup", Some(reason), Some(caller), error),
        }
        result
    }

    fn load_locked(&self) -> Result<Arc<ConfigSnapshot>> {
        let bytes = self
            .inner
            .fs
            .read_optional(&self.inner.path)
            .with_context(|| format!("读取 {} 失败", self.inner.path.display()))?;
        let exists = bytes.is_some();
        let environment = self.environment_layer()?;
        let snapshot = Arc::new(build_snapshot(
            &self.inner.path,
            exists,
            bytes.unwrap_or_default(),
            &self.inner.cli,
            &environment,
        )?);
        *self
            .inner
            .snapshot
            .write()
            .expect("config snapshot lock poisoned") = Some(snapshot.clone());
        Ok(snapshot)
    }

    fn environment_layer(&self) -> Result<ConfigLayer> {
        self.inner
            .environment
            .clone()
            .map(Ok)
            .unwrap_or_else(ConfigLayer::from_process_environment)
    }

    fn commit_locked(&self, current: &ConfigSnapshot, candidate: &ConfigSnapshot) -> Result<()> {
        let latest = self
            .inner
            .fs
            .read_optional(&self.inner.path)
            .with_context(|| format!("提交前重新读取 {} 失败", self.inner.path.display()))?;
        let latest_revision = revision_for(latest.is_some(), latest.as_deref().unwrap_or_default());
        if latest_revision != current.revision {
            bail!("config.toml 在保存准备期间被其他写者修改；未覆盖新内容");
        }

        validate_candidate(candidate.raw())?;
        let parent = self
            .inner
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("config.toml 路径缺少父目录"))?;
        self.inner.fs.create_dir_all(parent)?;
        if current.exists && self.inner.backup_limit > 0 {
            self.rotate_backups_locked()?;
            self.write_atomic_copy_locked(&backup_path(&self.inner.path, 0), current.raw())?;
        }

        let temp = temporary_path(&self.inner.path, "write");
        let write_result = (|| -> Result<()> {
            self.inner
                .fs
                .write_new_synced(&temp, candidate.raw())
                .with_context(|| format!("写入临时配置 {} 失败", temp.display()))?;
            self.inner
                .fs
                .atomic_replace(&temp, &self.inner.path)
                .with_context(|| format!("原子替换 {} 失败", self.inner.path.display()))?;
            self.inner.fs.sync_parent(&self.inner.path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = self.inner.fs.remove_optional(&temp);
        }
        write_result
    }

    fn rotate_backups_locked(&self) -> Result<()> {
        let limit = self.inner.backup_limit;
        if limit == 0 {
            return Ok(());
        }
        let oldest = backup_path(&self.inner.path, limit.saturating_sub(1));
        self.inner.fs.remove_optional(&oldest)?;
        for generation in (1..limit).rev() {
            let source = backup_path(&self.inner.path, generation - 1);
            if self.inner.fs.read_optional(&source)?.is_none() {
                continue;
            }
            let destination = backup_path(&self.inner.path, generation);
            self.inner.fs.remove_optional(&destination)?;
            self.inner.fs.rename(&source, &destination)?;
        }
        Ok(())
    }

    fn write_atomic_copy_locked(&self, destination: &Path, bytes: &[u8]) -> Result<()> {
        let temp = temporary_path(destination, "backup");
        let result = (|| -> Result<()> {
            self.inner.fs.write_new_synced(&temp, bytes)?;
            self.inner.fs.atomic_replace(&temp, destination)?;
            self.inner.fs.sync_parent(destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.inner.fs.remove_optional(&temp);
        }
        result
    }

    fn with_lock<T>(&self, action: impl FnOnce() -> Result<T>) -> Result<T> {
        let _process = self
            .inner
            .process_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("config.toml 进程内锁已损坏"))?;
        let _file = self
            .inner
            .fs
            .lock_exclusive(&self.inner.lock_path)
            .with_context(|| format!("获取配置锁 {} 失败", self.inner.lock_path.display()))?;
        action()
    }

    fn record(&self, event: ConfigAuditEvent) {
        self.inner.audit.record(&event);
    }

    fn record_failure(
        &self,
        operation: &str,
        reason: Option<String>,
        caller: Option<String>,
        error: &anyhow::Error,
    ) {
        self.record(ConfigAuditEvent {
            operation: operation.to_string(),
            path: self.inner.path.to_string_lossy().into_owned(),
            status: "error".to_string(),
            reason,
            caller,
            revision: self
                .cached_snapshot()
                .map(|snapshot| snapshot.revision.as_hex()),
            routing_enabled: self
                .cached_snapshot()
                .is_some_and(|snapshot| snapshot.resolved.routing_enabled.value),
            base_url_changed: false,
            error: Some(error.to_string()),
        });
    }
}

pub struct ConfigEditor {
    document: DocumentMut,
    recorded_base_url_paths: BTreeSet<String>,
    original_base_urls: BTreeMap<String, Option<String>>,
    routing_mode_changed: bool,
}

impl ConfigEditor {
    fn new(document: DocumentMut) -> Self {
        let original_base_urls = collect_base_urls(&document);
        Self {
            document,
            recorded_base_url_paths: BTreeSet::new(),
            original_base_urls,
            routing_mode_changed: false,
        }
    }

    /// Edits non-routing fields while enforcing that no `base_url` value is
    /// changed outside the dedicated setters below.
    pub fn edit_document(
        &mut self,
        edit: impl FnOnce(&mut DocumentMut) -> Result<()>,
    ) -> Result<()> {
        let before = collect_base_urls(&self.document);
        edit(&mut self.document)?;
        let after = collect_base_urls(&self.document);
        if before != after {
            bail!("base_url 只能通过 ConfigEditor setter 修改");
        }
        Ok(())
    }

    /// Complete-document setter used by migration and route-profile flows.
    /// Unlike `edit_document`, this method explicitly records every base URL
    /// path that changes, so the final guard and structured audit can account
    /// for those changes without permitting direct filesystem replacement.
    pub fn set_complete_document(&mut self, document: DocumentMut) -> Result<()> {
        let previous_routing = routing_enabled_from_document(&self.document)?;
        let next_routing = routing_enabled_from_document(&document)?;
        self.routing_mode_changed |= previous_routing != next_routing;
        let next_base_urls = collect_base_urls(&document);
        self.recorded_base_url_paths.extend(changed_base_url_paths(
            &collect_base_urls(&self.document),
            &next_base_urls,
        ));
        self.document = document;
        Ok(())
    }

    pub fn set_root_base_url(&mut self, base_url: Option<&str>) -> Result<()> {
        set_base_url_item(self.document.as_table_mut(), base_url)?;
        self.recorded_base_url_paths.insert("base_url".to_string());
        Ok(())
    }

    pub fn set_provider_base_url(
        &mut self,
        provider_id: &str,
        base_url: Option<&str>,
    ) -> Result<()> {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            bail!("provider id 不能为空");
        }
        let providers = ensure_root_table(&mut self.document, "model_providers")?;
        if providers.get(provider_id).is_none() {
            providers[provider_id] = Item::Table(Table::new());
        }
        let provider = providers
            .get_mut(provider_id)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| anyhow::anyhow!("model_providers.{provider_id} 必须是 table"))?;
        set_base_url_item(provider, base_url)?;
        self.recorded_base_url_paths
            .insert(format!("model_providers.{provider_id}.base_url"));
        Ok(())
    }

    pub fn set_provider_wire_api(
        &mut self,
        provider_id: &str,
        wire_api: Option<&str>,
    ) -> Result<()> {
        let providers = ensure_root_table(&mut self.document, "model_providers")?;
        if providers.get(provider_id).is_none() {
            providers[provider_id] = Item::Table(Table::new());
        }
        let provider = providers
            .get_mut(provider_id)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| anyhow::anyhow!("model_providers.{provider_id} 必须是 table"))?;
        match wire_api.map(str::trim).filter(|value| !value.is_empty()) {
            Some(wire_api) => provider["wire_api"] = value(wire_api),
            None => {
                provider.remove("wire_api");
            }
        }
        Ok(())
    }

    pub fn set_routing_enabled(&mut self, enabled: bool) -> Result<()> {
        let codey = ensure_root_table(&mut self.document, "codey")?;
        let routing = ensure_child_table(codey, "routing")?;
        let previous = routing
            .get("enabled")
            .and_then(Item::as_bool)
            .unwrap_or(false);
        routing["enabled"] = value(enabled);
        self.routing_mode_changed |= previous != enabled;
        Ok(())
    }

    pub fn set_active_route(&mut self, route: Option<&str>) -> Result<()> {
        let codey = ensure_root_table(&mut self.document, "codey")?;
        let routing = ensure_child_table(codey, "routing")?;
        match route.map(str::trim).filter(|value| !value.is_empty()) {
            Some(route) => routing["active_route"] = value(route),
            None => {
                routing.remove("active_route");
            }
        }
        Ok(())
    }

    pub fn set_non_routing_provider(&mut self, provider: Option<&str>) -> Result<()> {
        let codey = ensure_root_table(&mut self.document, "codey")?;
        let non_routing = ensure_child_table(codey, "non_routing")?;
        match provider.map(str::trim).filter(|value| !value.is_empty()) {
            Some(provider) => non_routing["active_provider"] = value(provider),
            None => {
                non_routing.remove("active_provider");
            }
        }
        Ok(())
    }

    pub fn document(&self) -> &DocumentMut {
        &self.document
    }

    fn finish(self) -> Result<DocumentMut> {
        let current = collect_base_urls(&self.document);
        let changed = changed_base_url_paths(&self.original_base_urls, &current);
        if !changed.is_subset(&self.recorded_base_url_paths) {
            bail!("检测到未通过 ConfigEditor setter 的 base_url 修改");
        }
        parse_schema(&self.document)?.validate()?;
        Ok(self.document)
    }

    fn base_url_changed(&self) -> bool {
        self.original_base_urls != collect_base_urls(&self.document)
    }

    fn routing_mode_changed(&self) -> bool {
        self.routing_mode_changed
    }
}

impl CodexConfigSchema {
    pub fn validate(&self) -> Result<()> {
        if let Some(base_url) = self.base_url.as_deref() {
            validate_base_url(base_url, "base_url")?;
        }
        for (route_id, route) in &self.routes {
            if route_id.trim().is_empty() {
                bail!("route id 不能为空");
            }
            if let Some(base_url) = route.base_url.as_deref() {
                validate_base_url(base_url, &format!("routes.{route_id}.base_url"))?;
            }
        }
        if self.routing.enabled.unwrap_or(false) {
            let active = self
                .routing
                .active_route
                .as_deref()
                .or(self.model_provider.as_deref())
                .ok_or_else(|| anyhow::anyhow!("routing.enabled=true 时必须指定活动 route"))?;
            if !self.routes.contains_key(active) {
                bail!("活动 route「{active}」不存在于 model_providers");
            }
        }
        Ok(())
    }
}

fn build_snapshot(
    path: &Path,
    exists: bool,
    raw: Vec<u8>,
    cli: &ConfigLayer,
    environment: &ConfigLayer,
) -> Result<ConfigSnapshot> {
    let text =
        std::str::from_utf8(&raw).with_context(|| format!("{} 不是 UTF-8", path.display()))?;
    let document = parse_document_text(text, path)?;
    let schema = parse_schema(&document)?;
    schema.validate()?;
    let resolved = resolve_config(&schema, environment, cli)?;
    let revision = revision_for(exists, &raw);
    Ok(ConfigSnapshot {
        path: path.to_path_buf(),
        exists,
        raw: Arc::from(raw),
        document: Arc::new(document),
        schema: Arc::new(schema),
        resolved: Arc::new(resolved),
        revision,
    })
}

fn parse_schema(document: &DocumentMut) -> Result<CodexConfigSchema> {
    let base_url = optional_string(document.as_table(), "base_url", "base_url")?;
    let model_provider = optional_string(document.as_table(), "model_provider", "model_provider")?;
    let model_catalog_json = optional_string(
        document.as_table(),
        "model_catalog_json",
        "model_catalog_json",
    )?;
    let routing = document
        .get("codey")
        .and_then(Item::as_table_like)
        .and_then(|codey| codey.get("routing"))
        .and_then(Item::as_table_like)
        .map(parse_routing)
        .transpose()?
        .unwrap_or_default();
    let non_routing = document
        .get("codey")
        .and_then(Item::as_table_like)
        .and_then(|codey| codey.get("non_routing"))
        .and_then(Item::as_table_like)
        .map(parse_non_routing)
        .transpose()?
        .unwrap_or_default();
    let mut routes = BTreeMap::new();
    if let Some(providers) = document
        .get("model_providers")
        .and_then(Item::as_table_like)
    {
        for (route_id, item) in providers.iter() {
            let Some(route) = item.as_table_like() else {
                continue;
            };
            routes.insert(
                route_id.to_string(),
                RouteConfig {
                    base_url: optional_string_like(
                        route,
                        "base_url",
                        &format!("model_providers.{route_id}.base_url"),
                    )?,
                    wire_api: optional_string_like(
                        route,
                        "wire_api",
                        &format!("model_providers.{route_id}.wire_api"),
                    )?,
                },
            );
        }
    }
    Ok(CodexConfigSchema {
        base_url,
        model_provider,
        model_catalog_json,
        routing,
        non_routing,
        routes,
    })
}

fn parse_routing(table: &dyn toml_edit::TableLike) -> Result<RoutingConfig> {
    let enabled = match table.get("enabled") {
        Some(item) => Some(
            item.as_bool()
                .ok_or_else(|| anyhow::anyhow!("codey.routing.enabled 必须是 boolean"))?,
        ),
        None => None,
    };
    let active_route = optional_string_like(table, "active_route", "codey.routing.active_route")?;
    Ok(RoutingConfig {
        enabled,
        active_route,
    })
}

fn parse_non_routing(table: &dyn toml_edit::TableLike) -> Result<NonRoutingConfig> {
    Ok(NonRoutingConfig {
        active_provider: optional_string_like(
            table,
            "active_provider",
            "codey.non_routing.active_provider",
        )?,
    })
}

fn resolve_config(
    file: &CodexConfigSchema,
    environment: &ConfigLayer,
    cli: &ConfigLayer,
) -> Result<ResolvedConfig> {
    let base_url = resolve_optional_string(
        file.base_url.clone(),
        environment.base_url.clone(),
        cli.base_url.clone(),
    );
    if let Some(value) = base_url.value.as_deref() {
        validate_base_url(value, "effective base_url")?;
    }
    let routing_enabled = if let Some(value) = cli.routing_enabled {
        ResolvedField {
            value,
            source: FieldSource::Cli,
        }
    } else if let Some(value) = environment.routing_enabled {
        ResolvedField {
            value,
            source: FieldSource::Environment,
        }
    } else {
        ResolvedField {
            value: file.routing.enabled.unwrap_or(false),
            source: if file.routing.enabled.is_some() {
                FieldSource::File
            } else {
                FieldSource::Default
            },
        }
    };
    let active_route = resolve_optional_string(
        file.routing
            .active_route
            .clone()
            .or_else(|| file.model_provider.clone()),
        environment.active_route.clone(),
        cli.active_route.clone(),
    );
    let active_provider = resolve_optional_string(
        file.non_routing
            .active_provider
            .clone()
            .or_else(|| file.model_provider.clone()),
        environment.active_provider.clone(),
        cli.active_provider.clone(),
    );
    if routing_enabled.value {
        let active = active_route
            .value
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("有效 routing 配置缺少 active route"))?;
        if !file.routes.contains_key(active) {
            bail!("有效 active route「{active}」不存在于 model_providers");
        }
    }
    Ok(ResolvedConfig {
        base_url,
        routing_enabled,
        active_route,
        active_provider,
    })
}

fn resolve_optional_string(
    file: Option<String>,
    environment: Option<String>,
    cli: Option<String>,
) -> ResolvedField<Option<String>> {
    if let Some(value) = cli {
        return ResolvedField {
            value: Some(value),
            source: FieldSource::Cli,
        };
    }
    if let Some(value) = environment {
        return ResolvedField {
            value: Some(value),
            source: FieldSource::Environment,
        };
    }
    if let Some(value) = file {
        return ResolvedField {
            value: Some(value),
            source: FieldSource::File,
        };
    }
    ResolvedField {
        value: None,
        source: FieldSource::Default,
    }
}

fn validate_candidate(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("待保存 config.toml 不是 UTF-8")?;
    let document = parse_document_text(text, Path::new("config.toml"))
        .context("待保存 config.toml 不是合法 TOML")?;
    parse_schema(&document)?.validate()
}

fn parse_document_text(text: &str, path: &Path) -> Result<DocumentMut> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        Ok(DocumentMut::new())
    } else {
        text.parse::<DocumentMut>()
            .with_context(|| format!("解析 {} 失败", path.display()))
    }
}

fn routing_enabled_from_document(document: &DocumentMut) -> Result<Option<bool>> {
    document
        .get("codey")
        .and_then(Item::as_table_like)
        .and_then(|codey| codey.get("routing"))
        .and_then(Item::as_table_like)
        .map(parse_routing)
        .transpose()
        .map(|routing| routing.and_then(|routing| routing.enabled))
}

fn validate_base_url(value: &str, field: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(value).with_context(|| format!("{field} 不是合法 URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("{field} 必须是带主机名的 HTTP(S) URL");
    }
    Ok(())
}

fn optional_string(table: &Table, key: &str, field: &str) -> Result<Option<String>> {
    optional_string_like(table, key, field)
}

fn optional_string_like(
    table: &dyn toml_edit::TableLike,
    key: &str,
    field: &str,
) -> Result<Option<String>> {
    match table.get(key) {
        Some(item) => item
            .as_str()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("{field} 必须是非空字符串")),
        None => Ok(None),
    }
}

fn ensure_root_table<'a>(document: &'a mut DocumentMut, key: &str) -> Result<&'a mut Table> {
    if document.get(key).is_none() {
        document[key] = Item::Table(Table::new());
    }
    document
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{key} 必须是 table"))
}

fn ensure_child_table<'a>(table: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    if table.get(key).is_none() {
        table[key] = Item::Table(Table::new());
    }
    table
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{key} 必须是 table"))
}

fn set_base_url_item(table: &mut Table, base_url: Option<&str>) -> Result<()> {
    match base_url.map(str::trim).filter(|value| !value.is_empty()) {
        Some(base_url) => {
            validate_base_url(base_url, "base_url")?;
            table["base_url"] = value(base_url);
        }
        None => {
            table.remove("base_url");
        }
    }
    Ok(())
}

fn collect_base_urls(document: &DocumentMut) -> BTreeMap<String, Option<String>> {
    let mut urls = BTreeMap::new();
    if let Some(item) = document.get("base_url") {
        urls.insert("base_url".to_string(), item.as_str().map(str::to_string));
    }
    if let Some(providers) = document
        .get("model_providers")
        .and_then(Item::as_table_like)
    {
        for (provider_id, provider) in providers.iter() {
            if let Some(item) = provider
                .as_table_like()
                .and_then(|provider| provider.get("base_url"))
            {
                urls.insert(
                    format!("model_providers.{provider_id}.base_url"),
                    item.as_str().map(str::to_string),
                );
            }
        }
    }
    urls
}

fn changed_base_url_paths(
    before: &BTreeMap<String, Option<String>>,
    after: &BTreeMap<String, Option<String>>,
) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|key| before.get(*key) != after.get(*key))
        .cloned()
        .collect()
}

fn render_document(document: &DocumentMut) -> String {
    let mut rendered = document.to_string();
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

fn revision_for(exists: bool, bytes: &[u8]) -> ConfigRevision {
    let mut hasher = Sha256::new();
    hasher.update([u8::from(exists)]);
    hasher.update(bytes);
    ConfigRevision(hasher.finalize().into())
}

fn lock_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    path.with_file_name(format!("{file_name}.lock"))
}

fn backup_path(path: &Path, generation: usize) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    if generation == 0 {
        path.with_file_name(format!("{file_name}.bak"))
    } else {
        path.with_file_name(format!("{file_name}.bak.{generation}"))
    }
}

fn temporary_path(path: &Path, purpose: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    path.with_file_name(format!(
        ".{file_name}.codey-{purpose}-{}.tmp",
        Uuid::new_v4()
    ))
}

fn process_lock_for(path: &Path) -> Arc<Mutex<()>> {
    let registry = PROCESS_LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut registry = registry.lock().expect("config lock registry poisoned");
    if let Some(lock) = registry.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    registry.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => bail!("invalid boolean"),
    }
}

fn validate_audit_metadata(reason: &str, caller: &str) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("配置变更 reason 不能为空");
    }
    if caller.trim().is_empty() {
        bail!("配置变更 caller 不能为空");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Default)]
    struct CollectingAudit(Mutex<Vec<ConfigAuditEvent>>);

    impl ConfigAuditSink for CollectingAudit {
        fn record(&self, event: &ConfigAuditEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    struct FailingReplaceFs {
        inner: OsConfigFileSystem,
        target: PathBuf,
        fail: AtomicBool,
    }

    impl ConfigFileSystem for FailingReplaceFs {
        fn read_optional(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
            self.inner.read_optional(path)
        }

        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            self.inner.create_dir_all(path)
        }

        fn write_new_synced(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.inner.write_new_synced(path, bytes)
        }

        fn atomic_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
            if destination == self.target && self.fail.swap(false, Ordering::AcqRel) {
                return Err(io::Error::other("injected replace failure"));
            }
            self.inner.atomic_replace(source, destination)
        }

        fn rename(&self, source: &Path, destination: &Path) -> io::Result<()> {
            self.inner.rename(source, destination)
        }

        fn remove_optional(&self, path: &Path) -> io::Result<()> {
            self.inner.remove_optional(path)
        }

        fn sync_parent(&self, path: &Path) -> io::Result<()> {
            self.inner.sync_parent(path)
        }

        fn lock_exclusive(&self, path: &Path) -> io::Result<Box<dyn FileLockGuard>> {
            self.inner.lock_exclusive(path)
        }
    }

    fn manager(
        path: &Path,
        backup_limit: usize,
        cli: ConfigLayer,
        environment: ConfigLayer,
    ) -> (ConfigManager, Arc<CollectingAudit>) {
        let audit = Arc::new(CollectingAudit::default());
        (
            ConfigManager::with_components(
                path,
                Arc::new(OsConfigFileSystem),
                audit.clone(),
                backup_limit,
                cli,
                Some(environment),
            ),
            audit,
        )
    }

    #[test]
    fn loads_defaults_from_a_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let (manager, _) = manager(
            &temp.path().join("config.toml"),
            3,
            ConfigLayer::default(),
            ConfigLayer::default(),
        );
        let snapshot = manager.load().unwrap();
        assert!(!snapshot.exists());
        assert_eq!(snapshot.resolved().base_url.source, FieldSource::Default);
        assert_eq!(
            snapshot.resolved().routing_enabled,
            ResolvedField {
                value: false,
                source: FieldSource::Default
            }
        );
    }

    #[test]
    fn resolves_cli_before_environment_before_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            "base_url = \"https://file.example/v1\"\n\n[model_providers.file]\nbase_url = \"https://route.example/v1\"\n\n[codey.routing]\nenabled = false\nactive_route = \"file\"\n",
        )
        .unwrap();
        let (manager, _) = manager(
            &path,
            3,
            ConfigLayer {
                base_url: Some("https://cli.example/v1".into()),
                routing_enabled: Some(true),
                active_route: Some("file".into()),
                active_provider: Some("file".into()),
            },
            ConfigLayer {
                base_url: Some("https://env.example/v1".into()),
                routing_enabled: Some(false),
                active_route: None,
                active_provider: None,
            },
        );
        let snapshot = manager.load().unwrap();
        assert_eq!(
            snapshot.resolved().base_url,
            ResolvedField {
                value: Some("https://cli.example/v1".into()),
                source: FieldSource::Cli
            }
        );
        assert_eq!(snapshot.resolved().routing_enabled.source, FieldSource::Cli);
        assert_eq!(snapshot.resolved().active_provider.source, FieldSource::Cli);
    }

    #[test]
    fn mode_switch_preserves_routes_unknown_keys_and_comments() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            "# keep me\ncustom_key = \"value\"\nmodel_provider = \"route-a\"\n\n[model_providers.route-a]\nbase_url = \"https://route.example/v1\"\ncustom = 42\n\n[codey.non_routing]\nactive_provider = \"route-a\"\n",
        )
        .unwrap();
        let (manager, _) = manager(&path, 3, ConfigLayer::default(), ConfigLayer::default());
        let first = manager.load().unwrap();
        let routed = manager
            .set_routing_enabled(Some(first.revision()), true, "enable route mode", "test")
            .unwrap();
        manager
            .set_routing_enabled(Some(routed.revision()), false, "disable route mode", "test")
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("custom_key = \"value\""));
        assert!(text.contains("custom = 42"));
        assert!(text.contains("enabled = false"));
        assert!(text.contains("model_provider = \"route-a\""));
        assert!(text.contains("base_url = \"https://route.example/v1\""));
        assert!(text.contains("active_provider = \"route-a\""));
        assert_eq!(
            manager.reload().unwrap().resolved().routing_enabled.source,
            FieldSource::File
        );
    }

    #[test]
    fn base_url_requires_a_setter_and_is_audited() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "base_url = \"https://old.example/v1\"\n").unwrap();
        let (manager, audit) = manager(&path, 3, ConfigLayer::default(), ConfigLayer::default());
        let snapshot = manager.load().unwrap();
        let error = manager
            .update(Some(snapshot.revision()), "bad edit", "test", |editor| {
                editor.edit_document(|document| {
                    document["base_url"] = value("https://bad.example/v1");
                    Ok(())
                })
            })
            .unwrap_err();
        assert!(error.to_string().contains("setter"));

        let snapshot = manager.reload().unwrap();
        manager
            .update(
                Some(snapshot.revision()),
                "operator selected endpoint",
                "settings.save",
                |editor| editor.set_root_base_url(Some("https://new.example/v1")),
            )
            .unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("new.example"));
        assert!(
            audit
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|event| event.operation == "base_url_change" && event.base_url_changed)
        );
    }

    #[test]
    fn rotates_atomic_backups() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "custom = 0\n").unwrap();
        let (manager, _) = manager(&path, 3, ConfigLayer::default(), ConfigLayer::default());
        for next_value in 1..=4 {
            let snapshot = manager.reload().unwrap();
            manager
                .update(Some(snapshot.revision()), "test write", "test", |editor| {
                    editor.edit_document(|document| {
                        document["custom"] = value(next_value);
                        Ok(())
                    })
                })
                .unwrap();
        }
        assert_eq!(
            fs::read_to_string(backup_path(&path, 0)).unwrap(),
            "custom = 3\n"
        );
        assert_eq!(
            fs::read_to_string(backup_path(&path, 1)).unwrap(),
            "custom = 2\n"
        );
        assert_eq!(
            fs::read_to_string(backup_path(&path, 2)).unwrap(),
            "custom = 1\n"
        );
        assert!(!backup_path(&path, 3).exists());
    }

    #[test]
    fn stale_revisions_cannot_overwrite_a_newer_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "custom = 0\n").unwrap();
        let (manager, _) = manager(&path, 3, ConfigLayer::default(), ConfigLayer::default());
        let initial = manager.load().unwrap();
        let first = manager.clone();
        let second = manager.clone();
        let revision_a = initial.revision().clone();
        let revision_b = initial.revision().clone();
        let a = std::thread::spawn(move || {
            first.update(Some(&revision_a), "a", "thread-a", |editor| {
                editor.edit_document(|document| {
                    document["custom"] = value(1);
                    Ok(())
                })
            })
        });
        let b = std::thread::spawn(move || {
            second.update(Some(&revision_b), "b", "thread-b", |editor| {
                editor.edit_document(|document| {
                    document["custom"] = value(2);
                    Ok(())
                })
            })
        });
        let successes = [a.join().unwrap(), b.join().unwrap()]
            .into_iter()
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, 1);
        fs::read_to_string(&path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
    }

    #[test]
    fn invalid_config_and_replace_failure_leave_the_target_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let original = "base_url = \"https://old.example/v1\"\n";
        fs::write(&path, original).unwrap();
        let audit = Arc::new(CollectingAudit::default());
        let filesystem = Arc::new(FailingReplaceFs {
            inner: OsConfigFileSystem,
            target: path.clone(),
            fail: AtomicBool::new(true),
        });
        let manager = ConfigManager::with_components(
            &path,
            filesystem,
            audit,
            3,
            ConfigLayer::default(),
            Some(ConfigLayer::default()),
        );
        let snapshot = manager.load().unwrap();
        assert!(
            manager
                .update(Some(snapshot.revision()), "fail", "test", |editor| {
                    editor.set_root_base_url(Some("https://new.example/v1"))
                })
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        fs::write(&path, "base_url = [\n").unwrap();
        assert!(manager.reload().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "base_url = [\n");
    }
}
