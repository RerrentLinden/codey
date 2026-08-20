use std::collections::BTreeSet;

use anyhow::{Context, Result};
use toml_edit::{Item, Table, Value, value};

use super::{
    CODEY_FASTCTX_NAMESPACE, CODEY_FASTCTX_SERVER_ID, CODEY_HOOK_EVENTS,
    direct_only_tool_namespaces, direct_only_tool_namespaces_mut, document_string, parse_document,
};
use crate::codex_config_guidance::{
    codey_fastctx_guidance_blocks, remove_owned_guidance_block, remove_owned_guidance_paragraph,
    root_agent_collaboration_usage_hint_blocks,
};

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
    restore_codey_hooks(&original, &applied, &mut current);
    restore_codey_direct_only_namespaces(&original, &applied, &mut current);
    if current.as_table().is_empty() {
        Ok(String::new())
    } else {
        document_string(&current)
    }
}

fn restore_codey_hooks(
    original: &toml_edit::DocumentMut,
    applied: &toml_edit::DocumentMut,
    current: &mut toml_edit::DocumentMut,
) {
    let original_hooks = original.get("hooks").and_then(Item::as_table);
    let applied_hooks = applied.get("hooks").and_then(Item::as_table);
    let Some(current_hooks) = current.get_mut("hooks").and_then(Item::as_table_mut) else {
        return;
    };

    for event in CODEY_HOOK_EVENTS {
        let original_event = original_hooks.and_then(|hooks| hooks.get(event));
        let applied_event = applied_hooks.and_then(|hooks| hooks.get(event));
        let remove_empty_added_event =
            match (original_event, applied_event, current_hooks.get_mut(event)) {
                (
                    original_event,
                    Some(Item::ArrayOfTables(applied_groups)),
                    Some(Item::ArrayOfTables(current_groups)),
                ) => {
                    let original_groups = original_event.and_then(Item::as_array_of_tables);
                    let original_gate_count = original_groups
                        .map(|groups| {
                            groups
                                .iter()
                                .filter(|group| table_is_codey_hook_group(group))
                                .count()
                        })
                        .unwrap_or_default();
                    let added_gate_count = applied_groups
                        .iter()
                        .filter(|group| table_is_codey_hook_group(group))
                        .count()
                        .saturating_sub(original_gate_count);
                    let mut removed_gate_count = 0;
                    let mut original_matches = original_groups
                        .map(|groups| vec![false; groups.len()])
                        .unwrap_or_default();
                    for applied_group in applied_groups
                        .iter()
                        .filter(|group| table_is_codey_hook_group(group))
                    {
                        let matching_original = original_groups.and_then(|groups| {
                            groups.iter().enumerate().position(|(index, group)| {
                                !original_matches[index]
                                    && tables_semantically_equal(group, applied_group)
                            })
                        });
                        if let Some(index) = matching_original {
                            original_matches[index] = true;
                            continue;
                        }
                        let current_match = current_groups
                            .iter()
                            .enumerate()
                            .filter(|(_, group)| tables_semantically_equal(group, applied_group))
                            .map(|(index, _)| index)
                            .last();
                        if let Some(index) = current_match {
                            current_groups.remove(index);
                            removed_gate_count += 1;
                        }
                    }
                    while removed_gate_count < added_gate_count {
                        let current_gate_count = current_groups
                            .iter()
                            .filter(|group| table_is_codey_hook_group(group))
                            .count();
                        if current_gate_count <= original_gate_count {
                            break;
                        }
                        let current_match = current_groups
                            .iter()
                            .enumerate()
                            .filter(|(_, group)| table_is_codey_hook_group(group))
                            .map(|(index, _)| index)
                            .last();
                        let Some(index) = current_match else {
                            break;
                        };
                        current_groups.remove(index);
                        removed_gate_count += 1;
                    }
                    original_event.is_none() && current_groups.is_empty()
                }
                (
                    original_event,
                    Some(Item::Value(Value::Array(applied_groups))),
                    Some(Item::Value(Value::Array(current_groups))),
                ) => {
                    let original_groups = original_event.and_then(Item::as_array);
                    let original_gate_count = original_groups
                        .map(|groups| {
                            groups
                                .iter()
                                .filter(|group| value_is_codey_hook_group(group))
                                .count()
                        })
                        .unwrap_or_default();
                    let added_gate_count = applied_groups
                        .iter()
                        .filter(|group| value_is_codey_hook_group(group))
                        .count()
                        .saturating_sub(original_gate_count);
                    let mut removed_gate_count = 0;
                    let mut original_matches = original_groups
                        .map(|groups| vec![false; groups.len()])
                        .unwrap_or_default();
                    for applied_group in applied_groups
                        .iter()
                        .filter(|group| value_is_codey_hook_group(group))
                    {
                        let matching_original = original_groups.and_then(|groups| {
                            groups.iter().enumerate().position(|(index, group)| {
                                !original_matches[index]
                                    && values_semantically_equal(group, applied_group)
                            })
                        });
                        if let Some(index) = matching_original {
                            original_matches[index] = true;
                            continue;
                        }
                        let current_match = current_groups
                            .iter()
                            .enumerate()
                            .filter(|(_, group)| values_semantically_equal(group, applied_group))
                            .map(|(index, _)| index)
                            .last();
                        if let Some(index) = current_match {
                            current_groups.remove(index);
                            removed_gate_count += 1;
                        }
                    }
                    while removed_gate_count < added_gate_count {
                        let current_gate_count = current_groups
                            .iter()
                            .filter(|group| value_is_codey_hook_group(group))
                            .count();
                        if current_gate_count <= original_gate_count {
                            break;
                        }
                        let current_match = current_groups
                            .iter()
                            .enumerate()
                            .filter(|(_, group)| value_is_codey_hook_group(group))
                            .map(|(index, _)| index)
                            .last();
                        let Some(index) = current_match else {
                            break;
                        };
                        current_groups.remove(index);
                        removed_gate_count += 1;
                    }
                    original_event.is_none() && current_groups.is_empty()
                }
                _ => false,
            };
        if remove_empty_added_event {
            current_hooks.remove(event);
        }
    }

    let remove_empty_added_hooks = original_hooks.is_none() && current_hooks.is_empty();
    if remove_empty_added_hooks {
        current.as_table_mut().remove("hooks");
    }
}

