use super::*;

/// 子代理入站不带 `x-codex-turn-state`，上游会把每一次都当成新轮次。
/// 这里只记住每条线程最近一次成功响应的票据，并在该线程的子代理自己没带时补上。
/// 主会话请求不改写。
pub(crate) const TURN_STATE_HEADER: &str = "x-codex-turn-state";
const PARENT_THREAD_HEADER: &str = "x-codex-parent-thread-id";
const MAX_TURN_STATES: usize = 1024;
const MAX_TURN_STATE_BYTES: usize = 8 * 1024;

#[derive(Default)]
pub(crate) struct SubagentTurnStateCache {
    entries: HashMap<String, String>,
    order: VecDeque<String>,
}

impl SubagentTurnStateCache {
    /// 请求已经带了票据时保持原值。找不到这条线程的成功票据时不补。
    pub(crate) fn apply_if_absent(&mut self, headers: &mut HeaderMap) -> bool {
        if headers.contains_key(TURN_STATE_HEADER) {
            return false;
        }
        let Some(thread_id) = continuation_thread(headers) else {
            return false;
        };
        let Some(value) = self.entries.get(&thread_id).cloned() else {
            return false;
        };
        let Ok(header) = HeaderValue::from_str(&value) else {
            return false;
        };
        headers.insert(HeaderName::from_static(TURN_STATE_HEADER), header);
        true
    }

    pub(crate) fn remember_response(
        &mut self,
        request_headers: &HeaderMap,
        response_headers: &HeaderMap,
    ) {
        let Some(state) = turn_state_from_headers(response_headers) else {
            return;
        };
        self.remember_value(request_headers, &state);
    }

    pub(crate) fn remember_value(&mut self, request_headers: &HeaderMap, state: &str) {
        let Some(state) = valid_turn_state(state) else {
            return;
        };
        let Some(thread_id) = continuation_thread(request_headers) else {
            return;
        };
        self.insert(thread_id, state.to_string());
    }

    /// 上游拒绝了这次带出去的票据时，丢掉这条线程的续接票据。
    pub(crate) fn evict_if_current(&mut self, request_headers: &HeaderMap) {
        let Some(sent) = turn_state_from_headers(request_headers) else {
            return;
        };
        let Some(thread_id) = continuation_thread(request_headers) else {
            return;
        };
        if self
            .entries
            .get(&thread_id)
            .is_some_and(|entry| entry == &sent)
        {
            self.entries.remove(&thread_id);
            self.order.retain(|existing| existing != &thread_id);
        }
    }

    fn insert(&mut self, thread_id: String, value: String) {
        if let Some(index) = self
            .order
            .iter()
            .position(|existing| existing == &thread_id)
        {
            self.order.remove(index);
        }
        self.entries.insert(thread_id.clone(), value);
        self.order.push_back(thread_id);
        while self.entries.len() > MAX_TURN_STATES {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&expired);
        }
    }
}

pub(crate) fn reuse_subagent_turn_state(
    cache: &Mutex<SubagentTurnStateCache>,
    headers: &mut HeaderMap,
) {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .apply_if_absent(headers);
}

pub(crate) fn remember_observed_turn_state(
    cache: &Mutex<SubagentTurnStateCache>,
    request_headers: &HeaderMap,
    state: &str,
) {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remember_value(request_headers, state);
}

/// 2xx 响应更新这条线程的续接票据。401、403、409 只丢掉这次实际发出的那一张。
pub(crate) fn observe_upstream_turn_state(
    cache: &Mutex<SubagentTurnStateCache>,
    request_headers: &HeaderMap,
    status: u16,
    response_headers: &HeaderMap,
) {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(status, 401 | 403 | 409) {
        cache.evict_if_current(request_headers);
        return;
    }
    if (200..300).contains(&status) {
        cache.remember_response(request_headers, response_headers);
    }
}

pub(crate) fn turn_state_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(TURN_STATE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(valid_turn_state)
        .map(str::to_owned)
}

pub(crate) fn turn_state_from_metadata_event(event: &Value) -> Option<&str> {
    if event.get("type").and_then(Value::as_str) != Some("codex.response.metadata") {
        return None;
    }
    let headers = event.get("headers")?.as_object()?;
    headers.iter().find_map(|(name, value)| {
        if !name.eq_ignore_ascii_case(TURN_STATE_HEADER) {
            return None;
        }
        value.as_str().and_then(valid_turn_state)
    })
}

/// 子代理归到父线程，这样同一次父轮次里的子代理共用这张续接票据。
fn continuation_thread(headers: &HeaderMap) -> Option<String> {
    header_value(headers, PARENT_THREAD_HEADER)
        .or_else(|| header_value(headers, "thread-id"))
        .or_else(|| header_value(headers, "thread_id"))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| valid_cache_component(value))
        .map(str::to_owned)
}

