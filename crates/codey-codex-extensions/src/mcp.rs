use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table};

pub const MASK: &str = "__CODEY_SECRET_UNCHANGED__";

// 在克隆和序列化前限制结构；报错只说明字段规则，不包含配置值或解析器原文。
pub fn json_table(value: &Value) -> Result<Table> {
    fn check(value: &Value, depth: usize, nodes: &mut usize, bytes: &mut usize) -> Result<()> {
        *nodes += 1;
        ensure!(depth <= 32 && *nodes <= 10000, "MCP JSON 配置结构超过限制");
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    *bytes += key.len();
                    ensure!(
                        *bytes as u64 <= crate::fsutil::MAX_FILE,
                        "MCP JSON 配置超过大小限制"
                    );
                    check(value, depth + 1, nodes, bytes)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    check(value, depth + 1, nodes, bytes)?;
                }
            }
            Value::String(text) => *bytes += text.len(),
            Value::Null => bail!("MCP JSON 配置不能包含 null，请移除未设置的字段"),
            _ => {}
        }
        ensure!(
            *bytes as u64 <= crate::fsutil::MAX_FILE,
            "MCP JSON 配置超过大小限制"
        );
        Ok(())
    }
    let fields = value
        .as_object()
        .context("configJson 必须为单个 MCP 服务对象")?;
    check(value, 0, &mut 0, &mut 0)?;
    let encoded =
        serde_json::to_vec(value).map_err(|_| anyhow::anyhow!("MCP JSON 配置无法序列化"))?;
    ensure!(
        encoded.len() as u64 <= crate::fsutil::MAX_FILE,
        "MCP JSON 配置超过大小限制"
    );
    ensure!(
        !fields.contains_key("mcpServers"),
        "configJson 仅接受单个服务，请先从 mcpServers 中选择服务"
    );
    ensure!(
        !(fields.contains_key("headers") && fields.contains_key("http_headers")),
        "headers 与 http_headers 不能同时设置"
    );
    if let Some(transport) = fields.get("type") {
        match transport.as_str() {
            Some("stdio") => ensure!(
                fields.contains_key("command") && !fields.contains_key("url"),
                "stdio 类型必须使用 command，不能包含 url"
            ),
            Some("http" | "streamable-http" | "streamableHttp") => ensure!(
                fields.contains_key("url") && !fields.contains_key("command"),
                "HTTP 类型必须使用 url，不能包含 command"
            ),
            Some("sse") => bail!("不支持旧版 SSE 连接，请使用 Streamable HTTP 配置"),
            _ => bail!("MCP type 仅支持 stdio、http、streamable-http 或 streamableHttp"),
        }
    }
    let mut normalized = fields.clone();
    normalized.remove("type");
    if let Some(headers) = normalized.remove("headers") {
        normalized.insert("http_headers".to_owned(), headers);
    }
    toml_edit::ser::to_document(&normalized)
        .map(|doc| doc.as_table().clone())
        .map_err(|_| anyhow::anyhow!("MCP JSON 配置包含无法保存为 TOML 的值"))
}

pub fn parse(bytes: Option<&[u8]>) -> Result<DocumentMut> {
    let text = std::str::from_utf8(bytes.unwrap_or_default()).context("配置不是 UTF-8")?;
    text.parse::<DocumentMut>()
        .context("Codex 配置 TOML 无效，未执行修改")
}

fn owned_table(item: &Item) -> Result<Table> {
    let entries = item.as_table_like().context("MCP 配置必须为表")?;
    if let Some(table) = item.as_table() {
        return Ok(table.clone());
    }
    let mut table = Table::new();
    for (k, v) in entries.iter() {
        table.insert(k, v.clone());
    }
    Ok(table)
}

fn document_text(table: Table) -> String {
    let mut doc = DocumentMut::new();
    *doc.as_table_mut() = table;
    doc.to_string()
}

pub fn object(table: &Item) -> Result<Value> {
    let text = document_text(owned_table(table)?);
    toml_edit::de::from_str(&text).context("MCP 配置解析失败")
}

pub fn table(doc: &DocumentMut, id: &str) -> Result<Item> {
    doc.get("mcp_servers")
        .and_then(|s| s.get(id))
        .cloned()
        .context("MCP 服务不存在")
}

