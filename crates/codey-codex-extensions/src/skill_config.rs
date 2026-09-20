use anyhow::{Context, Result};
use std::{collections::BTreeSet, path::PathBuf};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

/// Codex 接受表数组与内联表数组；只转换目标节点，保留其他配置。
fn rules(doc: &DocumentMut) -> Result<Vec<Table>> {
    let Some(config) = doc.get("skills").and_then(|v| v.get("config")) else {
        return Ok(Vec::new());
    };
    if let Some(tables) = config.as_array_of_tables() {
        return Ok(tables.iter().cloned().collect());
    }
    config
        .as_array()
        .context("skills.config 必须为表数组")?
        .iter()
        .map(|value| {
            let inline = value
                .as_inline_table()
                .context("skills.config 必须为表数组")?;
            Ok(inline.clone().into_table())
        })
        .collect()
}

pub fn disabled(doc: &DocumentMut) -> Result<(Vec<PathBuf>, BTreeSet<String>)> {
    let mut disabled = Vec::new();
    let mut named = BTreeSet::new();
    for rule in rules(doc)? {
        if let Some(name) = rule.get("name").and_then(Item::as_str) {
            named.insert(name.to_owned());
        }
        if let Some(path) = rule.get("path").and_then(Item::as_str) {
            let path = PathBuf::from(path);
            disabled.retain(|p| p != &path);
            if rule.get("enabled").and_then(Item::as_bool) == Some(false) {
                disabled.push(path);
            }
        }
    }
    Ok((disabled, named))
}

pub fn unknown_paths(doc: &DocumentMut) -> Result<BTreeSet<PathBuf>> {
    Ok(rules(doc)?
        .iter()
        .filter(|rule| rule.get("enabled").is_some_and(|v| v.as_bool().is_none()))
        .filter_map(|rule| rule.get("path").and_then(Item::as_str).map(PathBuf::from))
        .collect())
}

pub fn set_enabled(doc: &mut DocumentMut, path: &str, enabled: bool) -> Result<()> {
    rules(doc)?;
    if !doc.contains_key("skills") {
        doc["skills"] = Item::Table(Table::new());
    }
    let inline = doc["skills"].is_inline_table();
    let skills = doc["skills"]
        .as_table_like_mut()
        .context("skills 配置必须为表")?;
    if !skills.contains_key("config") {
        skills.insert(
            "config",
            if inline {
                Item::Value(Value::Array(Array::new()))
            } else {
                Item::ArrayOfTables(ArrayOfTables::new())
            },
        );
    }
    let config = skills.get_mut("config").unwrap();
    let mut matched = false;
    if let Some(tables) = config.as_array_of_tables_mut() {
        for rule in tables.iter_mut() {
            if rule.get("path").and_then(Item::as_str) == Some(path) {
                rule["enabled"] = toml_edit::value(enabled);
                matched = true;
            }
        }
        if !matched {
            let mut rule = Table::new();
            rule["path"] = toml_edit::value(path);
            rule["enabled"] = toml_edit::value(enabled);
            tables.push(rule);
        }
    } else {
        let array = config
            .as_array_mut()
            .context("skills.config 必须为表数组")?;
        for rule in array.iter_mut() {
            let rule = rule
                .as_inline_table_mut()
                .context("skills.config 必须为表数组")?;
            if rule.get("path").and_then(Value::as_str) == Some(path) {
                rule.insert("enabled", Value::from(enabled));
                matched = true;
            }
        }
        if !matched {
            let mut rule = InlineTable::new();
            rule.insert("path", Value::from(path));
            rule.insert("enabled", Value::from(enabled));
            array.push(Value::InlineTable(rule));
        }
    }
    Ok(())
}
