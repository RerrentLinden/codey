use std::collections::HashSet;

pub(crate) fn key(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

pub(crate) fn equal(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

pub(crate) fn dedupe_preserving_first<'a>(
    models: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    models
        .into_iter()
        .filter_map(|model| {
            let model = model.trim();
            let key = key(model);
            if key.is_empty() || !seen.insert(key) {
                return None;
            }
            Some(model.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_are_case_insensitive_but_keep_first_spelling() {
        assert!(equal(" Provider-A ", "provider-a"));
        assert_eq!(key(" Provider-A "), "provider-a");
        assert_eq!(
            dedupe_preserving_first([" Provider-A ", "provider-a", "Provider-B"]),
            ["Provider-A", "Provider-B"]
        );
    }
}
