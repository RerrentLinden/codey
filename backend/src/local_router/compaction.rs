use base64::Engine as _;

use super::*;

// 压缩请求只在等待响应头时使用固定期限；响应体读取沿用上游响应的总期限
// 与空闲期限，避免把耗时较长的压缩在请求总期限处截断。
pub(crate) const COMPACTION_RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(120);

// Codex 运行时的协作任务载荷使用 Fernet 令牌：版本字节、时间戳、初始向量、
// 按 16 字节分组且至少一组的密文、HMAC。Codey 不持有解密密钥，只能在发送
// 前做结构校验。
const FERNET_PREFIX_BYTES: usize = 1 + 8 + 16;
const FERNET_SUFFIX_BYTES: usize = 32;
const FERNET_BLOCK_BYTES: usize = 16;

enum EncryptedContentRewrite {
    Keep,
    Replace(Value),
    Drop,
}

fn input_items(body: &Value) -> &[Value] {
    match body.get("input") {
        Some(Value::Array(items)) => items,
        Some(item @ Value::Object(_)) => std::slice::from_ref(item),
        _ => &[],
    }
}

pub(crate) fn is_compaction_request(body: &Value, kind: ResponsesRequestKind) -> bool {
    kind == ResponsesRequestKind::Compact
        || input_items(body)
            .iter()
            .any(|item| item["type"] == "compaction_trigger")
}

