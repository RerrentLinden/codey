use serde_json::Value;

const KEYS: &[&str] = &[
    "type",
    "title",
    "description",
    "default",
    "properties",
    "required",
    "additionalProperties",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "items",
    "minItems",
    "maxItems",
];

pub fn check_schema(schema: &Value) -> Result<(), String> {
    check(schema, 0)
}

fn check(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err("配置定义嵌套过深".into());
    }
    let object = schema.as_object().ok_or("配置定义必须是对象")?;
    for key in object.keys() {
        if !KEYS.contains(&key.as_str()) {
            return Err(format!("不支持的配置约束: {key}"));
        }
    }
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or("配置定义必须声明 type")?;
    if ![
        "object", "array", "string", "boolean", "integer", "number", "null",
    ]
    .contains(&kind)
    {
        return Err("不支持的配置类型".into());
    }
    for key in ["title", "description"] {
        if schema.get(key).is_some_and(|v| !v.is_string()) {
            return Err(format!("{key} 必须是字符串"));
        }
    }
    if let Some(values) = schema.get("enum") {
        if values.as_array().is_none_or(|v| v.is_empty()) {
            return Err("enum 必须是非空数组".into());
        }
    }
    for key in ["properties", "required", "additionalProperties"] {
        if schema.get(key).is_some() && kind != "object" {
            return Err(format!("{key} 只用于 object"));
        }
    }
    if let Some(properties) = schema.get("properties") {
        for child in properties
            .as_object()
            .ok_or("properties 必须是对象")?
            .values()
        {
            check(child, depth + 1)?;
        }
    }
    if let Some(required) = schema.get("required") {
        for name in required.as_array().ok_or("required 必须是数组")? {
            let name = name.as_str().ok_or("required 项必须是字符串")?;
            if schema.get("properties").and_then(|v| v.get(name)).is_none() {
                return Err(format!("required 未定义字段: {name}"));
            }
        }
    }
    if schema
        .get("additionalProperties")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err("additionalProperties 只支持布尔值".into());
    }
    for key in ["minimum", "maximum"] {
        if let Some(v) = schema.get(key) {
            if !["number", "integer"].contains(&kind) || !v.is_number() {
                return Err(format!("{key} 必须用于数值类型"));
            }
        }
    }
    for (key, expected) in [
        ("minLength", "string"),
        ("maxLength", "string"),
        ("minItems", "array"),
        ("maxItems", "array"),
    ] {
        if let Some(v) = schema.get(key) {
            if kind != expected || v.as_u64().is_none() {
                return Err(format!("{key} 必须用于 {expected} 且为非负整数"));
            }
        }
    }
    if let Some(items) = schema.get("items") {
        if kind != "array" {
            return Err("items 只用于 array".into());
        }
        check(items, depth + 1)?;
    }
    for (min, max) in [
        ("minimum", "maximum"),
        ("minLength", "maxLength"),
        ("minItems", "maxItems"),
    ] {
        if let (Some(a), Some(b)) = (
            schema.get(min).and_then(Value::as_f64),
            schema.get(max).and_then(Value::as_f64),
        ) {
            if a > b {
                return Err(format!("{min} 不能大于 {max}"));
            }
        }
    }
    if let Some(default) = schema.get("default") {
        validate(schema, default)?;
    }
    Ok(())
}

pub fn default_value(schema: &Value) -> Value {
    if let Some(v) = schema.get("default") {
        return v.clone();
    }
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        let mut object = serde_json::Map::new();
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in props {
                if child.get("default").is_some() {
                    object.insert(key.clone(), default_value(child));
                }
            }
        }
        Value::Object(object)
    } else {
        Value::Null
    }
}

pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or("配置类型缺失")?;
    let valid = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => false,
    };
    if !valid {
        return Err(format!("配置值类型应为 {kind}"));
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err("配置值不在 enum 允许范围内".into());
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("缺少配置字段: {name}"));
                }
            }
        }
        for (name, v) in object {
            if let Some(child) = schema.get("properties").and_then(|p| p.get(name)) {
                validate(child, v).map_err(|e| format!("{name}: {e}"))?;
            } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                return Err(format!("未知配置字段: {name}"));
            }
        }
    }
    if let Some(array) = value.as_array() {
        bound(schema, "minItems", "maxItems", array.len() as f64)?;
        if let Some(items) = schema.get("items") {
            for v in array {
                validate(items, v)?;
            }
        }
    }
    if let Some(s) = value.as_str() {
        bound(schema, "minLength", "maxLength", s.chars().count() as f64)?;
    }
    if let Some(v) = value.as_f64() {
        bound(schema, "minimum", "maximum", v)?;
    }
    Ok(())
}

fn bound(schema: &Value, min: &str, max: &str, value: f64) -> Result<(), String> {
    if schema
        .get(min)
        .and_then(Value::as_f64)
        .is_some_and(|v| value < v)
    {
        return Err(format!("低于 {min}"));
    }
    if schema
        .get(max)
        .and_then(Value::as_f64)
        .is_some_and(|v| value > v)
    {
        return Err(format!("超过 {max}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unsupported_constraints_and_invalid_config_rejected() {
        assert!(check_schema(&json!({"type":"string","pattern":"x"})).is_err());
        let schema = json!({"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"],"additionalProperties":false});
        check_schema(&schema).unwrap();
        assert!(validate(&schema, &json!({})).is_err());
        assert!(validate(&schema, &json!({"count":0})).is_err());
        assert!(validate(&schema, &json!({"count":1,"other":true})).is_err());
        validate(&schema, &json!({"count":1})).unwrap();
    }
}