fn valid_cache_component(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_turn_state(value: &str) -> Option<&str> {
    let value = value.trim();
    (value.len() <= MAX_TURN_STATE_BYTES
        && !value.is_empty()
        && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)))
    .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_headers(thread_id: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("thread-id"),
            HeaderValue::from_str(thread_id).unwrap(),
        );
        headers
    }

    fn child_headers(parent_thread_id: &str) -> HeaderMap {
        let mut headers = thread_headers("child-thread");
        headers.insert(
            HeaderName::from_static(PARENT_THREAD_HEADER),
            HeaderValue::from_str(parent_thread_id).unwrap(),
        );
        headers
    }

    fn response_with_state(state: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(TURN_STATE_HEADER),
            HeaderValue::from_str(state).unwrap(),
        );
        headers
    }

    #[test]
    fn subagent_receives_its_parent_threads_turn_state() {
        let mut cache = SubagentTurnStateCache::default();
        let parent = thread_headers("parent-thread");
        cache.remember_response(&parent, &response_with_state("parent-turn"));

        let mut child = child_headers("parent-thread");
        assert!(cache.apply_if_absent(&mut child));
        assert_eq!(child[TURN_STATE_HEADER], "parent-turn");

        let mut other = child_headers("other-parent");
        assert!(!cache.apply_if_absent(&mut other));
        assert!(!other.contains_key(TURN_STATE_HEADER));
    }

    #[test]
    fn a_later_success_updates_only_that_thread() {
        let mut cache = SubagentTurnStateCache::default();
        let parent = thread_headers("parent-thread");
        cache.remember_response(&parent, &response_with_state("first-turn"));
        cache.remember_value(&child_headers("parent-thread"), "next-turn");

        let mut child = child_headers("parent-thread");
        assert!(cache.apply_if_absent(&mut child));
        assert_eq!(child[TURN_STATE_HEADER], "next-turn");

        let other = thread_headers("other-thread");
        cache.remember_response(&other, &response_with_state("other-turn"));
        let mut same_parent = child_headers("parent-thread");
        assert!(cache.apply_if_absent(&mut same_parent));
        assert_eq!(same_parent[TURN_STATE_HEADER], "next-turn");
    }

    #[test]
    fn existing_turn_state_is_left_unchanged() {
        let mut cache = SubagentTurnStateCache::default();
        cache.remember_response(
            &thread_headers("parent-thread"),
            &response_with_state("parent-turn"),
        );
        let mut child = child_headers("parent-thread");
        child.insert(
            HeaderName::from_static(TURN_STATE_HEADER),
            HeaderValue::from_static("client-ticket"),
        );
        assert!(!cache.apply_if_absent(&mut child));
        assert_eq!(child[TURN_STATE_HEADER], "client-ticket");
    }

    #[test]
    fn rejected_continuation_is_dropped_and_body_errors_are_kept() {
        let cache = Mutex::new(SubagentTurnStateCache::default());
        let parent = thread_headers("parent-thread");
        observe_upstream_turn_state(&cache, &parent, 200, &response_with_state("parent-turn"));
        let mut sent = child_headers("parent-thread");
        assert!(cache.lock().unwrap().apply_if_absent(&mut sent));

        observe_upstream_turn_state(&cache, &sent, 400, &HeaderMap::new());
        let mut after_body_error = child_headers("parent-thread");
        assert!(cache.lock().unwrap().apply_if_absent(&mut after_body_error));

        observe_upstream_turn_state(&cache, &sent, 403, &HeaderMap::new());
        let mut after_reject = child_headers("parent-thread");
        assert!(!cache.lock().unwrap().apply_if_absent(&mut after_reject));
    }

    #[test]
    fn a_child_without_a_parent_thread_does_not_borrow_another_turn() {
        let mut cache = SubagentTurnStateCache::default();
        cache.remember_response(
            &thread_headers("parent-thread"),
            &response_with_state("parent-turn"),
        );
        let mut orphan = thread_headers("brand-new-child");
        assert!(!cache.apply_if_absent(&mut orphan));
    }

    #[test]
    fn metadata_event_yields_only_a_header_safe_turn_state() {
        let event = json!({
            "type": "codex.response.metadata",
            "headers": {"X-Codex-Turn-State": "ws-ticket"}
        });
        assert_eq!(turn_state_from_metadata_event(&event), Some("ws-ticket"));
        assert_eq!(
            turn_state_from_metadata_event(&json!({"type": "response.completed"})),
            None
        );
        assert_eq!(
            turn_state_from_metadata_event(&json!({
                "type": "codex.response.metadata",
                "headers": {"x-codex-turn-state": "has space"}
            })),
            None
        );
    }
}