fn table_is_codey_hook_group(group: &Table) -> bool {
    group
        .get("hooks")
        .and_then(Item::as_array_of_tables)
        .is_some_and(|handlers| handlers.iter().any(table_is_codey_hook_handler))
}

fn table_is_codey_hook_handler(handler: &Table) -> bool {
    ["command", "commandWindows"].iter().any(|field| {
        handler
            .get(field)
            .and_then(Item::as_str)
            .is_some_and(|command| {
                command.contains(crate::subagent_gate::HOOK_ARGUMENT)
                    || command.contains(crate::fastctx_route_gate::HOOK_ARGUMENT)
            })
    })
}

fn value_is_codey_hook_group(group: &Value) -> bool {
    group
        .as_inline_table()
        .and_then(|group| group.get("hooks"))
        .and_then(Value::as_array)
        .is_some_and(|handlers| handlers.iter().any(value_is_codey_hook_handler))
}

fn value_is_codey_hook_handler(handler: &Value) -> bool {
    handler.as_inline_table().is_some_and(|handler| {
        ["command", "commandWindows"].iter().any(|field| {
            handler
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    command.contains(crate::subagent_gate::HOOK_ARGUMENT)
                        || command.contains(crate::fastctx_route_gate::HOOK_ARGUMENT)
                })
        })
    })
}

pub(super) fn restore_owned_model_provider_changes(
    original: &str,
    applied: &str,
    current: &str,
) -> Result<String> {
    let original = parse_document(original).context("解析 Codex 原配置备份失败")?;
    let applied = parse_document(applied).context("解析 Codey 已应用配置快照失败")?;
    let mut current = parse_document(current).context("解析 Codex 当前配置失败")?;
    let empty_original = Table::new();
    let empty_applied = Table::new();
    let original_providers = original
        .get("model_providers")
        .and_then(Item::as_table)
        .unwrap_or(&empty_original);
    let applied_providers = applied
        .get("model_providers")
        .and_then(Item::as_table)
        .unwrap_or(&empty_applied);
    let original_providers_missing = original.get("model_providers").is_none();
    let remove_empty_providers = current
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .is_some_and(|current_providers| {
            restore_table_changes(original_providers, applied_providers, current_providers);
            original_providers_missing && current_providers.is_empty()
        });
    if remove_empty_providers {
        current.as_table_mut().remove("model_providers");
    }
    if current.as_table().is_empty() {
        Ok(String::new())
    } else {
        document_string(&current)
    }
}

