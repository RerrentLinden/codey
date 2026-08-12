use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result};
use codey_runtime_core::codex_sqlite::codex_session_db_paths_from_home;
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const TOMBSTONE_VERSION: u32 = 1;
const TOMBSTONE_DIR: &str = ".codey-message-delete-tombstones-v1";
const TOMBSTONE_LOCK_FILE: &str = ".codey-message-delete-tombstones-v1.lock";
static TOMBSTONE_LOCK: Mutex<()> = Mutex::new(());
type PendingDeleteTombstones = BTreeMap<String, (BTreeSet<String>, Vec<PathBuf>)>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDeleteResult {
    pub deleted: usize,
    pub unsupported_databases: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct MessageDeleteReplaySummary {
    pub deleted: usize,
    pub cleared_sessions: usize,
    pub failures: Vec<(String, String)>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageDeleteTombstone {
    version: u32,
    session_id: String,
    message_id: String,
}

struct MessageDeleteLock {
    _file: fs::File,
    _thread_guard: MutexGuard<'static, ()>,
}

pub fn delete_messages_persistently(
    home: &Path,
    session_id: &str,
    message_ids: &[String],
) -> Result<MessageDeleteResult> {
    let session_id = crate::session_metadata::normalize_session_id(session_id)
        .trim()
        .to_string();
    let message_ids = message_ids
        .iter()
        .map(|message_id| normalize_message_id(message_id))
        .filter(|message_id| !message_id.is_empty())
        .collect::<BTreeSet<_>>();
    if session_id.is_empty() || message_ids.is_empty() {
        anyhow::bail!("session_id 和 message_ids 不能为空");
    }

    // Persist the intent before touching the rollout. A loaded Codex thread
    // can flush stale history after this request returns; startup maintenance
    // replays the tombstone after that writer has been stopped.
    let _guard = lock_message_deletes(home)?;
    record_delete_tombstones_unlocked(home, &session_id, &message_ids)?;
    delete_messages(
        home,
        &session_id,
        &message_ids.into_iter().collect::<Vec<_>>(),
    )
}

pub(crate) fn reapply_persisted_deletions(home: &Path) -> Result<MessageDeleteReplaySummary> {
    let _guard = lock_message_deletes(home)?;
    let pending = read_delete_tombstones_unlocked(home)?;
    let mut summary = MessageDeleteReplaySummary::default();

    for (session_id, (message_ids, paths)) in pending {
        let message_ids = message_ids.into_iter().collect::<Vec<_>>();
        match delete_messages(home, &session_id, &message_ids) {
            Ok(result) if result.unsupported_databases.is_empty() => {
                summary.deleted += result.deleted;
                match remove_delete_tombstones(&paths) {
                    Ok(()) => summary.cleared_sessions += 1,
                    Err(error) => summary.failures.push((session_id, format!("{error:#}"))),
                }
            }
            Ok(result) => summary.failures.push((
                session_id,
                format!(
                    "无法确认消息删除是否已落地；{} 个数据库结构不受支持",
                    result.unsupported_databases.len()
                ),
            )),
            Err(error) => summary.failures.push((session_id, format!("{error:#}"))),
        }
    }

    Ok(summary)
}

fn lock_message_deletes(home: &Path) -> Result<MessageDeleteLock> {
    let thread_guard = TOMBSTONE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("消息删除墓碑锁已损坏"))?;
    fs::create_dir_all(home)
        .with_context(|| format!("创建 Codex 数据目录失败：{}", home.display()))?;
    let path = home.join(TOMBSTONE_LOCK_FILE);
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("打开消息删除墓碑锁失败：{}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("获取消息删除墓碑锁失败：{}", path.display()))?;
    Ok(MessageDeleteLock {
        _file: file,
        _thread_guard: thread_guard,
    })
}

fn record_delete_tombstones_unlocked(
    home: &Path,
    session_id: &str,
    message_ids: &BTreeSet<String>,
) -> Result<()> {
    let directory = tombstone_directory(home);
    create_private_directory(&directory)?;
    for message_id in message_ids {
        let tombstone = MessageDeleteTombstone {
            version: TOMBSTONE_VERSION,
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
        };
        let path = tombstone_path(&directory, session_id, message_id);
        if path.exists() {
            let existing: MessageDeleteTombstone = serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("解析消息删除墓碑失败：{}", path.display()))?;
            if existing != tombstone {
                anyhow::bail!("消息删除墓碑哈希冲突：{}", path.display());
            }
            continue;
        }
        let temp = crate::fs_util::unique_temp_path(&path);
        let bytes = serde_json::to_vec(&tombstone)?;
        if let Err(error) = write_private_file(&temp, &bytes) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        match fs::rename(&temp, &path) {
            Ok(()) => {}
            Err(_error) if path.exists() => {
                let _ = fs::remove_file(&temp);
                let existing = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                if existing != Some(tombstone) {
                    anyhow::bail!("消息删除墓碑哈希冲突：{}", path.display());
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error)
                    .with_context(|| format!("保存消息删除墓碑失败：{}", path.display()));
            }
        }
    }
    sync_directory(&directory)
}

fn read_delete_tombstones_unlocked(home: &Path) -> Result<PendingDeleteTombstones> {
    let directory = tombstone_directory(home);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取消息删除墓碑目录失败：{}", directory.display()));
        }
    };
    let mut pending = BTreeMap::<String, (BTreeSet<String>, Vec<PathBuf>)>::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let tombstone: MessageDeleteTombstone = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("解析消息删除墓碑失败：{}", path.display()))?;
        if tombstone.version != TOMBSTONE_VERSION
            || tombstone.session_id.trim().is_empty()
            || tombstone.message_id.trim().is_empty()
        {
            anyhow::bail!("无效的消息删除墓碑：{}", path.display());
        }
        let message_id = normalize_message_id(&tombstone.message_id);
        if message_id.is_empty() {
            anyhow::bail!("无效的消息删除墓碑：{}", path.display());
        }
        let (message_ids, paths) = pending.entry(tombstone.session_id).or_default();
        message_ids.insert(message_id);
        paths.push(path);
    }
    Ok(pending)
}