pub(crate) fn validate_portable_context(body: &Value) -> Result<()> {
    let is_compaction = |item: &Value| {
        matches!(
            item.get("type").and_then(Value::as_str),
            Some("compaction" | "compaction_trigger")
        )
    };
    for item in input_items(body) {
        let content = item.get("content");
        if is_compaction(item)
            || content.is_some_and(|content| {
                is_compaction(content)
                    || content
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(is_compaction))
            })
        {
            anyhow::bail!(
                "context_not_portable: compaction 历史不能转换到当前线路；请回到原线路完成本地摘要后再切换"
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_cross_route_context(body: &Value) -> Result<()> {
    validate_portable_context(body)?;
    fn contains_item_reference(value: &Value) -> bool {
        if value.get("type").and_then(Value::as_str) == Some("item_reference") {
            return true;
        }
        match value {
            Value::Array(items) => items.iter().any(contains_item_reference),
            Value::Object(object) => object.values().any(contains_item_reference),
            _ => false,
        }
    }
    if input_items(body).iter().any(contains_item_reference) {
        anyhow::bail!("context_not_portable: item_reference 属于上一条线路，不能发送到新的供应商");
    }
    Ok(())
}

/// 原生 Responses 线路在发送前统一处理协作任务载荷和跨线路 reasoning 状态。
pub(crate) fn normalize_native_responses_context(
    body: &mut Value,
    discard_opaque_reasoning: bool,
) -> bool {
    let mut changed = normalize_encrypted_agent_payloads(body);
    // 同一线路必须原样回传 reasoning，包括第三方 thinking 模式需要的明文内容。
    if discard_opaque_reasoning {
        changed |= discard_reasoning_history(body);
    }
    changed
}

// 第三方 thinking 模式只校验 reasoning 明文是否存在，占位文本不影响后续回答。
pub(crate) const MISSING_REASONING_TEXT_PLACEHOLDER: &str = "(thinking unavailable)";

/// 部分第三方 thinking 模式（DeepSeek 等）要求把上一轮的 reasoning 明文原样
/// 回传，而 Codex 回放历史时会省略 reasoning 项的明文 content，只保留
/// encrypted_content。切换线路时这些密文项会被去掉。这里给缺少明文的
/// reasoning 项补占位文本；已经没有 reasoning 项的助手回合补回一项。
/// Chat Completions 则给缺少 `reasoning_content` 的助手消息补上同一段占位。
/// 已有明文的项保持原始字节不变。
pub(crate) fn fill_missing_reasoning_text(body: &mut Value) -> bool {
    if let Some(input) = body.get_mut("input") {
        return match input {
            Value::Array(items) => {
                let mut changed = false;
                for item in items.iter_mut() {
                    changed |= fill_reasoning_item_text(item);
                }
                changed |= insert_missing_reasoning_items(items);
                changed
            }
            item @ Value::Object(_) => fill_reasoning_item_text(item),
            _ => false,
        };
    }
    fill_missing_chat_reasoning_content(body)
}

fn fill_reasoning_item_text(item: &mut Value) -> bool {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return false;
    }
    let Some(object) = item.as_object_mut() else {
        return false;
    };
    if object.get("content").is_none() {
        object.insert(
            "content".to_string(),
            Value::Array(vec![reasoning_text_placeholder()]),
        );
        return true;
    }
    let Some(content) = object.get_mut("content") else {
        return false;
    };
    match content {
        Value::Array(parts) => {
            if parts.iter().any(reasoning_part_has_text) {
                return false;
            }
            // 空白片段一并清理，只留下占位明文。
            parts.retain(|part| !is_reasoning_text_part(part));
            parts.push(reasoning_text_placeholder());
            true
        }
        Value::Object(_) => {
            if reasoning_part_has_text(content) {
                return false;
            }
            *content = reasoning_text_placeholder();
            true
        }
        _ => {
            *content = Value::Array(vec![reasoning_text_placeholder()]);
            true
        }
    }
}

fn reasoning_text_placeholder() -> Value {
    json!({"type":"reasoning_text","text":MISSING_REASONING_TEXT_PLACEHOLDER})
}

fn placeholder_reasoning_item() -> Value {
    json!({
        "type": "reasoning",
        "summary": [],
        "content": [reasoning_text_placeholder()],
    })
}

/// 切模型后 reasoning 项已被删除。每个助手回合开头补一项占位，供思考模式回传。
/// 同一回合里的工具调用和工具结果保持在一起，不在结果后面再插一条推理。
fn insert_missing_reasoning_items(items: &mut Vec<Value>) -> bool {
    let mut changed = false;
    let mut index = 0;
    while index < items.len() {
        if !is_assistant_side_item(&items[index]) {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < items.len() && !is_user_turn_boundary(&items[end]) {
            end += 1;
        }
        if items[index..end].iter().any(reasoning_item_has_text) {
            index = end;
            continue;
        }
        items.insert(index, placeholder_reasoning_item());
        changed = true;
        index = end + 1;
    }
    changed
}

fn is_user_turn_boundary(item: &Value) -> bool {
    matches!(
        item.get("role").and_then(Value::as_str),
        Some("user" | "system" | "developer")
    )
}

fn is_assistant_side_item(item: &Value) -> bool {
    let item_type = item.get("type").and_then(Value::as_str);
    let role = item.get("role").and_then(Value::as_str);
    match item_type {
        Some(
            "reasoning" | "function_call" | "custom_tool_call" | "tool_search_call"
            | "web_search_call",
        ) => true,
        Some("message") => role == Some("assistant"),
        Some(
            "function_call_output"
            | "custom_tool_call_output"
            | "tool_search_output"
            | "compaction",
        ) => false,
        _ => role == Some("assistant"),
    }
}

fn reasoning_item_has_text(item: &Value) -> bool {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return false;
    }
    match item.get("content") {
        Some(Value::Array(parts)) => parts.iter().any(reasoning_part_has_text),
        Some(part @ Value::Object(_)) => reasoning_part_has_text(part),
        _ => false,
    }
}

fn fill_missing_chat_reasoning_content(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
        {
            continue;
        }
        let Some(object) = message.as_object_mut() else {
            continue;
        };
        object.insert(
            "reasoning_content".to_string(),
            Value::String(MISSING_REASONING_TEXT_PLACEHOLDER.to_string()),
        );
        changed = true;
    }
    changed
}

fn reasoning_part_has_text(part: &Value) -> bool {
    is_reasoning_text_part(part)
        && part
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
}

fn is_reasoning_text_part(part: &Value) -> bool {
    matches!(
        part.get("type").and_then(Value::as_str),
        Some("reasoning_text" | "text")
    )
}

/// 协作任务正文由 Codex 运行时刻写入 `agent_message` 的 `encrypted_content`
/// 字段，交给持有密钥的上游解密；线路本身不判断内容。第三方线路经常把该
/// 字段直接写成明文，接收方解密失败会拒绝整条请求。Chat Completions 和
/// Anthropic Messages 都表达不了这个字段，转换时只能丢弃，任务正文会随之
/// 消失，因此先按结构识别：令牌形态留给原生 Responses，适配线路显式拒绝；
/// 其余形态改写为可见文本。
pub(crate) fn normalize_encrypted_agent_payloads(body: &mut Value) -> bool {
    match body.get_mut("input") {
        Some(Value::Array(items)) => {
            let mut changed = false;
            for item in items {
                changed |= normalize_agent_message_item(item);
            }
            changed
        }
        Some(item @ Value::Object(_)) => normalize_agent_message_item(item),
        _ => false,
    }
}

/// Chat 和 Anthropic 无法解密协作任务，必须在转换丢弃不透明内容前拒绝。
/// 只检查 agent_message，其他消息的推理状态仍按原有规则处理。
pub(crate) fn validate_adapted_agent_payloads(body: &Value) -> Result<()> {
    let is_encrypted_payload = |part: &Value| {
        part.get("type").and_then(Value::as_str) == Some("encrypted_content")
            && part
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(is_codex_encrypted_payload)
    };
    for item in input_items(body) {
        if item.get("type").and_then(Value::as_str) != Some("agent_message") {
            continue;
        }
        if item.get("content").is_some_and(|content| {
            is_encrypted_payload(content)
                || content
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(is_encrypted_payload))
        }) {
            anyhow::bail!(
                "context_not_portable: 当前线路无法解密子代理任务正文，请使用原生 Responses 线路或重新提供明文任务"
            );
        }
    }
    Ok(())
}

