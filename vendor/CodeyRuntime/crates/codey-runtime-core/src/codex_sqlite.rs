use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::Connection;

pub fn default_codex_home_dir() -> PathBuf {
    crate::codex_home::default_codex_home_dir()
}

pub fn codex_session_db_path() -> PathBuf {
    codex_session_db_path_from_home(&default_codex_home_dir())
}

pub fn codex_session_db_path_from_home(home: &Path) -> PathBuf {
    let paths = codex_session_db_paths_from_home(home);
    let legacy = legacy_state_db_path(home);
    paths
        .iter()
        .find(|path| sqlite_has_table(path, "threads"))
        .cloned()
        .or_else(|| paths.into_iter().next())
        .unwrap_or(legacy)
}

pub fn codex_session_db_paths_from_home(home: &Path) -> Vec<PathBuf> {
    CodexSessionDbDiscoveryCache::default().session_db_paths_from_home(home)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionDbFileSignature {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl SessionDbFileSignature {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionDbCandidateSignature {
    database: SessionDbFileSignature,
    wal: Option<SessionDbFileSignature>,
}

impl SessionDbCandidateSignature {
    fn read(path: &Path) -> Option<Self> {
        let database = SessionDbFileSignature::from_metadata(&fs::metadata(path).ok()?);
        let mut wal_path = path.as_os_str().to_os_string();
        wal_path.push("-wal");
        let wal_path = PathBuf::from(wal_path);
        let wal = fs::metadata(wal_path)
            .ok()
            .map(|metadata| SessionDbFileSignature::from_metadata(&metadata));
        Some(Self { database, wal })
    }
}

#[derive(Debug)]
struct CachedSessionDbProbe {
    signature: SessionDbCandidateSignature,
    has_session_table: bool,
}

/// Stateful discovery for callers that scan session databases repeatedly.
///
/// Directory membership is checked on every scan, while an unchanged candidate
/// reuses its previous schema classification. The database and WAL signatures
/// invalidate that classification after replacement or a later schema change.
#[derive(Debug, Default)]
pub struct CodexSessionDbDiscoveryCache {
    candidates: HashMap<PathBuf, CachedSessionDbProbe>,
    #[cfg(test)]
    probe_count: usize,
}

impl CodexSessionDbDiscoveryCache {
    pub fn session_db_paths_from_home(&mut self, home: &Path) -> Vec<PathBuf> {
        let candidates = codex_sqlite_dir_candidates(home);
        let active_candidates = candidates.iter().cloned().collect::<HashSet<_>>();
        self.candidates
            .retain(|path, _| active_candidates.contains(path));

        let mut paths = Vec::new();
        for path in candidates {
            let Some(signature) = SessionDbCandidateSignature::read(&path) else {
                self.candidates.remove(&path);
                continue;
            };
            let has_session_table = if let Some(cached) = self
                .candidates
                .get(&path)
                .filter(|cached| cached.signature == signature)
            {
                cached.has_session_table
            } else {
                let has_session_table = has_session_table(&path);
                #[cfg(test)]
                {
                    self.probe_count += 1;
                }
                self.candidates.insert(
                    path.clone(),
                    CachedSessionDbProbe {
                        signature,
                        has_session_table,
                    },
                );
                has_session_table
            };
            if has_session_table {
                paths.push(path);
            }
        }

        let legacy = legacy_state_db_path(home);
        if !paths.iter().any(|path| path == &legacy) {
            paths.push(legacy);
        }
        paths
    }
}

/// codex 客户端日志数据库路径（固定文件名）。
pub fn codex_logs_db_path_from_home(home: &Path) -> PathBuf {
    home.join("logs_2.sqlite")
}

pub fn codex_sqlite_sidecar_paths(db_path: &Path) -> [PathBuf; 3] {
    [
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.to_string_lossy())),
        PathBuf::from(format!("{}-shm", db_path.to_string_lossy())),
    ]
}

pub fn relative_to_codex_home(home: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(home).unwrap_or(path).to_path_buf()
}

fn legacy_state_db_path(home: &Path) -> PathBuf {
    home.join("state_5.sqlite")
}

fn codex_sqlite_dir_candidates(home: &Path) -> Vec<PathBuf> {
    let sqlite_dir = home.join("sqlite");
    let Ok(entries) = fs::read_dir(sqlite_dir) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_file())
                .map(|_| entry.path())
        })
        .filter(|path| is_sqlite_candidate(path))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        (
            path.file_name()
                .map(|name| name != OsStr::new("codex-dev.db"))
                .unwrap_or(true),
            path.file_name().map(|name| name.to_os_string()),
        )
    });
    candidates
}

