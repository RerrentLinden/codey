use std::collections::HashSet;

use rusqlite::Connection;

/// `PRAGMA table_info` 的统一包装：返回列名集合，表名做引号转义。此前同一
/// 逻辑散落在 6 个模块并在返回类型与转义处理上各自漂移。
pub(crate) fn table_columns(
    connection: &Connection,
    table: &str,
) -> rusqlite::Result<HashSet<String>> {
    let escaped = table.replace('"', "\"\"");
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{escaped}\")"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(columns)
}
