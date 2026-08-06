use std::path::Path;

use crate::config::{CodeyConfig, DEFAULT_SUBAGENT_MODEL, DEFAULT_SUBAGENT_REASONING_EFFORT};
use crate::model_catalog;

pub(crate) fn apply_after_provider_sync(
    previous_provider_id: Option<&str>,
    current_provider_id: &str,
    config: &mut CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) {
    if previous_provider_id != Some(current_provider_id) {
        reset_for_current_provider(config, codex_home, official_provider);
    } else {
        reconcile_for_current_provider(config, codex_home, official_provider);
    }
}

fn reset_for_current_provider(
    config: &mut CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) {
    let (model, reasoning_effort, compatible_model_available) =
        defaults_for_current_provider(config, codex_home, official_provider);
    config.subagent_model = model;
    config.subagent_reasoning_effort = reasoning_effort;
    if !compatible_model_available {
        config.subagent_optimization = false;
    }
}

pub(crate) fn reconcile_for_current_provider(
    config: &mut CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) {
    if config.subagent_optimization
        && !selected_model_available_for_current_provider(config, codex_home, official_provider)
    {
        reset_for_current_provider(config, codex_home, official_provider);
    }
}

fn defaults_for_current_provider(
    config: &CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) -> (String, String, bool) {
    let Ok(state) = model_catalog::selection_state_with_manual_models(
        codex_home,
        official_provider,
        config.upstream_models_snapshot(),
        config.selected_models(),
        config.manual_third_party_models(),
        Some(DEFAULT_SUBAGENT_MODEL),
    ) else {
        return (
            DEFAULT_SUBAGENT_MODEL.to_string(),
            DEFAULT_SUBAGENT_REASONING_EFFORT.to_string(),
            official_provider,
        );
    };
    let Some(model) = state
        .available_subagent_model(DEFAULT_SUBAGENT_MODEL)
        .or_else(|| state.available_subagent_model(&state.default_model))
        .or_else(|| state.first_available_subagent_model())
    else {
        return (
            DEFAULT_SUBAGENT_MODEL.to_string(),
            DEFAULT_SUBAGENT_REASONING_EFFORT.to_string(),
            false,
        );
    };
    let model = model.to_string();
    let reasoning_effort =
        reasoning_effort_for_model(&state, &model, &config.subagent_reasoning_effort);
    (model, reasoning_effort, true)
}

fn selected_model_available_for_current_provider(
    config: &CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) -> bool {
    let model = config.subagent_model.trim();
    if model.is_empty() {
        return false;
    }
    let Ok(state) = model_catalog::selection_state_with_manual_models(
        codex_home,
        official_provider,
        config.upstream_models_snapshot(),
        config.selected_models(),
        config.manual_third_party_models(),
        Some(DEFAULT_SUBAGENT_MODEL),
    ) else {
        return true;
    };
    state.available_subagent_model(model).is_some()
}

