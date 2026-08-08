use std::collections::BTreeSet;

use anyhow::{Context, Result};
use toml_edit::{Item, Table, Value, value};

use super::{CODEY_FASTCTX_NAMESPACE, CODEY_FASTCTX_SERVER_ID, document_string, parse_document};
use crate::codex_config_guidance::{codey_fastctx_guidance_blocks, remove_owned_guidance_block};

pub(super) fn restore_owned_config_changes(
    original: &str,
    applied: &str,
    current: &str,
) -> Result<String> {
    let original = parse_document(original).context("解析 Codex 原配置备份失败")?;
    let applied = parse_document(applied).context("解析 Codey 已应用配置快照失败")?;
    let mut current = parse_document(current).context("解析 Codex 当前配置失败")?;
    restore_table_changes(
        original.as_table(),
        applied.as_table(),
        current.as_table_mut(),
    );
    if current.as_table().is_empty() {
        Ok(String::new())
    } else {
        document_string(&current)
    }
}

fn restore_table_changes(original: &Table, applied: &Table, current: &mut Table) {
    let keys = original
        .iter()
        .chain(applied.iter())
        .map(|(key, _)| key.to_string())
        .collect::<BTreeSet<_>>();

    for key in keys {
        let original_item = original.get(&key).filter(|item| !item.is_none());
        let applied_item = applied.get(&key).filter(|item| !item.is_none());
        if optional_items_semantically_equal(original_item, applied_item) {
            continue;
        }

        let current_matches_applied = optional_items_semantically_equal(
            current.get(&key).filter(|item| !item.is_none()),
            applied_item,
        );
        if current_matches_applied {
            if let Some(original_item) = original_item {
                current.insert(&key, original_item.clone());
            } else {
                current.remove(&key);
            }
            continue;
        }

        if key == CODEY_FASTCTX_SERVER_ID && original_item.is_none() {
            let still_codey_owned = applied_item
                .and_then(Item::as_table)
                .zip(current.get(&key).and_then(Item::as_table))
                .is_some_and(|(applied, current)| {
                    ["command", "args"].iter().all(|field| {
                        optional_items_semantically_equal(applied.get(field), current.get(field))
                    })
                });
            if still_codey_owned {
                current.remove(&key);
            }
            // A complete replacement under the reserved id belongs to the
            // concurrent writer; do not strip matching fields out of it.
            continue;
        }

        if restore_fastctx_owned_value(&key, original_item, applied_item, current.get_mut(&key)) {
            continue;
        }

        let empty_original = Table::new();
        let original_table = match original_item {
            Some(item) => item.as_table(),
            None => Some(&empty_original),
        };
        let applied_table = applied_item.and_then(Item::as_table);
        let mut remove_empty_added_table = false;
        if let (Some(original_table), Some(applied_table), Some(current_table)) = (
            original_table,
            applied_table,
            current.get_mut(&key).and_then(Item::as_table_mut),
        ) {
            restore_table_changes(original_table, applied_table, current_table);
            remove_empty_added_table = original_item.is_none() && current_table.is_empty();
        }
        if remove_empty_added_table {
            current.remove(&key);
        }
    }
}

fn restore_fastctx_owned_value(
    key: &str,
    original: Option<&Item>,
    applied: Option<&Item>,
    current: Option<&mut Item>,
) -> bool {
    match key {
        "direct_only_tool_namespaces" => {
            let original_has_namespace = original.and_then(Item::as_array).is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
            });
            let applied_has_namespace = applied.and_then(Item::as_array).is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
            });
            if original_has_namespace || !applied_has_namespace {
                return false;
            }
            let Some(entries) = current.and_then(Item::as_array_mut) else {
                return false;
            };
            let Some(index) = entries
                .iter()
                .position(|entry| entry.as_str() == Some(CODEY_FASTCTX_NAMESPACE))
            else {
                return false;
            };
            entries.remove(index);
            true
        }
        "developer_instructions" | "subagent_developer_instructions" => {
            let Some(current) = current else {
                return false;
            };
            let Some(text) = current.as_str() else {
                return false;
            };
            if key == "subagent_developer_instructions"
                && original.is_none()
                && let Some(applied_text) = applied.and_then(Item::as_str)
                && let Some(without_applied) = remove_owned_guidance_block(text, applied_text)
            {
                *current = value(without_applied);
                return true;
            }
            let mut restored = text.to_string();
            let mut changed = false;
            let original_text = original.and_then(Item::as_str);
            let applied_blocks = applied
                .and_then(Item::as_str)
                .map(codey_fastctx_guidance_blocks)
                .unwrap_or_default();
            for guidance in applied_blocks {
                if original_text.is_some_and(|text| text.contains(&guidance)) {
                    continue;
                }
                while let Some(without_guidance) = remove_owned_guidance_block(&restored, &guidance)
                {
                    restored = without_guidance;
                    changed = true;
                }
            }
            if changed {
                *current = value(restored);
            }
            changed
        }
        _ => false,
    }
}

fn optional_items_semantically_equal(left: Option<&Item>, right: Option<&Item>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => items_semantically_equal(left, right),
        _ => false,
    }
}

pub(super) fn items_semantically_equal(left: &Item, right: &Item) -> bool {
    match (left, right) {
        (Item::None, Item::None) => true,
        (Item::Value(left), Item::Value(right)) => values_semantically_equal(left, right),
        (Item::Table(left), Item::Table(right)) => tables_semantically_equal(left, right),
        (Item::ArrayOfTables(left), Item::ArrayOfTables(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| tables_semantically_equal(left, right))
        }
        _ => false,
    }
}

pub(super) fn tables_semantically_equal(left: &Table, right: &Table) -> bool {
    left.len() == right.len()
        && left.iter().all(|(key, left)| {
            right
                .get(key)
                .is_some_and(|right| items_semantically_equal(left, right))
        })
}

fn values_semantically_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(left), Value::String(right)) => left.value() == right.value(),
        (Value::Integer(left), Value::Integer(right)) => left.value() == right.value(),
        (Value::Float(left), Value::Float(right)) => {
            left.value().to_bits() == right.value().to_bits()
        }
        (Value::Boolean(left), Value::Boolean(right)) => left.value() == right.value(),
        (Value::Datetime(left), Value::Datetime(right)) => left.value() == right.value(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| values_semantically_equal(left, right))
        }
        (Value::InlineTable(left), Value::InlineTable(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| values_semantically_equal(left, right))
                })
        }
        _ => false,
    }
}
