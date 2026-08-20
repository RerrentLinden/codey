use serde_json::{Map, Value};

/// Runs the next hook only when the higher-priority hook produced no decision.
/// This keeps composition order explicit and prevents feature-specific routing
/// from leaking into the lifecycle gate.
pub(crate) fn first_decision(primary: Value, fallback: impl FnOnce() -> Value) -> Value {
    if primary.as_object().is_some_and(Map::is_empty) {
        fallback()
    } else {
        primary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn higher_priority_decision_short_circuits_fallback() {
        let selected = first_decision(json!({"decision": "block"}), || {
            panic!("fallback must not run")
        });
        assert_eq!(selected["decision"], "block");
    }

    #[test]
    fn empty_decision_delegates_to_the_next_hook() {
        assert_eq!(
            first_decision(json!({}), || json!({"route": "fastctx"})),
            json!({"route": "fastctx"})
        );
    }
}