pub fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
        "MCP 名称只能包含字母、数字、下划线和连字符"
    );
    Ok(())
}

pub fn validate(v: &Value) -> Result<()> {
    let obj = v.as_object().context("MCP 配置必须为对象")?;
    ensure!(
        !obj.contains_key("bearer_token"),
        "不接受明文 bearer_token，请使用 bearer_token_env_var"
    );
    validate_existing(v)
}

pub fn validate_existing(v: &Value) -> Result<()> {
    let obj = v.as_object().context("MCP 配置必须为对象")?;
    if let Some(token) = obj.get("bearer_token") {
        ensure!(
            token
                .as_str()
                .is_some_and(|s| !s.is_empty() && !s.contains(['\0', '\n', '\r'])),
            "bearer_token 必须为有效字符串"
        );
    }
    let cmd = obj.get("command");
    let url = obj.get("url");
    ensure!(
        cmd.is_some() ^ url.is_some(),
        "必须且只能设置 command 或 url"
    );
    if let Some(c) = cmd {
        ensure!(
            c.as_str()
                .is_some_and(|s| !s.trim().is_empty() && !s.contains('\0')),
            "command 必须为非空字符串"
        );
    }
    if let Some(u) = url {
        let u = u.as_str().context("url 必须为字符串")?;
        ensure!(
            u.starts_with("https://") || u.starts_with("http://"),
            "只支持 HTTP 或 HTTPS MCP 地址"
        );
        ensure!(
            !u.contains('@') && !u.chars().any(char::is_whitespace),
            "地址不能包含凭证或空白字符"
        );
        for key in ["args", "env", "env_vars", "cwd"] {
            ensure!(!obj.contains_key(key), "HTTP 服务不能包含 stdio 字段 {key}");
        }
    } else {
        for key in ["http_headers", "env_http_headers", "bearer_token_env_var"] {
            ensure!(!obj.contains_key(key), "stdio 服务不能包含 HTTP 字段 {key}");
        }
    }
    for key in ["args", "env_vars", "enabled_tools", "disabled_tools"] {
        if let Some(v) = obj.get(key) {
            ensure!(
                v.as_array().is_some_and(|a| a
                    .iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.contains('\0')))),
                "{key} 必须为字符串数组"
            );
        }
    }
    for key in ["enabled", "required"] {
        if let Some(v) = obj.get(key) {
            ensure!(v.is_boolean(), "{key} 必须为布尔值");
        }
    }
    for key in ["cwd", "bearer_token_env_var"] {
        if let Some(v) = obj.get(key) {
            ensure!(
                v.as_str()
                    .is_some_and(|s| !s.trim().is_empty() && !s.contains('\0')),
                "{key} 必须为非空字符串"
            );
        }
    }
    for key in ["env", "http_headers", "env_http_headers"] {
        if let Some(v) = obj.get(key) {
            let map = v.as_object().context(format!("{key} 必须为字符串映射"))?;
            for (k, value) in map {
                ensure!(
                    !k.is_empty()
                        && !k.contains(['\0', '\n', '\r'])
                        && value.as_str().is_some_and(|s| !s.contains('\0')),
                    "{key} 包含无效字段"
                );
            }
        }
    }
    for key in ["startup_timeout_sec", "tool_timeout_sec"] {
        if let Some(v) = obj.get(key) {
            ensure!(
                v.as_f64()
                    .is_some_and(|n| n.is_finite() && n > 0.0 && n <= 86400.0),
                "{key} 必须在 0 到 86400 秒之间"
            );
        }
    }
    for key in [
        "model",
        "model_provider",
        "profiles",
        "mcp_servers",
        "skills",
        "projects",
    ] {
        ensure!(
            !obj.contains_key(key),
            "只接受单个 MCP 服务的配置正文，不能包含 {key}"
        );
    }
    Ok(())
}