fn reasoning_effort_for_model(
    state: &model_catalog::ModelSelectionState,
    model: &str,
    preferred_reasoning_effort: &str,
) -> String {
    let preferred_reasoning_effort = preferred_reasoning_effort.trim().to_ascii_lowercase();
    if let Some(official_model) = state
        .official_models
        .iter()
        .find(|candidate| candidate.supported && candidate.slug == model)
    {
        if official_model
            .supported_reasoning_efforts
            .iter()
            .any(|effort| effort.eq_ignore_ascii_case(&preferred_reasoning_effort))
        {
            return preferred_reasoning_effort;
        }
        if official_model
            .supported_reasoning_efforts
            .iter()
            .any(|effort| effort == DEFAULT_SUBAGENT_REASONING_EFFORT)
        {
            return DEFAULT_SUBAGENT_REASONING_EFFORT.to_string();
        }
        if !official_model.default_reasoning_effort.trim().is_empty() {
            return official_model.default_reasoning_effort.clone();
        }
    }
    if state
        .third_party_models
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(model))
        && matches!(
            preferred_reasoning_effort.as_str(),
            "low" | "medium" | "high" | "xhigh"
        )
    {
        return preferred_reasoning_effort;
    }
    DEFAULT_SUBAGENT_REASONING_EFFORT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderProfile;

    fn route_config(provider_id: &str) -> CodeyConfig {
        let mut profile = ProviderProfile::new("Route");
        profile.id = provider_id.to_string();
        profile.cc_switch_read_only = false;
        CodeyConfig {
            active_profile_id: provider_id.to_string(),
            profiles: vec![profile],
            subagent_optimization: true,
            subagent_model: "provider-old-model".into(),
            ..CodeyConfig::default()
        }
    }

    #[test]
    fn provider_change_selects_a_compatible_model_and_reasoning_effort() {
        struct Case {
            upstream_models: &'static [&'static str],
            saved_effort: &'static str,
            expected_model: &'static str,
            expected_effort: &'static str,
            optimization_enabled: bool,
        }

        let cases = [
            Case {
                upstream_models: &[DEFAULT_SUBAGENT_MODEL],
                saved_effort: "xhigh",
                expected_model: DEFAULT_SUBAGENT_MODEL,
                expected_effort: "xhigh",
                optimization_enabled: true,
            },
            Case {
                upstream_models: &["gpt-5.6-sol"],
                saved_effort: "xhigh",
                expected_model: "gpt-5.6-sol",
                expected_effort: "xhigh",
                optimization_enabled: true,
            },
            Case {
                upstream_models: &["gpt-5.4"],
                saved_effort: "ultra",
                expected_model: "gpt-5.4",
                expected_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                optimization_enabled: true,
            },
            Case {
                upstream_models: &["provider-custom-model"],
                saved_effort: "high",
                expected_model: DEFAULT_SUBAGENT_MODEL,
                expected_effort: DEFAULT_SUBAGENT_REASONING_EFFORT,
                optimization_enabled: false,
            },
        ];

        for case in cases {
            let home = tempfile::tempdir().unwrap();
            let mut config = route_config("route-b");
            config.subagent_reasoning_effort = case.saved_effort.into();
            config.upstream_models_by_provider.insert(
                "route-b".into(),
                case.upstream_models
                    .iter()
                    .map(|model| (*model).to_string())
                    .collect(),
            );

            apply_after_provider_sync(Some("route-a"), "route-b", &mut config, home.path(), false);

            assert_eq!(config.subagent_model, case.expected_model);
            assert_eq!(config.subagent_reasoning_effort, case.expected_effort);
            assert_eq!(
                config.subagent_optimization, case.optimization_enabled,
                "unexpected optimization state for {}",
                case.expected_model
            );
        }
    }

    #[test]
    fn provider_change_accepts_a_selected_third_party_model() {
        let home = tempfile::tempdir().unwrap();
        let mut config = route_config("route-b");
        config.subagent_reasoning_effort = "high".into();
        config
            .selected_models_by_provider
            .insert("route-b".into(), vec!["provider-custom-model".into()]);
        config
            .upstream_models_by_provider
            .insert("route-b".into(), vec!["provider-custom-model".into()]);

        apply_after_provider_sync(Some("route-a"), "route-b", &mut config, home.path(), false);

        assert!(config.subagent_optimization);
        assert_eq!(config.subagent_model, "provider-custom-model");
        assert_eq!(config.subagent_reasoning_effort, "high");
    }

    #[test]
    fn unchanged_provider_only_reconciles_an_unavailable_model() {
        let available_home = tempfile::tempdir().unwrap();
        let mut available = route_config("route-a");
        available.subagent_model = "gpt-5.6-sol".into();
        available.subagent_reasoning_effort = "high".into();

        apply_after_provider_sync(
            Some("route-a"),
            "route-a",
            &mut available,
            available_home.path(),
            false,
        );

        assert!(available.subagent_optimization);
        assert_eq!(available.subagent_model, "gpt-5.6-sol");
        assert_eq!(available.subagent_reasoning_effort, "high");

        let unavailable_home = tempfile::tempdir().unwrap();
        let mut unavailable = route_config("route-a");
        unavailable.subagent_model = DEFAULT_SUBAGENT_MODEL.into();
        unavailable.subagent_reasoning_effort = "high".into();
        unavailable
            .upstream_models_by_provider
            .insert("route-a".into(), vec!["provider-custom-model".into()]);

        apply_after_provider_sync(
            Some("route-a"),
            "route-a",
            &mut unavailable,
            unavailable_home.path(),
            false,
        );

        assert!(!unavailable.subagent_optimization);
        assert_eq!(unavailable.subagent_model, DEFAULT_SUBAGENT_MODEL);
        assert_eq!(
            unavailable.subagent_reasoning_effort,
            DEFAULT_SUBAGENT_REASONING_EFFORT
        );
    }
}
