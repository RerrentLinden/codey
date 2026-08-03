use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use base64::Engine;
use codey_runtime_core::codex_sqlite::codex_session_db_paths_from_home;
use codey_runtime_core::models::SessionRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, types::ValueRef};
use serde_json::{Map, Value, json};

use crate::sqlite_util::table_columns;

const FALLBACK_SESSION_NAME: &str = "未命名会话";
const MAX_SESSION_NAME_CHARS: usize = 80;
const MAX_THREAD_SORT_KEYS: usize = 200;
const THREAD_TIMESTAMP_COLUMNS: [&str; 6] = [
    "updated_at",
    "updated_at_ms",
    "created_at",
    "created_at_ms",
    "recency_at",
    "recency_at_ms",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct DatabaseSignature {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    created_at: u64,
    #[cfg(not(any(unix, windows)))]
    created_at: Option<SystemTime>,
}

#[derive(Debug)]
struct CachedConnection {
    signature: DatabaseSignature,
    connection: Connection,
}

#[derive(Debug, Default)]
pub(crate) struct SessionMetadataCache {
    connections: HashMap<PathBuf, CachedConnection>,
    #[cfg(test)]
    database_open_count: usize,
}

/// 会话 ID 归一：允许 `local:` 前缀与两侧空白。此前 5 处各自实现，trim
/// 顺序互不一致。
pub(crate) fn normalize_session_id(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed.strip_prefix("local:").unwrap_or(trimmed).trim()
}

#[cfg(test)]
pub fn thread_sort_keys(home: &Path, sessions: &[SessionRef]) -> Value {
    SessionMetadataCache::default().thread_sort_keys(home, sessions)
}

impl SessionMetadataCache {
    pub(crate) fn thread_sort_keys(&mut self, home: &Path, sessions: &[SessionRef]) -> Value {
        let sessions = sessions
            .iter()
            .filter(|session| !session.session_id.trim().is_empty())
            .take(MAX_THREAD_SORT_KEYS)
            .cloned()
            .collect::<Vec<_>>();
        if sessions.is_empty() {
            return json!({"status": "ok", "sort_keys": []});
        }

        let mut latest_by_session = HashMap::<String, Value>::new();
        for path in self.active_database_paths(home) {
            if !self.ensure_connection(&path) {
                continue;
            }
            let result = {
                let connection = &self
                    .connections
                    .get(&path)
                    .expect("connection was inserted above")
                    .connection;
                thread_sort_keys_from_connection(connection, &sessions)
            };
            let Ok(sort_keys) = result else {
                self.connections.remove(&path);
                continue;
            };
            for sort_key in sort_keys {
                let Some(session_id) = sort_key.get("session_id").and_then(Value::as_str) else {
                    continue;
                };
                let should_replace = latest_by_session
                    .get(session_id)
                    .is_none_or(|current| timestamp_ms(&sort_key) > timestamp_ms(current));
                if should_replace {
                    latest_by_session.insert(session_id.to_string(), sort_key);
                }
            }
        }

        let sort_keys = sessions
            .iter()
            .filter_map(|session| {
                latest_by_session.remove(normalize_session_id(&session.session_id))
            })
            .collect::<Vec<_>>();
        json!({"status": "ok", "sort_keys": sort_keys})
    }

    pub(crate) fn resolve_session_name_with_preferred(
        &mut self,
        home: &Path,
        session_id: &str,
        preferred_title: Option<&str>,
    ) -> String {
        let session_id = normalize_session_id(session_id);
        if session_id.is_empty() {
            return FALLBACK_SESSION_NAME.to_string();
        }

        let preferred_title = preferred_title
            .map(clean_session_name)
            .filter(|title| !title.is_empty());
        let mut found_metadata = false;
        for path in self.active_database_paths(home) {
            if !self.ensure_connection(&path) {
                continue;
            }
            let row = {
                let connection = &self
                    .connections
                    .get(&path)
                    .expect("connection was inserted above")
                    .connection;
                session_name_row(connection, session_id)
            };
            let (title, first_user_message, preview) = match row {
                Ok(Some(row)) => row,
                Ok(None) => continue,
                Err(_) => {
                    self.connections.remove(&path);
                    continue;
                }
            };
            found_metadata = true;
            let first_user_message = first_user_message
                .as_deref()
                .map(clean_session_name)
                .unwrap_or_default();
            let preview = preview
                .as_deref()
                .map(clean_session_name)
                .unwrap_or_default();
            if let Some(preferred_title) = preferred_title.as_ref()
                && !is_placeholder_title(preferred_title, &first_user_message, &preview)
            {
                return preferred_title.clone();
            }
            let title = title.as_deref().map(clean_session_name).unwrap_or_default();
            if !title.is_empty() && !is_placeholder_title(&title, &first_user_message, &preview) {
                return title;
            }
        }
        if !found_metadata && let Some(preferred_title) = preferred_title {
            return preferred_title;
        }
        FALLBACK_SESSION_NAME.to_string()
    }

    fn active_database_paths(&mut self, home: &Path) -> Vec<PathBuf> {
        let paths = codex_session_db_paths_from_home(home)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        let active = paths.iter().cloned().collect::<HashSet<_>>();
        self.connections.retain(|path, _| active.contains(path));
        paths
    }

    fn ensure_connection(&mut self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            self.connections.remove(path);
            return false;
        };
        let signature = database_signature(&metadata);
        let is_current = self
            .connections
            .get(path)
            .is_some_and(|cached| cached.signature == signature);
        if !is_current {
            self.connections.remove(path);
        }
        if self.connections.contains_key(path) {
            return true;
        }
        let Ok(connection) = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return false;
        };
        if connection.busy_timeout(Duration::from_millis(250)).is_err() {
            return false;
        }
        #[cfg(test)]
        {
            self.database_open_count += 1;
        }
        self.connections.insert(
            path.to_path_buf(),
            CachedConnection {
                signature,
                connection,
            },
        );
        true
    }
}