pub fn redacted(item: &Item) -> Result<Value> {
    let mut table = owned_table(item)?;
    for (key, value) in table.iter_mut() {
        // 未知字段无法判断是否包含凭证，整体保护并在保存时恢复原类型和内容。
        match key.get() {
            "command"
            | "args"
            | "url"
            | "cwd"
            | "enabled"
            | "required"
            | "startup_timeout_sec"
            | "tool_timeout_sec"
            | "enabled_tools"
            | "disabled_tools" => redact_item(key.get(), value),
            "env_vars" | "env_http_headers" | "bearer_token_env_var" => {}
            _ => *value = toml_edit::value(MASK),
        }
    }
    // 只返回解析后的 JSON 值，避免注释和格式装饰携带的凭证进入界面或导出。
    object(&Item::Table(table))
}

fn sensitive(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    key == "env"
        || key == "http_headers"
        || [
            "token",
            "secret",
            "password",
            "passwd",
            "credential",
            "authorization",
            "api_key",
            "apikey",
            "private_key",
            "access_key",
            "cookie",
        ]
        .iter()
        .any(|part| key.contains(part))
}

fn sensitive_url(value: &str) -> bool {
    value.contains("://") && value.contains(['?', '@', '#'])
}

fn redact_value(value: &mut toml_edit::Value) {
    if value.as_str().is_some_and(sensitive_url) {
        *value = toml_edit::Value::from(MASK);
    } else if let Some(table) = value.as_inline_table_mut() {
        for (key, value) in table.iter_mut() {
            if sensitive(key.get()) {
                *value = toml_edit::Value::from(MASK);
            } else {
                redact_value(value);
            }
        }
    } else if let Some(array) = value.as_array_mut() {
        let mut secret_follows = false;
        for value in array.iter_mut() {
            let text = value.as_str().unwrap_or_default();
            let flag = text.trim_start_matches('-');
            let next_secret = text.starts_with('-') && !text.contains('=') && sensitive(flag);
            let inline_secret = text
                .split_once(['=', ':'])
                .is_some_and(|(name, _)| sensitive(name.trim_start_matches('-')))
                || text.to_ascii_lowercase().starts_with("bearer ");
            if secret_follows || inline_secret {
                *value = toml_edit::Value::from(MASK);
            } else {
                redact_value(value);
            }
            secret_follows = next_secret;
        }
    }
}

fn redact_item(key: &str, item: &mut Item) {
    if sensitive(key) {
        *item = toml_edit::value(MASK);
    } else if let Some(value) = item.as_value_mut() {
        redact_value(value);
    } else if let Some(table) = item.as_table_mut() {
        for (key, value) in table.iter_mut() {
            redact_item(key.get(), value);
        }
    } else if let Some(tables) = item.as_array_of_tables_mut() {
        for table in tables.iter_mut() {
            for (key, value) in table.iter_mut() {
                redact_item(key.get(), value);
            }
        }
    }
}

fn restore_value(value: &mut toml_edit::Value, old: Option<&toml_edit::Value>) -> Result<()> {
    if value.as_str() == Some(MASK) {
        *value = old.cloned().context("新字段不能使用未修改凭证占位符")?;
    } else if let Some(table) = value.as_inline_table_mut() {
        for (key, value) in table.iter_mut() {
            restore_value(
                value,
                old.and_then(|o| o.as_inline_table())
                    .and_then(|o| o.get(key.get())),
            )?;
        }
    } else if let Some(array) = value.as_array_mut() {
        for (index, value) in array.iter_mut().enumerate() {
            restore_value(
                value,
                old.and_then(|o| o.as_array()).and_then(|o| o.get(index)),
            )?;
        }
    }
    Ok(())
}

fn restore_item(item: &mut Item, old: Option<&Item>) -> Result<()> {
    if item.as_str() == Some(MASK) {
        *item = old.cloned().context("新字段不能使用未修改凭证占位符")?;
    } else if let Some(value) = item.as_value_mut() {
        restore_value(value, old.and_then(Item::as_value))?;
    } else if let Some(table) = item.as_table_mut() {
        for (key, item) in table.iter_mut() {
            restore_item(item, old.and_then(|o| o.get(key.get())))?;
        }
    } else if let Some(tables) = item.as_array_of_tables_mut() {
        for (index, table) in tables.iter_mut().enumerate() {
            for (key, item) in table.iter_mut() {
                restore_item(
                    item,
                    old.and_then(Item::as_array_of_tables)
                        .and_then(|o| o.get(index))
                        .and_then(|o| o.get(key.get())),
                )?;
            }
        }
    }
    Ok(())
}