fn normalize_agent_message_item(item: &mut Value) -> bool {
    if item.get("type").and_then(Value::as_str) != Some("agent_message") {
        return false;
    }
    let Some(object) = item.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    let mut drop_content = false;
    match object.get_mut("content") {
        Some(Value::Array(parts)) => {
            parts.retain_mut(|part| match encrypted_content_rewrite(part) {
                EncryptedContentRewrite::Keep => true,
                EncryptedContentRewrite::Replace(replacement) => {
                    *part = replacement;
                    changed = true;
                    true
                }
                EncryptedContentRewrite::Drop => {
                    changed = true;
                    false
                }
            })
        }
        Some(part @ Value::Object(_)) => match encrypted_content_rewrite(part) {
            EncryptedContentRewrite::Keep => {}
            EncryptedContentRewrite::Replace(replacement) => {
                *part = replacement;
                changed = true;
            }
            EncryptedContentRewrite::Drop => {
                changed = true;
                drop_content = true;
            }
        },
        _ => {}
    }
    if drop_content {
        object.remove("content");
    }
    changed
}

fn encrypted_content_rewrite(part: &Value) -> EncryptedContentRewrite {
    if part.get("type").and_then(Value::as_str) != Some("encrypted_content") {
        return EncryptedContentRewrite::Keep;
    }
    let Some(payload) = part.get("encrypted_content").and_then(Value::as_str) else {
        return EncryptedContentRewrite::Drop;
    };
    if is_codex_encrypted_payload(payload) {
        return EncryptedContentRewrite::Keep;
    }
    if payload.trim().is_empty() {
        return EncryptedContentRewrite::Drop;
    }
    EncryptedContentRewrite::Replace(json!({"type":"input_text","text":payload}))
}

fn is_codex_encrypted_payload(value: &str) -> bool {
    let encoded = value.trim().trim_end_matches('=');
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        return false;
    };
    decoded.first() == Some(&0x80)
        && decoded.len() >= FERNET_PREFIX_BYTES + FERNET_SUFFIX_BYTES + FERNET_BLOCK_BYTES
        && (decoded.len() - FERNET_PREFIX_BYTES - FERNET_SUFFIX_BYTES)
            .is_multiple_of(FERNET_BLOCK_BYTES)
}

fn discard_reasoning_history(body: &mut Value) -> bool {
    let Some(input) = body.get_mut("input") else {
        return false;
    };
    match input {
        Value::Array(items) => {
            let previous_len = items.len();
            items.retain(|item| item.get("type").and_then(Value::as_str) != Some("reasoning"));
            items.len() != previous_len
        }
        Value::Object(_) if input.get("type").and_then(Value::as_str) == Some("reasoning") => {
            *input = Value::Array(Vec::new());
            true
        }
        _ => false,
    }
}

// Only in-flight work is owned here. Codex remains responsible for history
// versions, retries and installing a successful compaction result.
pub(crate) struct CompactionGuard {
    bindings: Arc<Mutex<RouteBindings>>,
    keys: Vec<String>,
}

impl CompactionGuard {
    pub(crate) fn acquire(bindings: &Arc<Mutex<RouteBindings>>, keys: Vec<String>) -> Result<Self> {
        let mut state = bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if keys.iter().any(|key| state.compacting.contains(key)) {
            anyhow::bail!("同一会话已有压缩请求正在执行，请等待完成后重试");
        }
        state.compacting.extend(keys.iter().cloned());
        Ok(Self {
            bindings: Arc::clone(bindings),
            keys,
        })
    }
}

impl Drop for CompactionGuard {
    fn drop(&mut self) {
        let mut state = self
            .bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for key in &self.keys {
            state.compacting.remove(key);
        }
    }
}

impl RouterServer {
    pub(crate) async fn proxy_with_compaction_budget<D: ResponsesDownstream + ?Sized>(
        &self,
        request: HttpRequest,
        body: Value,
        encoded_body: Option<Vec<u8>>,
        kind: ResponsesRequestKind,
        downstream: &mut D,
    ) -> Result<()> {
        if !is_compaction_request(&body, kind) {
            return self
                .proxy_parsed_responses_inner(request, body, encoded_body, kind, downstream)
                .await;
        }
        let keys = request_binding_keys(&request);
        // ponytail: without a session identifier only identical requests can be
        // deduplicated; a host revision is required for stronger idempotency.
        let keys = if keys.is_empty() {
            let bytes = serde_json::to_vec(&body)?;
            vec![format!("compact-input:{:x}", Sha256::digest(bytes))]
        } else {
            keys
        };
        let _guard = match CompactionGuard::acquire(&self.bindings, keys) {
            Ok(guard) => guard,
            Err(error) => {
                return downstream
                    .write_error(409, "compaction_in_progress", error.to_string(), None)
                    .await;
            }
        };
        // 压缩结果必须完整校验后才能写回下游：等待上游期间不能先开始 SSE 或
        // HTTP 响应，否则失败时只能在已经开始的响应里追加 JSON 错误。
        self.proxy_parsed_responses_inner(request, body, encoded_body, kind, downstream)
            .await
    }
}