fn database_signature(metadata: &fs::Metadata) -> DatabaseSignature {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        DatabaseSignature {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        DatabaseSignature {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created_at: metadata.creation_time(),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        DatabaseSignature {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created_at: metadata.created().ok(),
        }
    }
}

fn thread_sort_keys_from_connection(
    connection: &Connection,
    sessions: &[SessionRef],
) -> Result<Vec<Value>> {
    let columns = table_columns(connection, "threads")?;
    if !columns.contains("id") {
        return Ok(Vec::new());
    }
    let mut selected_columns = vec!["id"];
    selected_columns.extend(
        THREAD_TIMESTAMP_COLUMNS
            .iter()
            .copied()
            .filter(|column| columns.contains(*column)),
    );
    let sql = format!(
        "SELECT {} FROM threads WHERE id = ?1",
        selected_columns.join(", ")
    );
    let mut statement = connection.prepare(&sql)?;
    let mut seen = HashSet::new();
    let mut sort_keys = Vec::new();
    for session in sessions {
        let thread_id = normalize_session_id(&session.session_id);
        if thread_id.is_empty() || !seen.insert(thread_id.to_string()) {
            continue;
        }
        let row = statement
            .query_row([thread_id], |row| {
                let mut values = Map::new();
                for (index, column) in selected_columns.iter().enumerate() {
                    values.insert(
                        (*column).to_string(),
                        sql_value_to_json(row.get_ref(index)?),
                    );
                }
                Ok(values)
            })
            .optional()?;
        let Some(row) = row else {
            continue;
        };
        let mut payload = Map::new();
        for column in THREAD_TIMESTAMP_COLUMNS {
            payload.insert(
                column.to_string(),
                row.get(column).cloned().unwrap_or(Value::Null),
            );
        }
        payload.insert(
            "session_id".to_string(),
            Value::String(thread_id.to_string()),
        );
        sort_keys.push(Value::Object(payload));
    }
    Ok(sort_keys)
}

fn sql_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => json!(String::from_utf8_lossy(value).to_string()),
        ValueRef::Blob(value) => {
            json!(base64::engine::general_purpose::STANDARD.encode(value))
        }
    }
}