pub fn save(doc: &mut DocumentMut, id: &str, mut incoming: Table) -> Result<()> {
    validate_id(id)?;
    let old = doc.get("mcp_servers").and_then(|t| t.get(id)).cloned();
    let preserve_legacy_token = incoming.get("bearer_token").and_then(Item::as_str) == Some(MASK);
    for (key, item) in incoming.iter_mut() {
        restore_item(item, old.as_ref().and_then(|o| o.get(key.get())))?;
    }
    let mut merged = old
        .as_ref()
        .map(owned_table)
        .transpose()?
        .unwrap_or_default();
    // 基础字段允许删除，未知字段保留，防止旧版界面丢失新版配置。
    for key in [
        "command",
        "args",
        "url",
        "cwd",
        "env",
        "env_vars",
        "http_headers",
        "env_http_headers",
        "bearer_token_env_var",
        "bearer_token",
        "enabled",
        "required",
        "startup_timeout_sec",
        "tool_timeout_sec",
        "enabled_tools",
        "disabled_tools",
    ] {
        merged.remove(key);
    }
    for (key, item) in incoming {
        merged.insert(&key, item);
    }
    if let Some(enabled) = merged.get("enabled") {
        ensure!(enabled.as_bool().is_some(), "enabled 必须为布尔值");
    }
    if old.is_none() {
        merged["enabled"] = toml_edit::value(true);
    }
    let mut json = object(&Item::Table(merged.clone()))?;
    if json.to_string().contains(MASK) {
        bail!("凭证占位符只能用于保留原有敏感字段");
    }
    // 旧配置中的 Token 仅允许原样保留；新增或替换必须使用环境变量引用。
    if preserve_legacy_token {
        json.as_object_mut().unwrap().remove("bearer_token");
    }
    validate(&json)?;
    if !doc.contains_key("mcp_servers") {
        doc["mcp_servers"] = Item::Table(Table::new());
    }
    if let Some(servers) = doc["mcp_servers"].as_inline_table_mut() {
        servers.insert(
            id,
            toml_edit::Value::InlineTable(merged.into_inline_table()),
        );
    } else {
        doc["mcp_servers"][id] = Item::Table(merged);
    }
    Ok(())
}

pub fn list(doc: &DocumentMut, source: &str) -> Result<Vec<Value>> {
    let Some(servers) = doc.get("mcp_servers") else {
        return Ok(vec![]);
    };
    let servers = servers.as_table_like().context("mcp_servers 必须为表")?;
    Ok(servers
        .iter()
        .map(|(id, value)| {
            let command = value.get("command").and_then(Item::as_str);
            let url = value.get("url").and_then(Item::as_str);
            let transport = if command.is_some() {
                "stdio"
            } else if url.is_some() {
                "http"
            } else {
                "unknown"
            };
            let checked = object(value).and_then(|v| validate_existing(&v));
            let enabled_known = value.get("enabled").is_none_or(|v| v.as_bool().is_some());
            let mut entry = json!({"id":id,"name":id,"enabled":value.get("enabled").and_then(Item::as_bool).unwrap_or(true),"enabledKnown":enabled_known,"configurationStatus":if checked.is_ok(){"valid"}else{"invalid"},"error":checked.err().map(|e|e.to_string()),"canEdit":true,"canToggle":enabled_known,"canRemove":true,"canCheck":true,"sourcePath":source,"transport":transport,"readOnly":false,"summary":if command.is_some(){"本地进程"}else if url.is_some(){"HTTP 连接"}else{"配置待检查"}});
            // 保存会重新校验标识，所以不能把保存必然失败的条目显示为可编辑；
            // 删除入口保留，用户仍能清理这类历史配置。
            if validate_id(id).is_err() {
                entry["canEdit"] = json!(false);
                entry["reason"] = json!("名称含不支持的字符，无法在此编辑；可删除后重新创建");
            }
            entry
        })
        .collect())
}