fn restore_codey_direct_only_namespaces(
    original: &toml_edit::DocumentMut,
    applied: &toml_edit::DocumentMut,
    current: &mut toml_edit::DocumentMut,
) {
    for namespace in [
        CODEY_FASTCTX_NAMESPACE,
        crate::subagent_control_mcp::NAMESPACE,
    ] {
        let contains_namespace = |entries: Option<&toml_edit::Array>| {
            entries.is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.as_str() == Some(namespace))
            })
        };
        let original_has_namespace = contains_namespace(direct_only_tool_namespaces(original));
        let applied_has_namespace = contains_namespace(direct_only_tool_namespaces(applied));
        if original_has_namespace == applied_has_namespace {
            continue;
        }

        let Some(current_namespaces) = direct_only_tool_namespaces_mut(current) else {
            continue;
        };
        if applied_has_namespace {
            let namespace_index = current_namespaces
                .iter()
                .position(|entry| entry.as_str() == Some(namespace));
            if let Some(index) = namespace_index {
                current_namespaces.remove(index);
            }
        } else if current_namespaces
            .iter()
            .all(|entry| entry.as_str() != Some(namespace))
        {
            current_namespaces.push(namespace);
        }
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

        if [
            CODEY_FASTCTX_SERVER_ID,
            crate::subagent_control_mcp::SERVER_ID,
        ]
        .contains(&key.as_str())
            && original_item.is_none()
        {
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

        if restore_owned_value(&key, original_item, applied_item, current.get_mut(&key)) {
            if key == "root_agent_usage_hint_text"
                && original_item.is_none()
                && current
                    .get(&key)
                    .and_then(Item::as_str)
                    .is_some_and(|text| text.trim().is_empty())
            {
                current.remove(&key);
            }
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

fn restore_owned_value(
    key: &str,
    original: Option<&Item>,
    applied: Option<&Item>,
    current: Option<&mut Item>,
) -> bool {
    match key {
        "direct_only_tool_namespaces" => {
            let Some(entries) = current.and_then(Item::as_array_mut) else {
                return false;
            };
            let mut changed = false;
            for namespace in [
                CODEY_FASTCTX_NAMESPACE,
                crate::subagent_control_mcp::NAMESPACE,
            ] {
                let original_has_namespace =
                    original.and_then(Item::as_array).is_some_and(|items| {
                        items.iter().any(|entry| entry.as_str() == Some(namespace))
                    });
                let applied_has_namespace = applied.and_then(Item::as_array).is_some_and(|items| {
                    items.iter().any(|entry| entry.as_str() == Some(namespace))
                });
                if original_has_namespace == applied_has_namespace {
                    continue;
                }
                if applied_has_namespace {
                    let namespace_index = entries
                        .iter()
                        .position(|entry| entry.as_str() == Some(namespace));
                    if let Some(index) = namespace_index {
                        entries.remove(index);
                        changed = true;
                    }
                } else if entries
                    .iter()
                    .all(|entry| entry.as_str() != Some(namespace))
                {
                    entries.push(namespace);
                    changed = true;
                }
            }
            changed
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
        "root_agent_usage_hint_text" => {
            let Some(current) = current else {
                return false;
            };
            let Some(text) = current.as_str() else {
                return false;
            };
            let mut restored = text.to_string();
            let mut changed = false;
            let original_blocks = original
                .and_then(Item::as_str)
                .map(root_agent_collaboration_usage_hint_blocks)
                .unwrap_or_default();
            let applied_blocks = applied
                .and_then(Item::as_str)
                .map(root_agent_collaboration_usage_hint_blocks)
                .unwrap_or_default();
            for guidance in applied_blocks {
                if original_blocks.contains(&guidance) {
                    continue;
                }
                while let Some(without_guidance) =
                    remove_owned_guidance_paragraph(&restored, guidance)
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