fn remove_delete_tombstones(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("清理消息删除墓碑失败：{}", path.display()));
            }
        }
    }
    if let Some(directory) = paths.first().and_then(|path| path.parent()) {
        sync_directory(directory)?;
    }
    Ok(())
}

fn tombstone_directory(home: &Path) -> PathBuf {
    home.join(TOMBSTONE_DIR)
}

fn tombstone_path(directory: &Path, session_id: &str, message_id: &str) -> PathBuf {
    let digest = Sha256::digest(format!("{session_id}\0{message_id}").as_bytes());
    directory.join(format!("{digest:x}.json"))
}

fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("创建消息删除墓碑目录失败：{}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("创建消息删除墓碑临时文件失败：{}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("写入消息删除墓碑失败：{}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("同步消息删除墓碑失败：{}", path.display()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn delete_messages(
    home: &Path,
    session_id: &str,
    message_ids: &[String],
) -> Result<MessageDeleteResult> {
    if session_id.trim().is_empty() || message_ids.is_empty() {
        anyhow::bail!("session_id 和 message_ids 不能为空");
    }
    let session_id = crate::session_metadata::normalize_session_id(session_id);
    let message_ids = message_ids
        .iter()
        .map(|message_id| normalize_message_id(message_id))
        .filter(|message_id| !message_id.is_empty())
        .collect::<Vec<_>>();
    if message_ids.is_empty() {
        anyhow::bail!("message_ids 不能为空");
    }
    let mut result = MessageDeleteResult {
        deleted: 0,
        unsupported_databases: Vec::new(),
    };
    if let Some(rollout_path) = find_rollout_path(home, session_id)? {
        let selected = message_ids.iter().cloned().collect::<HashSet<_>>();
        result.deleted = delete_turns_from_rollout(home, &rollout_path, &selected)?;
        return Ok(result);
    }

    // Compatibility path for older Codex builds that stored individual
    // messages in SQLite instead of turn blocks in a rollout JSONL file.
    for db_path in codex_session_db_paths_from_home(home) {
        if !db_path.exists() {
            continue;
        }
        let Some(targets) = find_message_targets(&db_path)? else {
            result
                .unsupported_databases
                .push(db_path.to_string_lossy().to_string());
            continue;
        };
        let deleted = delete_from_db(&db_path, &targets, session_id, &message_ids)?;
        result.deleted += deleted;
    }
    Ok(result)
}

fn normalize_message_id(value: &str) -> String {
    let value = value.trim();
    value
        .rsplit_once(":turn:")
        .map(|(_, turn_id)| turn_id.trim())
        .unwrap_or(value)
        .to_string()
}

fn find_rollout_path(home: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    for db_path in codex_session_db_paths_from_home(home) {
        if !db_path.exists() {
            continue;
        }
        let connection = Connection::open(&db_path)?;
        let has_rollout_path = connection
            .query_row(
                "SELECT 1 FROM pragma_table_info('threads') WHERE name='rollout_path' LIMIT 1",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !has_rollout_path {
            continue;
        }
        let path = connection
            .query_row(
                "SELECT rollout_path FROM threads WHERE id=?1 LIMIT 1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(path) = path {
            let path = PathBuf::from(path);
            return Ok(Some(if path.is_absolute() {
                path
            } else {
                home.join(path)
            }));
        }
    }
    Ok(None)
}

fn delete_turns_from_rollout(
    home: &Path,
    rollout_path: &Path,
    selected: &HashSet<String>,
) -> Result<usize> {
    let canonical_home = home
        .canonicalize()
        .with_context(|| format!("找不到 Codex 数据目录：{}", home.display()))?;
    let canonical_rollout = rollout_path
        .canonicalize()
        .with_context(|| format!("找不到会话记录：{}", rollout_path.display()))?;
    if !canonical_rollout.starts_with(&canonical_home) {
        anyhow::bail!("会话记录不在 Codex 数据目录内，已拒绝修改");
    }

    let original = fs::read_to_string(&canonical_rollout)
        .with_context(|| format!("读取会话记录失败：{}", canonical_rollout.display()))?;
    let mut output = String::with_capacity(original.len());
    let mut removing_turn = false;
    let mut selected_turn_seen = false;
    let mut found = HashSet::new();
    for line in original.split_inclusive('\n') {
        let json_line = line.trim_end_matches(['\r', '\n']);
        if let Some(turn_id) = turn_boundary_id(json_line) {
            removing_turn = selected.contains(&turn_id);
            if removing_turn {
                selected_turn_seen = true;
                found.insert(turn_id);
            }
        }
        // A later compaction snapshot may contain the deleted turn inside its
        // encrypted summary.  It cannot be edited safely, so discard the
        // snapshot and let Codex rebuild history from the remaining rollout.
        if selected_turn_seen && is_compacted_summary(json_line) {
            continue;
        }
        if !removing_turn {
            output.push_str(line);
        }
    }
    if found.is_empty() {
        return Ok(0);
    }

    rewrite_in_place(&canonical_rollout, output.as_bytes())
        .with_context(|| format!("写回会话记录失败：{}", canonical_rollout.display()))?;
    Ok(found.len())
}

fn is_compacted_summary(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    if value.get("type").and_then(Value::as_str) != Some("compacted") {
        return false;
    }
    value
        .get("payload")
        .and_then(|payload| payload.get("replacement_history"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
        })
}

fn turn_boundary_id(line: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let payload = value.get("payload")?;
    let record_type = value.get("type").and_then(Value::as_str);
    let is_boundary = record_type == Some("turn_context")
        || (record_type == Some("event_msg")
            && payload.get("type").and_then(Value::as_str) == Some("task_started"));
    is_boundary
        .then(|| payload.get("turn_id").and_then(Value::as_str))
        .flatten()
        .map(str::trim)
        .filter(|turn_id| !turn_id.is_empty())
        .map(str::to_string)
}

fn rewrite_in_place(destination: &Path, contents: &[u8]) -> std::io::Result<()> {
    // Codex keeps rollout files open in append mode. Replacing the path would
    // leave that writer attached to an unlinked inode, so preserve the file
    // identity while updating its contents.
    let mut file = fs::OpenOptions::new().write(true).open(destination)?;
    file.set_len(0)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn table_columns(path: &Path, table: &str) -> Result<std::collections::HashSet<String>> {
    let connection = Connection::open(path)?;
    Ok(crate::sqlite_util::table_columns(&connection, table)?)
}

#[derive(Debug, Clone)]
struct MessageTarget {
    table: &'static str,
    id_column: &'static str,
    session_column: &'static str,
}

fn find_message_targets(path: &Path) -> Result<Option<Vec<MessageTarget>>> {
    let mut targets = Vec::new();
    for table in ["messages", "thread_items", "items"] {
        let columns = table_columns(path, table)?;
        if columns.is_empty() {
            continue;
        }
        let Some(id_column) = ["id", "message_id", "item_id"]
            .into_iter()
            .find(|candidate| columns.contains(*candidate))
        else {
            continue;
        };
        let Some(session_column) = ["session_id", "thread_id"]
            .into_iter()
            .find(|candidate| columns.contains(*candidate))
        else {
            continue;
        };
        targets.push(MessageTarget {
            table,
            id_column,
            session_column,
        });
    }
    Ok((!targets.is_empty()).then_some(targets))
}

fn delete_from_db(
    path: &Path,
    targets: &[MessageTarget],
    session_id: &str,
    message_ids: &[String],
) -> Result<usize> {
    let mut connection = Connection::open(path)?;
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let mut values = message_ids.to_vec();
    values.push(session_id.to_string());
    let transaction = connection.transaction()?;
    let mut deleted = 0;
    for target in targets {
        let sql = format!(
            "DELETE FROM {} WHERE {} IN ({placeholders}) AND {} = ?",
            target.table, target.id_column, target.session_column
        );
        deleted += transaction.execute(&sql, params_from_iter(values.iter()))?;
    }
    transaction.commit()?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::tempdir;

    #[test]
    fn deletes_messages_transactionally_without_a_backup() {
        let home = tempdir().unwrap();
        let path = home.path().join("state_5.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, body TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (id, session_id, body) VALUES (?1, ?2, ?3), (?4, ?2, ?5)",
                params!["m1", "s1", "one", "m2", "two"],
            )
            .unwrap();
        drop(connection);

        let result = delete_messages(home.path(), "s1", &["m1".into()]).unwrap();
        assert_eq!(result.deleted, 1);
        let connection = Connection::open(path).unwrap();
        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn deletes_a_current_codex_turn_from_its_rollout() {
        let home = tempdir().unwrap();
        let rollout_dir = home.path().join("sessions/2026/07/16");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join("rollout-test.jsonl");
        let original = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t1\"}}\n",
            "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t1\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t1\"}}\n",
            "{\"type\":\"compacted\",\"payload\":{\"replacement_history\":[{\"type\":\"message\",\"role\":\"user\",\"content\":[]},{\"type\":\"compaction\",\"id\":\"cmp_1\",\"encrypted_content\":\"old\"}]}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t2\"}}\n",
            "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t2\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t2\"}}\n",
        );
        fs::write(&rollout, original).unwrap();
        let db = home.path().join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params!["s1", rollout.to_string_lossy().to_string()],
            )
            .unwrap();
        drop(connection);

        let mut live_writer = fs::OpenOptions::new().append(true).open(&rollout).unwrap();
        let result =
            delete_messages(home.path(), "local:s1", &["history-content:turn:t1".into()]).unwrap();
        live_writer
            .write_all(
                b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t3\"}}\n",
            )
            .unwrap();
        live_writer.sync_all().unwrap();
        assert_eq!(result.deleted, 1);
        let remaining = fs::read_to_string(&rollout).unwrap();
        assert!(remaining.contains("session_meta"));
        assert!(!remaining.contains("t1"));
        assert!(!remaining.contains("cmp_1"));
        assert!(remaining.contains("t2"));
        assert!(remaining.contains("t3"));
    }

    #[test]
    fn deletes_a_turn_context_when_task_started_is_absent() {
        let home = tempdir().unwrap();
        let rollout_dir = home.path().join("sessions/2026/08/12");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join("rollout-turn-context.jsonl");
        fs::write(
            &rollout,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\"}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t1\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"remove context-only turn\"}]}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t2\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\"}}\n",
            ),
        )
        .unwrap();
        let db = home.path().join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params!["s1", rollout.to_string_lossy().to_string()],
            )
            .unwrap();
        drop(connection);

        let result = delete_messages(home.path(), "s1", &["t1".into()]).unwrap();

        assert_eq!(result.deleted, 1);
        let remaining = fs::read_to_string(&rollout).unwrap();
        assert!(!remaining.contains("t1"));
        assert!(!remaining.contains("remove context-only turn"));
        assert!(remaining.contains("t2"));
    }

    #[test]
    fn reapplies_hard_delete_after_a_loaded_thread_flushes_stale_history() {
        let home = tempdir().unwrap();
        let rollout_dir = home.path().join("sessions/2026/07/20");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join("rollout-test.jsonl");
        let deleted_turn = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t1\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"remove permanently\"}]}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t1\"}}\n",
        );
        let retained_turn = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t2\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t2\"}}\n",
        );
        fs::write(
            &rollout,
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s1\"}}}}\n{deleted_turn}{retained_turn}"),
        )
        .unwrap();
        let db = home.path().join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params!["s1", rollout.to_string_lossy().to_string()],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            delete_messages(home.path(), "s1", &["t1".into()])
                .unwrap()
                .deleted,
            1
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(deleted_turn.as_bytes())
            .unwrap();

        assert_eq!(
            delete_messages(home.path(), "s1", &["t1".into()])
                .unwrap()
                .deleted,
            1
        );
        let remaining = fs::read_to_string(&rollout).unwrap();
        assert!(!remaining.contains("t1"));
        assert!(!remaining.contains("remove permanently"));
        assert!(remaining.contains("t2"));
    }

    #[test]
    fn startup_replays_a_persisted_delete_after_stale_history_is_flushed() {
        let home = tempdir().unwrap();
        let rollout_dir = home.path().join("sessions/2026/08/12");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join("rollout-persisted-delete.jsonl");
        let deleted_turn = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t1\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"must stay deleted\"}]}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t1\"}}\n",
        );
        let retained_turn = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t2\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t2\"}}\n",
        );
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s1\"}}}}\n{deleted_turn}{retained_turn}"
            ),
        )
        .unwrap();
        let db = home.path().join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params!["s1", rollout.to_string_lossy().to_string()],
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            delete_messages_persistently(home.path(), " local:s1 ", &[" t1 ".into()])
                .unwrap()
                .deleted,
            1
        );
        let tombstone_path = tombstone_path(&tombstone_directory(home.path()), "s1", "t1");
        let tombstone: MessageDeleteTombstone =
            serde_json::from_slice(&fs::read(&tombstone_path).unwrap()).unwrap();
        assert_eq!(tombstone.session_id, "s1");
        assert_eq!(tombstone.message_id, "t1");

        fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(deleted_turn.as_bytes())
            .unwrap();

        let replay = reapply_persisted_deletions(home.path()).unwrap();

        assert_eq!(replay.deleted, 1);
        assert_eq!(replay.cleared_sessions, 1);
        assert!(replay.failures.is_empty());
        assert!(!tombstone_path.exists());
        let remaining = fs::read_to_string(&rollout).unwrap();
        assert!(!remaining.contains("t1"));
        assert!(!remaining.contains("must stay deleted"));
        assert!(remaining.contains("t2"));
    }

    #[test]
    fn startup_normalizes_and_consumes_an_old_prefixed_tombstone() {
        let home = tempdir().unwrap();
        let rollout_dir = home.path().join("sessions/2026/08/12");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join("rollout-prefixed-tombstone.jsonl");
        fs::write(
            &rollout,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t1\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t2\"}}\n",
            ),
        )
        .unwrap();
        let db = home.path().join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params!["s1", rollout.to_string_lossy().to_string()],
            )
            .unwrap();
        drop(connection);

        let directory = tombstone_directory(home.path());
        create_private_directory(&directory).unwrap();
        let prefixed_id = "history-content:turn:t1";
        let old_path = tombstone_path(&directory, "s1", prefixed_id);
        write_private_file(
            &old_path,
            &serde_json::to_vec(&MessageDeleteTombstone {
                version: TOMBSTONE_VERSION,
                session_id: "s1".into(),
                message_id: prefixed_id.into(),
            })
            .unwrap(),
        )
        .unwrap();

        let replay = reapply_persisted_deletions(home.path()).unwrap();

        assert_eq!(replay.deleted, 1);
        assert_eq!(replay.cleared_sessions, 1);
        assert!(!old_path.exists());
        let remaining = fs::read_to_string(&rollout).unwrap();
        assert!(!remaining.contains("t1"));
        assert!(remaining.contains("t2"));
    }

    #[test]
    fn startup_keeps_tombstones_when_storage_cannot_be_confirmed() {
        let home = tempdir().unwrap();
        let db = home.path().join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                [],
            )
            .unwrap();
        drop(connection);

        let result = delete_messages_persistently(home.path(), "s1", &["t1".into()]).unwrap();
        assert_eq!(result.deleted, 0);
        assert!(!result.unsupported_databases.is_empty());
        let tombstone_path = tombstone_path(&tombstone_directory(home.path()), "s1", "t1");
        assert!(tombstone_path.exists());

        let replay = reapply_persisted_deletions(home.path()).unwrap();

        assert_eq!(replay.deleted, 0);
        assert_eq!(replay.cleared_sessions, 0);
        assert_eq!(replay.failures.len(), 1);
        assert!(tombstone_path.exists());
    }

    #[test]
    fn startup_keeps_tombstones_after_a_partial_legacy_database_delete() {
        let home = tempdir().unwrap();
        let sqlite_dir = home.path().join("sqlite");
        fs::create_dir_all(&sqlite_dir).unwrap();
        let unsupported = Connection::open(sqlite_dir.join("codex.db")).unwrap();
        unsupported
            .execute("CREATE TABLE threads (id TEXT PRIMARY KEY)", [])
            .unwrap();
        drop(unsupported);
        let legacy = Connection::open(home.path().join("state_5.sqlite")).unwrap();
        legacy
            .execute(
                "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT NOT NULL)",
                [],
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO messages (id, session_id) VALUES (?1, ?2)",
                params!["t1", "s1"],
            )
            .unwrap();
        drop(legacy);

        let result = delete_messages_persistently(home.path(), "s1", &["t1".into()]).unwrap();
        assert_eq!(result.deleted, 1);
        assert!(!result.unsupported_databases.is_empty());
        let tombstone_path = tombstone_path(&tombstone_directory(home.path()), "s1", "t1");
        assert!(tombstone_path.exists());

        let replay = reapply_persisted_deletions(home.path()).unwrap();

        assert_eq!(replay.cleared_sessions, 0);
        assert_eq!(replay.failures.len(), 1);
        assert!(tombstone_path.exists());
    }
}
