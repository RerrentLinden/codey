use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use codey_runtime_core::codex_sqlite::codex_session_db_paths_from_home;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::sqlite_util::table_columns;

const FALLBACK_SESSION_NAME: &str = "未命名会话";
const MAX_SESSION_NAME_CHARS: usize = 80;

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
}

/// 会话 ID 归一：允许 `local:` 前缀与两侧空白。此前 5 处各自实现，trim
/// 顺序互不一致。
pub(crate) fn normalize_session_id(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed.strip_prefix("local:").unwrap_or(trimmed).trim()
}

impl SessionMetadataCache {
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
    use rusqlite::params;
    use tempfile::tempdir;

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