fn is_sqlite_candidate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("db") | Some("sqlite") | Some("sqlite3")
    )
}

fn has_session_table(path: &Path) -> bool {
    let Ok(db) = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    db.query_row(
        concat!(
            "SELECT 1 FROM sqlite_master ",
            "WHERE type = 'table' AND name IN (",
            "'threads', 'automation_runs', 'inbox_items', 'local_thread_catalog'",
            ") LIMIT 1"
        ),
        [],
        |_| Ok(()),
    )
    .is_ok()
}

fn sqlite_has_table(path: &Path, table: &str) -> bool {
    let Ok(db) = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    db.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SanitizeModelSuffixResult {
    pub scanned: usize,
    pub updated: usize,
}

/// 扫描 codex session 数据库中的 threads 表，把 model 字段里带合法后缀的
/// 记录改写为剥离后缀的 slug，使 codex 模型选择器不再显示带后缀的历史项。
pub fn sanitize_thread_model_suffixes(home: &Path) -> anyhow::Result<SanitizeModelSuffixResult> {
    let mut result = SanitizeModelSuffixResult::default();
    for db_path in codex_session_db_paths_from_home(home) {
        if !db_path.exists() {
            continue;
        }
        let (scanned, updated) = sanitize_thread_model_suffixes_in_db(&db_path)?;
        result.scanned += scanned;
        result.updated += updated;
    }
    Ok(result)
}

/// 同时清理 threads.model 与 logs_2.sqlite 中残留的带后缀模型名。
/// 返回的 scanned/updated 只统计 threads 表的改动数量；日志清理仅作为副作用。
pub fn sanitize_historical_model_suffixes(
    home: &Path,
) -> anyhow::Result<SanitizeModelSuffixResult> {
    let result = sanitize_thread_model_suffixes(home)?;
    if let Err(error) = sanitize_logs_model_suffixes(home) {
        // 日志清理失败不应阻断启动流程，仅记录诊断日志。
        let _ = crate::diagnostic_log::append_diagnostic_log(
            "codex_sqlite.sanitize_logs_model_suffixes_failed",
            serde_json::json!({
                "error": error.to_string(),
            }),
        );
    }
    Ok(result)
}

fn sanitize_thread_model_suffixes_in_db(db_path: &Path) -> anyhow::Result<(usize, usize)> {
    let mut conn = Connection::open(db_path)?;
    let tx = conn.transaction()?;
    let has_model = tx
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'threads' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok()
        && tx
            .query_row(
                "SELECT 1 FROM pragma_table_info('threads') WHERE name = 'model' LIMIT 1",
                [],
                |_| Ok(()),
            )
            .is_ok();
    if !has_model {
        return Ok((0, 0));
    }

    let mut stmt = tx.prepare("SELECT id, model FROM threads WHERE model LIKE '%[%'")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    let scanned = rows.len();
    let mut updated = 0;
    for (id, model) in rows {
        let (slug, suffix_window) = crate::model_suffix::parse_model_suffix(&model);
        if suffix_window.is_some() && slug != model {
            tx.execute("UPDATE threads SET model = ?1 WHERE id = ?2", [&slug, &id])?;
            updated += 1;
        }
    }
    tx.commit()?;
    Ok((scanned, updated))
}

/// 清理 logs_2.sqlite 中 feedback_log_body 字段里包含模型后缀的日志。
/// 这些日志只是历史记录，不会直接影响模型选择器，但清理后可避免
/// 诊断/遥测中继续出现已废弃的带后缀模型名。
fn sanitize_logs_model_suffixes(home: &Path) -> anyhow::Result<()> {
    let db_path = codex_logs_db_path_from_home(home);
    if !db_path.exists() {
        return Ok(());
    }
    let mut conn = Connection::open(&db_path)?;
    let has_table = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'logs' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_table {
        return Ok(());
    }
    let has_body = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('logs') WHERE name = 'feedback_log_body' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_body {
        return Ok(());
    }
    // 用保守模式匹配：包含 '[' 且以 ']%' 或包含 '[1M]' 等常见后缀。
    // 这里只替换明确符合 parse_model_suffix 规则的模型名，避免误改无关日志文本。
    let tx = conn.transaction()?;
    let mut stmt =
        tx.prepare("SELECT rowid, feedback_log_body FROM logs WHERE feedback_log_body LIKE '%[%'")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    let mut update = tx.prepare("UPDATE logs SET feedback_log_body = ?1 WHERE rowid = ?2")?;
    for (rowid, body) in rows {
        let sanitized = sanitize_model_suffixes_in_text(&body);
        if sanitized != body {
            update.execute([&sanitized, &rowid.to_string()])?;
        }
    }
    drop(update);
    tx.commit()?;
    Ok(())
}

/// 在一段文本中把所有符合 "slug[<number>K|M]" 格式的模型窗口后缀替换为纯 slug。
/// 只处理明确看起来像窗口大小后缀的形式（如 [1M]、[200K]），避免误改普通数组下标。
pub(crate) fn sanitize_model_suffixes_in_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut last = 0; // 上次已复制到 result 的字符索引
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            // 向后找窗口后缀：数字 + K/M（大小写均可）
            let digits_start = i + 1;
            let mut j = digits_start;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let has_digits = j > digits_start;
            let unit_seen = j < chars.len() && matches!(chars[j], 'K' | 'k' | 'M' | 'm');
            if unit_seen {
                j += 1;
            }
            if has_digits && unit_seen && j < chars.len() && chars[j] == ']' {
                // 向前找 slug
                let mut slug_start = i;
                while slug_start > 0 && is_model_id_char(chars[slug_start - 1]) {
                    slug_start -= 1;
                }
                if slug_start < i {
                    result.extend(chars[last..slug_start].iter());
                    result.extend(chars[slug_start..i].iter());
                    last = j + 1;
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    result.extend(chars[last..].iter());
    result
}

fn is_model_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '/' || c == '_' || c == '-' || c == ':'
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{
        CodexSessionDbDiscoveryCache, codex_session_db_paths_from_home,
        sanitize_logs_model_suffixes, sanitize_model_suffixes_in_text,
        sanitize_thread_model_suffixes_in_db,
    };

    #[test]
    fn discovers_catalog_only_session_databases() {
        let home = tempdir().unwrap();
        let sqlite = home.path().join("sqlite");
        std::fs::create_dir_all(&sqlite).unwrap();
        let catalog = sqlite.join("catalog.db");
        Connection::open(&catalog)
            .unwrap()
            .execute(
                "CREATE TABLE local_thread_catalog (thread_id TEXT NOT NULL)",
                [],
            )
            .unwrap();

        assert!(
            codex_session_db_paths_from_home(home.path())
                .iter()
                .any(|path| path == &catalog)
        );
    }

    #[test]
    fn discovery_cache_skips_unchanged_probes_and_detects_wal_schema_changes() {
        let home = tempdir().unwrap();
        let sqlite = home.path().join("sqlite");
        std::fs::create_dir_all(&sqlite).unwrap();
        let candidate = sqlite.join("candidate.db");
        let database = Connection::open(&candidate).unwrap();
        let journal_mode = database
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .unwrap();
        assert_eq!(journal_mode, "wal");
        database
            .execute("CREATE TABLE unrelated (id INTEGER)", [])
            .unwrap();

        let legacy = home.path().join("state_5.sqlite");
        let mut cache = CodexSessionDbDiscoveryCache::default();

        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![legacy.clone()]
        );
        assert_eq!(cache.probe_count, 1);

        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![legacy.clone()]
        );
        assert_eq!(
            cache.probe_count, 1,
            "an unchanged candidate must not query sqlite_master again"
        );

        database
            .execute("CREATE TABLE threads (id TEXT PRIMARY KEY)", [])
            .unwrap();

        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![candidate.clone(), legacy.clone()]
        );
        assert_eq!(
            cache.probe_count, 2,
            "a schema change recorded only in the WAL must invalidate discovery"
        );

        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![candidate, legacy]
        );
        assert_eq!(cache.probe_count, 2);
    }

    #[test]
    fn discovery_cache_tracks_add_delete_replace_and_keeps_legacy_state() {
        let home = tempdir().unwrap();
        let sqlite = home.path().join("sqlite");
        std::fs::create_dir_all(&sqlite).unwrap();
        let legacy = home.path().join("state_5.sqlite");
        Connection::open(&legacy)
            .unwrap()
            .execute("CREATE TABLE unrelated (id INTEGER)", [])
            .unwrap();

        let primary = sqlite.join("primary.db");
        Connection::open(&primary)
            .unwrap()
            .execute("CREATE TABLE threads (id TEXT PRIMARY KEY)", [])
            .unwrap();
        let mut cache = CodexSessionDbDiscoveryCache::default();

        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![primary.clone(), legacy.clone()]
        );
        assert_eq!(cache.probe_count, 1);

        let added = sqlite.join("added.sqlite");
        Connection::open(&added)
            .unwrap()
            .execute("CREATE TABLE automation_runs (id TEXT PRIMARY KEY)", [])
            .unwrap();
        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![added.clone(), primary.clone(), legacy.clone()]
        );
        assert_eq!(cache.probe_count, 2);

        let replacement = home.path().join("replacement.sqlite");
        Connection::open(&replacement)
            .unwrap()
            .execute("CREATE TABLE unrelated (id INTEGER)", [])
            .unwrap();
        std::fs::remove_file(&primary).unwrap();
        std::fs::rename(&replacement, &primary).unwrap();

        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![added.clone(), legacy.clone()]
        );
        assert_eq!(
            cache.probe_count, 3,
            "a replacement at the same path must be probed"
        );

        std::fs::remove_file(&added).unwrap();
        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![legacy.clone()]
        );
        assert_eq!(cache.probe_count, 3);
        assert!(!cache.candidates.contains_key(&added));
        assert_eq!(
            cache.session_db_paths_from_home(home.path()),
            vec![legacy],
            "legacy state_5.sqlite remains a candidate regardless of schema"
        );
    }

    #[test]
    fn strips_trailing_suffix_from_model_names() {
        assert_eq!(
            sanitize_model_suffixes_in_text("model=deepseek-v4-flash[1M]"),
            "model=deepseek-v4-flash"
        );
        assert_eq!(
            sanitize_model_suffixes_in_text("nvidia/nemotron-3-super-120b-a12b:free[1M]"),
            "nvidia/nemotron-3-super-120b-a12b:free"
        );
        assert_eq!(sanitize_model_suffixes_in_text("glm-5.2[1M]"), "glm-5.2");
    }

    #[test]
    fn leaves_non_model_brackets_unchanged() {
        assert_eq!(
            sanitize_model_suffixes_in_text("array[0] and foo[bar]"),
            "array[0] and foo[bar]"
        );
        assert_eq!(
            sanitize_model_suffixes_in_text("some [placeholder] text"),
            "some [placeholder] text"
        );
    }

    #[test]
    fn leaves_text_without_brackets_unchanged() {
        let text = "no suffix here";
        assert_eq!(sanitize_model_suffixes_in_text(text), text);
    }

    #[test]
    fn thread_cleanup_reports_incompatible_rows_instead_of_skipping_them() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("threads.sqlite");
        let db = Connection::open(&db_path).unwrap();
        db.execute("CREATE TABLE threads (id INTEGER, model TEXT)", [])
            .unwrap();
        db.execute(
            "INSERT INTO threads (id, model) VALUES (1, 'model[1M]')",
            [],
        )
        .unwrap();
        drop(db);

        let error = sanitize_thread_model_suffixes_in_db(&db_path).unwrap_err();
        assert!(error.downcast_ref::<rusqlite::Error>().is_some());
    }

    #[test]
    fn log_cleanup_reports_incompatible_rows_instead_of_skipping_them() {
        let home = tempdir().unwrap();
        let db = Connection::open(home.path().join("logs_2.sqlite")).unwrap();
        db.execute("CREATE TABLE logs (feedback_log_body BLOB)", [])
            .unwrap();
        db.execute(
            "INSERT INTO logs (feedback_log_body) VALUES (x'5b314d5d')",
            [],
        )
        .unwrap();
        drop(db);

        let error = sanitize_logs_model_suffixes(home.path()).unwrap_err();
        assert!(error.downcast_ref::<rusqlite::Error>().is_some());
    }
}