pub(crate) fn validate_compaction_result(value: &Value, v2: bool) -> Result<()> {
    check_context_length_error(value)?;
    // A valid candidate must also fit in the next request. Ciphertext byte
    // length cannot establish token count or semantic quality; Codex rechecks
    // the target model budget before installing/sending its history.
    bounded_json_bytes(value, MAX_REQUEST_BYTES)?;
    if v2 && value.get("status").and_then(Value::as_str) != Some("completed") {
        anyhow::bail!("远程压缩未成功完成");
    }
    let output = value
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("远程压缩缺少 output 数组"))?;
    let mut count = 0;
    for item in output {
        if item.get("type").and_then(Value::as_str).is_none() {
            anyhow::bail!("远程压缩包含无效的输出项");
        }
        if item["type"] == "compaction" {
            count += 1;
            if item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_none_or(|s| s.trim().is_empty())
            {
                anyhow::bail!("远程压缩缺少有效的 encrypted_content");
            }
        }
    }
    if count != 1 {
        anyhow::bail!("远程压缩必须返回且仅返回一个 compaction 项，实际收到 {count} 个");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fernet_token(ciphertext: &[u8]) -> String {
        let mut token = vec![0x80];
        token.extend_from_slice(&1_700_000_000u64.to_be_bytes());
        token.extend_from_slice(&[0x11; 16]);
        token.extend_from_slice(ciphertext);
        token.extend_from_slice(&[0x22; 32]);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
    }

    fn agent_message(content: Value) -> Value {
        json!({
            "type":"agent_message",
            "id":"amsg_test",
            "author":"/root",
            "recipient":"/root/child",
            "content":content
        })
    }

    #[test]
    fn native_reasoning_normalization_preserves_same_route_history() {
        for encrypted in [None, Some("opaque-state")] {
            let mut reasoning = json!({
                "type":"reasoning", "id":"rs_provider", "summary":[],
                "content":[{"type":"reasoning_text","text":"检查工具结果。Next step 🙂"}]
            });
            if let Some(encrypted) = encrypted {
                reasoning["encrypted_content"] = json!(encrypted);
            }
            let user = json!({"role":"user","content":"continue"});
            for input in [reasoning.clone(), json!([reasoning, user.clone()])] {
                let original = json!({"input":input});
                let mut body = original.clone();
                assert!(!normalize_native_responses_context(&mut body, false));
                assert_eq!(body, original);

                assert!(normalize_native_responses_context(&mut body, true));
                assert_eq!(
                    body["input"],
                    if input.is_array() {
                        json!([user])
                    } else {
                        json!([])
                    }
                );
                assert!(!normalize_native_responses_context(&mut body, true));
            }
        }
    }

    #[test]
    fn missing_reasoning_text_gets_placeholder() {
        let mut body = json!({
            "input":[
                {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"},
                {"type":"reasoning","id":"rs_2","summary":[],"encrypted_content":"opaque",
                 "content":[{"type":"reasoning_text","text":"真实明文"}]},
                {"type":"reasoning","id":"rs_3","summary":[],"encrypted_content":"opaque","content":[]},
                {"role":"user","content":[{"type":"input_text","text":"继续"}]}
            ]
        });
        assert!(fill_missing_reasoning_text(&mut body));
        assert_eq!(
            body["input"][0]["content"],
            json!([{"type":"reasoning_text","text":MISSING_REASONING_TEXT_PLACEHOLDER}])
        );
        assert_eq!(
            body["input"][1]["content"],
            json!([{"type":"reasoning_text","text":"真实明文"}])
        );
        assert_eq!(
            body["input"][2]["content"],
            json!([{"type":"reasoning_text","text":MISSING_REASONING_TEXT_PLACEHOLDER}])
        );
        assert_eq!(
            body["input"][3],
            json!({"role":"user","content":[{"type":"input_text","text":"继续"}]})
        );
        // 补齐后的请求再次经过时保持字节不变。
        assert!(!fill_missing_reasoning_text(&mut body));
    }

    #[test]
    fn blank_reasoning_text_is_replaced_and_other_inputs_are_kept() {
        let mut single = json!({
            "input":{"type":"reasoning","id":"rs_1","summary":[],
                     "content":[{"type":"reasoning_text","text":"   "}]}
        });
        assert!(fill_missing_reasoning_text(&mut single));
        assert_eq!(
            single["input"]["content"],
            json!([{"type":"reasoning_text","text":MISSING_REASONING_TEXT_PLACEHOLDER}])
        );

        let mut text_plaintext = json!({
            "input":[{"type":"reasoning","id":"rs_2","summary":[],
                      "content":[{"type":"text","text":"第三方明文"}]}]
        });
        assert!(!fill_missing_reasoning_text(&mut text_plaintext));

        let mut plain = json!({"input":"没有 reasoning 项"});
        let original = plain.clone();
        assert!(!fill_missing_reasoning_text(&mut plain));
        assert_eq!(plain, original);
    }

    #[test]
    fn stripped_assistant_turn_gets_a_reasoning_placeholder() {
        let mut body = json!({
            "input":[
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"先看文件"}]},
                {"type":"function_call","call_id":"call-1","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"call-1","output":"done"},
                {"type":"function_call","call_id":"call-2","name":"lookup","arguments":"{}"},
                {"role":"user","content":[{"type":"input_text","text":"继续"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"第二轮"}]}
            ]
        });
        assert!(fill_missing_reasoning_text(&mut body));
        assert_eq!(body["input"][0]["type"], "reasoning");
        assert_eq!(
            body["input"][0]["content"],
            json!([{"type":"reasoning_text","text":MISSING_REASONING_TEXT_PLACEHOLDER}])
        );
        assert_eq!(body["input"][1]["type"], "message");
        assert_eq!(body["input"][3]["type"], "function_call_output");
        assert_eq!(body["input"][4]["type"], "function_call");
        assert_eq!(body["input"][6]["type"], "reasoning");
        assert_eq!(
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "reasoning")
                .count(),
            2
        );
        assert!(!fill_missing_reasoning_text(&mut body));

        let error = br#"{"error":{"message":"{\"code\":11155,\"msg\":\"the reasoning content from the previous turn must be passed back in thinking mode\",\"extError\":{\"code\":\"reasoning_content_missing\"}}","type":"invalid_request_error"}}"#;
        assert!(requires_reasoning_text_fallback(error));
        assert!(!requires_reasoning_text_fallback(
            br#"{"error":{"message":"quota exceeded"}}"#
        ));
    }

    #[test]
    fn chat_assistant_messages_get_reasoning_content_placeholder() {
        let mut body = json!({
            "messages":[
                {"role":"user","content":"继续"},
                {"role":"assistant","content":"先看文件"},
                {"role":"assistant","content":"已有推理","reasoning_content":"真实明文"},
                {"role":"assistant","content":"空白","reasoning_content":"  "}
            ]
        });
        assert!(fill_missing_reasoning_text(&mut body));
        assert_eq!(
            body["messages"][1]["reasoning_content"],
            MISSING_REASONING_TEXT_PLACEHOLDER
        );
        assert_eq!(body["messages"][2]["reasoning_content"], "真实明文");
        assert_eq!(
            body["messages"][3]["reasoning_content"],
            MISSING_REASONING_TEXT_PLACEHOLDER
        );
        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert!(!fill_missing_reasoning_text(&mut body));
    }

    #[test]
    fn plaintext_agent_payloads_become_visible_text_without_route_change() {
        let task = "只读冒烟任务（第 1 轮）。禁止派生任何子代理。";
        let mut body = json!({
            "input":[
                {"role":"user","content":"continue"},
                agent_message(json!([
                    {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
                    {"type":"encrypted_content","encrypted_content":task}
                ]))
            ]
        });
        let expected_content = json!([
            {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
            {"type":"input_text","text":task}
        ]);

        assert!(normalize_native_responses_context(&mut body, false));
        assert_eq!(body["input"][1]["content"], expected_content);
        assert_eq!(
            body["input"][0],
            json!({"role":"user","content":"continue"})
        );
        assert!(!normalize_native_responses_context(&mut body, false));

        // 线路切换会额外丢弃 reasoning，但已经改写的任务正文保持不变。
        assert!(!normalize_native_responses_context(&mut body, true));
        assert_eq!(body["input"][1]["content"], expected_content);
    }

    #[test]
    fn encrypted_agent_payloads_are_preserved_byte_for_byte() {
        let mut body = json!({
            "input":[agent_message(json!([
                {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
                {"type":"encrypted_content","encrypted_content":fernet_token(&[0x33; 16])}
            ]))]
        });
        let original = body.clone();

        assert!(!normalize_native_responses_context(&mut body, false));
        assert_eq!(body, original);
        assert!(
            ProtocolBridge::NativeResponses
                .convert_responses_body(&body)
                .unwrap()
                .is_none()
        );
        assert!(validate_cross_route_context(&body).is_ok());
    }

    #[test]
    fn adapted_agent_payloads_reject_encrypted_tasks_without_exposing_them() {
        let token = fernet_token(&[0x33; 16]);
        let part = json!({"type":"encrypted_content","encrypted_content":token});
        for content in [
            part.clone(),
            json!([
                {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
                part
            ]),
        ] {
            let item = agent_message(content);
            for input in [item.clone(), json!([item])] {
                let mut body = json!({"model":"model","input":input});
                let original = body.clone();
                assert!(!normalize_encrypted_agent_payloads(&mut body));
                for bridge in [
                    ProtocolBridge::ResponsesToChatCompletions,
                    ProtocolBridge::ResponsesToAnthropicMessages,
                ] {
                    let error = bridge
                        .convert_responses_body(&body)
                        .err()
                        .unwrap()
                        .to_string();
                    assert!(error.contains("context_not_portable"));
                    assert!(!error.contains(&token));
                }
                assert_eq!(body, original);
            }
        }
    }

    #[test]
    fn adapted_agent_payloads_preserve_recovered_plaintext() {
        let task = "检查测试结果并报告问题";
        let mut body = json!({"model":"model","input":[agent_message(json!([
            {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
            {"type":"encrypted_content","encrypted_content":task}
        ]))]});
        assert!(normalize_encrypted_agent_payloads(&mut body));
        for bridge in [
            ProtocolBridge::ResponsesToChatCompletions,
            ProtocolBridge::ResponsesToAnthropicMessages,
        ] {
            let converted = bridge.convert_responses_body(&body).unwrap().unwrap();
            assert!(converted.body["messages"].to_string().contains(task));
        }
    }

    #[test]
    fn adapted_agent_payloads_allow_unrelated_opaque_state() {
        let token = fernet_token(&[0x44; 16]);
        let body = json!({"model":"model","input":[
            {"type":"reasoning","encrypted_content":token,
             "content":[{"type":"encrypted_content","encrypted_content":token}]},
            {"role":"user","content":[
                {"type":"input_text","text":"continue"},
                {"type":"encrypted_content","encrypted_content":token}
            ]}
        ]});
        for bridge in [
            ProtocolBridge::ResponsesToChatCompletions,
            ProtocolBridge::ResponsesToAnthropicMessages,
        ] {
            assert!(bridge.convert_responses_body(&body).is_ok());
        }
    }

    #[test]
    fn empty_or_unreadable_agent_payloads_are_dropped() {
        for payload in [json!(""), json!("   "), json!(17), json!(null)] {
            let mut body = json!({"input":[agent_message(json!([
                {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
                {"type":"encrypted_content","encrypted_content":payload}
            ]))]});
            assert!(normalize_native_responses_context(&mut body, false));
            assert_eq!(
                body["input"][0]["content"],
                json!([{"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"}])
            );
        }
    }

    #[test]
    fn single_object_agent_content_keeps_the_rewritten_payload() {
        let mut plaintext = json!({
            "input":[agent_message(json!({
                "type":"encrypted_content","encrypted_content":"独立验证任务：只读。"
            }))]
        });
        assert!(normalize_native_responses_context(&mut plaintext, false));
        // 单个对象形态只替换内容本身，不额外改变内容结构。
        assert_eq!(
            plaintext["input"][0]["content"],
            json!({"type":"input_text","text":"独立验证任务：只读。"})
        );

        let mut empty = json!({
            "input":[agent_message(json!({"type":"encrypted_content","encrypted_content":""}))]
        });
        assert!(normalize_native_responses_context(&mut empty, false));
        assert!(empty["input"][0].get("content").is_none());
    }

    #[test]
    fn non_agent_items_keep_their_encrypted_state() {
        let mut body = json!({
            "input":[
                {"type":"reasoning","id":"rs_1","encrypted_content":"第三方线路的推理状态"},
                {"role":"user","content":[{"type":"encrypted_content","encrypted_content":"未识别的普通内容"}]}
            ]
        });
        let original = body.clone();

        assert!(!normalize_native_responses_context(&mut body, false));
        assert_eq!(body, original);
    }

    #[test]
    fn cross_route_context_rejects_nested_item_reference() {
        let body = json!({
            "input": [{
                "role": "assistant",
                "content": [{"type": "output_text", "annotations": [{"type": "item_reference"}]}]
            }]
        });
        assert!(validate_cross_route_context(&body).is_err());
    }

    #[test]
    fn compaction_guard_releases_on_failure_and_serializes_overlapping_sessions() {
        let bindings = Arc::new(Mutex::new(RouteBindings::default()));
        let guard =
            CompactionGuard::acquire(&bindings, vec!["thread:a".into(), "session:a".into()])
                .unwrap();
        assert!(CompactionGuard::acquire(&bindings, vec!["session:a".into()]).is_err());
        let other = CompactionGuard::acquire(&bindings, vec!["thread:b".into()]).unwrap();
        drop(guard);
        assert!(CompactionGuard::acquire(&bindings, vec!["thread:a".into()]).is_ok());
        drop(other);
        assert!(bindings.lock().unwrap().compacting.is_empty());
    }

    #[test]
    fn compaction_candidate_requires_complete_single_nonempty_snapshot() {
        let valid = json!({"status":"completed","output":[{"type":"compaction","encrypted_content":"opaque"}]});
        assert!(validate_compaction_result(&valid, true).is_ok());
        for invalid in [
            json!({"output":[]}),
            json!({"status":"incomplete","output":valid["output"]}),
            json!({"status":"completed","output":[{"type":"compaction","encrypted_content":""}]}),
            json!({"status":"completed","output":[valid["output"][0],valid["output"][0]]}),
            json!({"status":"completed","output":[{"type":"message","content":"summary"}]}),
        ] {
            assert!(validate_compaction_result(&invalid, true).is_err());
        }
        let mut accumulator = CompactionAccumulator::default();
        assert!(parse_sse_frames(b"data: [DONE]\n\n", &mut accumulator).is_err());
        assert!(!accumulator.finished());
        let failure = json!({"type":"response.failed","response":{"error":{"code":"context_length_exceeded"}}});
        assert!(
            accumulator
                .ingest_frame(&failure.to_string(), false)
                .unwrap_err()
                .is::<ContextLengthExceeded>()
        );
        let oversized = json!({"status":"completed","output":[{"type":"compaction","encrypted_content":"x".repeat(MAX_REQUEST_BYTES)}]});
        assert!(validate_compaction_result(&oversized, true).is_err());
    }

    #[test]
    fn compaction_sse_restores_done_items_only_after_successful_completion() {
        let item = json!({"type":"compaction","encrypted_content":"opaque"});
        let message = json!({"type":"message","role":"user","content":[]});
        for response in [json!({"id":"resp"}), json!({"id":"resp","output":[]})] {
            let mut accumulator = CompactionAccumulator::default();
            // Preserve output order even when done events arrive out of order.
            for (index, value) in [(1, &item), (0, &message)] {
                accumulator.ingest_frame(&json!({"type":"response.output_item.done","output_index":index,"item":value}).to_string(), false).unwrap();
            }
            assert!(!accumulator.finished());
            accumulator
                .ingest_frame(
                    &json!({"type":"response.completed","response":response}).to_string(),
                    false,
                )
                .unwrap();
            let result = accumulator.response.unwrap();
            assert_eq!(result["status"], "completed");
            assert_eq!(result["output"], json!([message, item]));
        }
        for (event_type, items, terminal) in [
            (
                "response.output_item.added",
                vec![item.clone()],
                json!({"type":"response.completed","response":{"output":[]}}),
            ),
            (
                "response.output_item.done",
                vec![item.clone(), item.clone()],
                json!({"type":"response.completed","response":{"output":[]}}),
            ),
            (
                "response.output_item.done",
                vec![json!({"type":"compaction","encrypted_content":""})],
                json!({"type":"response.completed","response":{"output":[]}}),
            ),
            (
                "response.output_item.done",
                vec![item.clone()],
                json!({"type":"response.incomplete","response":{}}),
            ),
            (
                "response.output_item.done",
                vec![item.clone()],
                json!({"type":"response.completed","response":null}),
            ),
            (
                "response.output_item.done",
                vec![item.clone()],
                json!({"type":"response.completed","response":{"output":[message]}}),
            ),
        ] {
            let mut accumulator = CompactionAccumulator::default();
            for (index, value) in items.iter().enumerate() {
                accumulator
                    .ingest_frame(
                        &json!({"type":event_type,"output_index":index,"item":value}).to_string(),
                        false,
                    )
                    .unwrap();
            }
            assert!(
                accumulator
                    .ingest_frame(&terminal.to_string(), false)
                    .is_err()
            );
            assert!(!accumulator.finished());
        }
        let mut accumulator = CompactionAccumulator::default();
        accumulator
            .ingest_frame(
                &json!({"type":"response.completed","response":{"output":[item]}}).to_string(),
                false,
            )
            .unwrap();
        assert!(accumulator.finished());
    }

    #[test]
    fn compaction_sse_rejects_conflicting_or_missing_output_items() {
        let item = json!({"type":"compaction","encrypted_content":"opaque"});
        let done = |index, item: Value| {
            json!({"type":"response.output_item.done","output_index":index,"item":item}).to_string()
        };
        let complete = json!({"type":"response.completed","response":{"output":[]}}).to_string();
        let mut duplicate = CompactionAccumulator::default();
        duplicate
            .ingest_frame(&done(0, item.clone()), false)
            .unwrap();
        duplicate
            .ingest_frame(&done(0, item.clone()), false)
            .unwrap();
        assert!(
            duplicate
                .ingest_frame(&done(0, json!({"type":"message"})), false)
                .is_err()
        );
        let mut missing = CompactionAccumulator::default();
        missing.ingest_frame(&done(1, item.clone()), false).unwrap();
        assert!(missing.ingest_frame(&complete, false).is_err());
        let mut unfinished = CompactionAccumulator::default();
        unfinished.ingest_frame(&done(0, item), false).unwrap();
        unfinished.ingest_frame(&json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message"}}).to_string(), false).unwrap();
        assert!(unfinished.ingest_frame(&complete, false).is_err());
    }

    #[test]
    fn compaction_tool_parts_do_not_restore_filtered_ciphertext() {
        for mixed in [
            json!([{"type":"encrypted_content","encrypted_content":"secret"},{"type":"unknown","value":1}]),
            json!([17,{"type":"encrypted_content","encrypted_content":"secret"}]),
            json!([{"type":"text","text":"visible"},{"custom":"json"}]),
            json!([{"type":"compaction","encrypted_content":"secret"}]),
        ] {
            assert!(responses_tool_output_content(&mixed).is_err());
        }
        assert!(
            responses_tool_output_content(&json!({"type":"custom_json","value":17}))
                .unwrap()
                .is_none()
        );
        assert_eq!(responses_tool_output_content(&json!([{"type":"text","text":"visible"},{"type":"reasoning","encrypted_content":"secret"}])).unwrap().unwrap().0, "visible");
    }

    #[test]
    fn compaction_context_errors_survive_json_and_stream_adapters() {
        for error in [
            json!({"error":{"code":"context_length_exceeded","message":"limit"}}),
            json!({"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 100 > 50"}}),
        ] {
            assert!(
                check_context_length_error(&error)
                    .unwrap_err()
                    .is::<ContextLengthExceeded>()
            );
            assert!(
                ChatSseAccumulator::new("model")
                    .ingest(&error)
                    .unwrap_err()
                    .is::<ContextLengthExceeded>()
            );
            assert!(
                AnthropicSseAccumulator::new("model")
                    .ingest(&error)
                    .unwrap_err()
                    .is::<ContextLengthExceeded>()
            );
            assert!(
                chat_completion_to_responses_body(error.clone(), "model")
                    .unwrap_err()
                    .is::<ContextLengthExceeded>()
            );
            assert!(
                anthropic_message_to_responses_body(&error, "model")
                    .unwrap_err()
                    .is::<ContextLengthExceeded>()
            );
        }
        assert!(!is_context_length_error(
            &json!({"error":{"message":"image too large","code":"invalid_request_error"}})
        ));
    }
}

#[derive(Default)]
struct CompactionAccumulator {
    response: Option<Value>,
    output: BTreeMap<u64, Value>,
    output_count: u64,
}
impl SseFrameAccumulator for CompactionAccumulator {
    const PROTOCOL_LABEL: &'static str = "Responses compaction";
    const READ_OPERATION: &'static str = "读取远程压缩响应失败";
    // A compaction item carries the complete encrypted snapshot in one frame.
    // collect_sse_frames still enforces the cumulative response/memory budgets.
    const MAX_BUFFER_BYTES: usize = MAX_UPSTREAM_RESPONSE_BYTES;
    fn ingest_frame(&mut self, data: &str, trailing: bool) -> Result<()> {
        let event = sse_json_frame(data, Self::PROTOCOL_LABEL, trailing)?;
        check_context_length_error(&event)?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_item.added" | "response.output_item.done") => {
                let index = event["output_index"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("远程压缩输出项缺少有效的 output_index"))?;
                self.output_count = self.output_count.max(
                    index
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("远程压缩 output_index 超过上限"))?,
                );
                if event["type"] == "response.output_item.added" {
                    return Ok(());
                }
                if let Some(previous) = self.output.get(&index)
                    && previous != &event["item"]
                {
                    anyhow::bail!("远程压缩包含冲突的 output_index");
                }
                self.output.insert(index, event["item"].clone());
            }
            Some("response.completed") => {
                let mut response = event["response"].clone();
                if !response.is_object() {
                    anyhow::bail!("远程压缩完成事件缺少有效的 response");
                }
                // Completed events may omit the output already sent in item.done.
                // Never restore item.added: its encrypted content may be incomplete.
                if response.get("output").is_none()
                    || response["output"].as_array().is_some_and(Vec::is_empty)
                {
                    if self.output.keys().copied().ne(0..self.output_count) {
                        anyhow::bail!("远程压缩输出项不连续，无法恢复完整结果");
                    }
                    response["output"] =
                        Value::Array(std::mem::take(&mut self.output).into_values().collect());
                }
                // The event itself establishes completion, even if a provider
                // omits the redundant response.status field.
                if response.get("status").is_none() {
                    response["status"] = json!("completed");
                }
                validate_compaction_result(&response, true)?;
                self.response = Some(response);
            }
            Some("response.failed" | "response.incomplete" | "error") => {
                anyhow::bail!("远程压缩返回失败或未完成终态")
            }
            _ => {}
        }
        Ok(())
    }
    fn finished(&self) -> bool {
        self.response.is_some()
    }
}

pub(crate) async fn write_validated_compaction<D: ResponsesDownstream + ?Sized>(
    downstream: &mut D,
    response: reqwest::Response,
    v2: bool,
    stream: bool,
    route: &RouteTarget,
) -> Result<()> {
    let probe = downstream.request_log_probe().cloned();
    let result = await_upstream(downstream, async {
        let mut prepared =
            prepare_upstream_response(response, "读取远程压缩响应失败", probe.as_ref()).await?;
        if prepared.is_sse {
            if !v2 {
                anyhow::bail!("旧版压缩端点必须返回 JSON");
            }
            let mut accumulator = CompactionAccumulator::default();
            collect_sse_frames(&mut prepared, &mut accumulator, probe.as_ref()).await?;
            accumulator
                .response
                .ok_or_else(|| anyhow::anyhow!("远程压缩未返回完成事件"))
        } else {
            let bytes = read_bounded_prepared_upstream_body(
                &mut prepared,
                MAX_REQUEST_BYTES,
                "读取远程压缩响应失败",
                probe.as_ref(),
            )
            .await?;
            let value: Value = serde_json::from_slice(&bytes).context("远程压缩返回无效 JSON")?;
            validate_compaction_result(&value, v2)?;
            Ok(value)
        }
    })
    .await?;
    match result {
        Ok(value) if stream => write_responses_response_as_events(downstream, &value).await,
        Ok(value) => downstream.write_json(200, &value).await,
        Err(error) => {
            let timeout = is_upstream_timeout_error(&error);
            let (status, code) = if timeout {
                (504, "compaction_timeout")
            } else if error.is::<ContextLengthExceeded>() {
                (400, CONTEXT_LENGTH_EXCEEDED)
            } else {
                (502, "invalid_compaction_response")
            };
            // 上游断流、超时与本地校验失败此前都只打印最外层上下文，压缩失败
            // 因此难以定位；这里保留脱敏后的完整原因链。
            let detail = sanitize_upstream_error_text(&format!("{error:#}"), route, 512)
                .unwrap_or_else(|| "上游未返回可用的压缩结果".to_string());
            let message = if timeout {
                format!(
                    "远程压缩读取上游响应超时（{detail}），原始会话历史未被 Codey 修改，请稍后重试"
                )
            } else if status == 400 {
                error.to_string()
            } else {
                format!("远程压缩上游响应未能完成：{detail}")
            };
            downstream
                .write_error(status, code, message, Some(route))
                .await
        }
    }
}