fn timestamp_ms(payload: &Value) -> i64 {
    payload
        .get("recency_at_ms")
        .and_then(json_i64)
        .or_else(|| {
            payload
                .get("recency_at")
                .and_then(json_i64)
                .map(|seconds| seconds.saturating_mul(1_000))
        })
        .or_else(|| payload.get("created_at_ms").and_then(json_i64))
        .or_else(|| {
            payload
                .get("created_at")
                .and_then(json_i64)
                .map(|seconds| seconds.saturating_mul(1_000))
        })
        .or_else(|| payload.get("updated_at_ms").and_then(json_i64))
        .or_else(|| {
            payload
                .get("updated_at")
                .and_then(json_i64)
                .map(|seconds| seconds.saturating_mul(1_000))
        })
        .unwrap_or_default()
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
pub fn resolve_session_name_with_preferred(
    home: &Path,
    session_id: &str,
    preferred_title: Option<&str>,
) -> String {
    SessionMetadataCache::default().resolve_session_name_with_preferred(
        home,
        session_id,
        preferred_title,
    )
}

type SessionNameRow = (Option<String>, Option<String>, Option<String>);

fn session_name_row(connection: &Connection, session_id: &str) -> Result<Option<SessionNameRow>> {
    let columns = table_columns(connection, "threads")?;
    if !["id", "title", "first_user_message", "preview"]
        .iter()
        .all(|column| columns.contains(*column))
    {
        return Ok(None);
    }
    Ok(connection
        .query_row(
            "SELECT title, first_user_message, preview FROM threads WHERE id=?1 LIMIT 1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?)
}

fn is_placeholder_title(title: &str, first_user_message: &str, preview: &str) -> bool {
    (!first_user_message.is_empty() && title == first_user_message)
        || (!preview.is_empty() && title == preview)
}

fn clean_session_name(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = normalized.chars();
    let truncated = characters
        .by_ref()
        .take(MAX_SESSION_NAME_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codey_runtime_core::models::SessionRef;
    use rusqlite::params;
    use tempfile::tempdir;

    fn create_thread_database(path: &Path, timestamp_column: &str, rows: &[(&str, i64)]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                &format!(
                    "CREATE TABLE threads (
                        id TEXT PRIMARY KEY,
                        title TEXT NOT NULL,
                        rollout_path TEXT NOT NULL,
                        {timestamp_column} INTEGER
                    )"
                ),
                [],
            )
            .unwrap();
        for (id, timestamp) in rows {
            connection
                .execute(
                    &format!(
                        "INSERT INTO threads (id, title, rollout_path, {timestamp_column})
                         VALUES (?1, ?2, ?3, ?4)"
                    ),
                    params![
                        id,
                        format!("Title {id}"),
                        format!("/tmp/{id}.jsonl"),
                        timestamp
                    ],
                )
                .unwrap();
        }
    }

    #[test]
    fn returns_latest_thread_sort_keys_across_codex_databases() {
        let home = tempdir().unwrap();
        create_thread_database(
            &home.path().join("sqlite/codex-dev.db"),
            "updated_at_ms",
            &[("thread-1", 3_600_000), ("thread-2", 7_200_000)],
        );
        create_thread_database(
            &home.path().join("state_5.sqlite"),
            "updated_at",
            &[("thread-1", 10_800)],
        );
        let sessions = vec![
            SessionRef::new("local:thread-1", "One").unwrap(),
            SessionRef::new("thread-2", "Two").unwrap(),
            SessionRef::new("missing", "Missing").unwrap(),
        ];

        assert_eq!(
            thread_sort_keys(home.path(), &sessions),
            json!({
                "status": "ok",
                "sort_keys": [
                    {
                        "session_id": "thread-1",
                        "updated_at": 10_800,
                        "updated_at_ms": null,
                        "created_at": null,
                        "created_at_ms": null,
                        "recency_at": null,
                        "recency_at_ms": null
                    },
                    {
                        "session_id": "thread-2",
                        "updated_at": null,
                        "updated_at_ms": 7_200_000,
                        "created_at": null,
                        "created_at_ms": null,
                        "recency_at": null,
                        "recency_at_ms": null
                    }
                ]
            })
        );
    }

    #[test]
    fn metadata_cache_reuses_unchanged_database_connections() {
        let home = tempdir().unwrap();
        create_thread_database(
            &home.path().join("state_5.sqlite"),
            "updated_at_ms",
            &[("thread-1", 3_600_000)],
        );
        let sessions = vec![SessionRef::new("thread-1", "One").unwrap()];
        let mut cache = SessionMetadataCache::default();

        assert_eq!(
            cache.thread_sort_keys(home.path(), &sessions)["sort_keys"][0]["updated_at_ms"],
            3_600_000
        );
        assert_eq!(
            cache.thread_sort_keys(home.path(), &sessions)["sort_keys"][0]["updated_at_ms"],
            3_600_000
        );
        assert_eq!(cache.database_open_count, 1);
    }

    #[test]
    fn thread_sort_keys_supports_a_minimal_threads_schema() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    updated_at_ms INTEGER
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, updated_at_ms) VALUES (?1, ?2)",
                params!["thread-1", 3_600_000],
            )
            .unwrap();
        drop(connection);
        let sessions = vec![SessionRef::new("thread-1", "One").unwrap()];

        assert_eq!(
            thread_sort_keys(home.path(), &sessions)["sort_keys"][0]["updated_at_ms"],
            3_600_000
        );
    }

    #[cfg(unix)]
    #[test]
    fn metadata_cache_reopens_a_replaced_database_with_the_same_size_and_mtime() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        create_thread_database(&path, "updated_at_ms", &[("thread-1", 3_600_000)]);
        let sessions = vec![SessionRef::new("thread-1", "One").unwrap()];
        let mut cache = SessionMetadataCache::default();
        assert_eq!(
            cache.thread_sort_keys(home.path(), &sessions)["sort_keys"][0]["updated_at_ms"],
            3_600_000
        );

        let original_metadata = fs::metadata(&path).unwrap();
        let original_modified = original_metadata.modified().unwrap();
        let replacement = home.path().join("replacement.sqlite");
        create_thread_database(&replacement, "updated_at_ms", &[("thread-1", 7_200_000)]);
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .unwrap();
        let replacement_metadata = fs::metadata(&replacement).unwrap();
        assert_eq!(replacement_metadata.len(), original_metadata.len());
        assert_eq!(replacement_metadata.modified().unwrap(), original_modified);
        fs::rename(replacement, &path).unwrap();

        assert_eq!(
            cache.thread_sort_keys(home.path(), &sessions)["sort_keys"][0]["updated_at_ms"],
            7_200_000
        );
        assert_eq!(cache.database_open_count, 2);
    }

    #[test]
    fn timestamp_selection_ignores_open_refreshed_updated_at_when_stable_fields_exist() {
        assert_eq!(
            timestamp_ms(&json!({
                "recency_at_ms": 3_600_000,
                "created_at_ms": 3_000_000,
                "updated_at_ms": 99_999_000
            })),
            3_600_000
        );
        assert_eq!(
            timestamp_ms(&json!({
                "created_at_ms": 3_000_000,
                "updated_at_ms": 99_999_000
            })),
            3_000_000
        );
    }

    #[test]
    fn resolves_the_saved_thread_title() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    first_user_message TEXT,
                    preview TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, title, first_user_message, preview)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "thread-1",
                    "  发布版本计划  ",
                    "请帮我发布版本",
                    "请帮我发布版本"
                ],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            resolve_session_name_with_preferred(home.path(), "local:thread-1", None),
            "发布版本计划"
        );
    }

    #[test]
    fn never_uses_the_first_user_message_as_the_title() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    first_user_message TEXT,
                    preview TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, title, first_user_message, preview)
                 VALUES (?1, ?2, ?3, ?3)",
                params![
                    "thread-2",
                    "请帮我\n检查  飞书通知",
                    "请帮我\n检查  飞书通知"
                ],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            resolve_session_name_with_preferred(home.path(), "thread-2", None),
            FALLBACK_SESSION_NAME
        );
        assert_eq!(
            resolve_session_name_with_preferred(home.path(), "missing", None),
            FALLBACK_SESSION_NAME
        );
    }

    #[test]
    fn prefers_the_codex_sidebar_title() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    first_user_message TEXT,
                    preview TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, title, first_user_message, preview)
                 VALUES (?1, ?2, ?2, ?2)",
                params!["thread-3", "为什么飞书标题不对"],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            resolve_session_name_with_preferred(
                home.path(),
                "local:thread-3",
                Some("  修复飞书会话标题  ")
            ),
            "修复飞书会话标题"
        );
    }

    #[test]
    fn rejects_a_sidebar_title_that_is_still_the_first_message() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    first_user_message TEXT,
                    preview TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, title, first_user_message, preview)
                 VALUES (?1, ?2, ?2, ?2)",
                params!["thread-4", "帮我处理这个问题"],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            resolve_session_name_with_preferred(home.path(), "thread-4", Some("帮我处理这个问题")),
            FALLBACK_SESSION_NAME
        );
    }
}
