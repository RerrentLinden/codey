use super::super::preserve_cc_switch_route;
use super::*;
use crate::cc_switch::RouteTakeoverState;

#[test]
fn managed_route_with_a_broken_live_config_blocks_startup() {
    let error = preserve_cc_switch_route(RouteTakeoverState {
        managed: true,
        live: false,
    })
    .unwrap_err();

    assert!(format!("{error:#}").contains("关闭并重新开启 Codex 路由"));
}

#[test]
fn live_route_is_preserved_and_normal_config_is_not() {
    assert!(
        preserve_cc_switch_route(RouteTakeoverState {
            managed: true,
            live: true,
        })
        .unwrap()
    );
    assert!(!preserve_cc_switch_route(RouteTakeoverState::default()).unwrap());
}

#[test]
fn route_watcher_restarts_for_config_or_auth_changes() {
    let applied = RouteFilesSnapshot {
        config: b"model_provider = \"route-a\"\n".to_vec(),
        auth: Some(br#"{"OPENAI_API_KEY":"PROXY_MANAGED"}"#.to_vec()),
    };
    let same = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: applied.auth.clone(),
    };
    let changed_config = RouteFilesSnapshot {
        config: b"model_provider = \"route-b\"\n".to_vec(),
        auth: applied.auth.clone(),
    };
    let changed_auth = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: Some(br#"{"OPENAI_API_KEY":"new-key"}"#.to_vec()),
    };

    assert!(!route_files_changed(&applied, &same));
    assert!(route_files_changed(&applied, &changed_config));
    assert!(route_files_changed(&applied, &changed_auth));
}

#[test]
fn route_watcher_ignores_codex_config_normalization() {
    // Codey applies a compact route for provider "custom".
    let applied = RouteFilesSnapshot {
        config: b"model_provider = \"custom\"\n\
[model_providers.custom]\n\
name = \"custom\"\n\
base_url = \"https://api.example.com/v1\"\n\
wire_api = \"responses\"\n"
            .to_vec(),
        auth: Some(br#"{"OPENAI_API_KEY":"sk-custom"}"#.to_vec()),
    };
    // Codex rewrote config.toml on startup: reordered the provider table,
    // added a default field, removed the default `wire_api = "responses"`,
    // and added a trailing slash. The effective route is unchanged.
    let normalized = RouteFilesSnapshot {
        config: b"model_provider = \"custom\"\n\n\
[model_providers.custom]\n\
base_url = \"https://api.example.com/v1/\"\n\
name = \"custom\"\n\
env_key = \"OPENAI_API_KEY\"\n"
            .to_vec(),
        auth: applied.auth.clone(),
    };
    // A real route switch (different endpoint) still restarts Codex.
    let rerouted = RouteFilesSnapshot {
        config: b"model_provider = \"custom\"\n\
[model_providers.custom]\n\
base_url = \"https://api.other.com/v1\"\n\
wire_api = \"responses\"\n"
            .to_vec(),
        auth: applied.auth.clone(),
    };
    // Reformatting auth.json and refreshing unrelated ChatGPT account tokens
    // must not restart a third-party route.
    let reformatted_auth = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: Some(
            b"{\n  \"tokens\": {\"access_token\": \"refreshed\"},\n  \
\"OPENAI_API_KEY\": \"sk-custom\",\n  \"account_id\": \"account-b\"\n}\n"
                .to_vec(),
        ),
    };
    let rerouted_format_variant = RouteFilesSnapshot {
        config: b"model_provider=\"custom\"\n\
[model_providers.custom]\n\
wire_api=\"RESPONSES\"\n\
base_url=\"https://api.other.com/v1/\"\n"
            .to_vec(),
        auth: applied.auth.clone(),
    };

    assert!(!route_files_changed(&applied, &normalized));
    assert!(!route_files_changed(&applied, &reformatted_auth));
    assert!(route_files_changed(&applied, &rerouted));
    assert!(!route_config_changed(&applied.config, &normalized.config));
    assert!(!route_auth_changed(
        applied.auth.as_deref(),
        reformatted_auth.auth.as_deref()
    ));
    assert!(
        route_change_fingerprint(&rerouted) == route_change_fingerprint(&rerouted_format_variant)
    );
}

#[test]
fn route_watcher_restarts_for_effective_credential_changes() {
    let config_with_key = |key: &str| {
        format!(
            "model_provider = \"custom\"\n\
[model_providers.custom]\n\
base_url = \"https://api.example.com/v1\"\n\
wire_api = \"responses\"\n\
experimental_bearer_token = \"{key}\"\n"
        )
        .into_bytes()
    };
    let applied = RouteFilesSnapshot {
        config: config_with_key("sk-old"),
        auth: Some(
            br#"{"OPENAI_API_KEY":"PROXY_MANAGED","tokens":{"access_token":"old"}}"#.to_vec(),
        ),
    };
    let rotated_config_key = RouteFilesSnapshot {
        config: config_with_key("sk-new"),
        auth: applied.auth.clone(),
    };
    let rotated_auth_marker = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: Some(br#"{"OPENAI_API_KEY":"sk-direct"}"#.to_vec()),
    };
    let refreshed_account_token = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: Some(
            br#"{"OPENAI_API_KEY":"PROXY_MANAGED","tokens":{"access_token":"new"}}"#.to_vec(),
        ),
    };
    let redundant_auth_key = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: Some(br#"{"OPENAI_API_KEY":"sk-unused"}"#.to_vec()),
    };
    let redundant_auth_key_rotated = RouteFilesSnapshot {
        config: applied.config.clone(),
        auth: Some(br#"{"OPENAI_API_KEY":"sk-still-unused"}"#.to_vec()),
    };
    let auth_fallback_old = RouteFilesSnapshot {
        config: b"model_provider = \"custom\"\n\
[model_providers.custom]\n\
base_url = \"https://api.example.com/v1\"\n"
            .to_vec(),
        auth: Some(br#"{"OPENAI_API_KEY":"sk-old"}"#.to_vec()),
    };
    let auth_fallback_new = RouteFilesSnapshot {
        config: auth_fallback_old.config.clone(),
        auth: Some(br#"{"OPENAI_API_KEY":"sk-new"}"#.to_vec()),
    };

    assert!(route_files_changed(&applied, &rotated_config_key));
    assert!(route_files_changed(&applied, &rotated_auth_marker));
    assert!(!route_files_changed(&applied, &refreshed_account_token));
    assert!(!route_files_changed(
        &redundant_auth_key,
        &redundant_auth_key_rotated
    ));
    assert!(route_files_changed(&auth_fallback_old, &auth_fallback_new));
}

#[test]
fn route_watcher_ignores_an_auth_file_without_route_fields() {
    let config = b"model_provider = \"custom\"\n\
[model_providers.custom]\n\
base_url = \"https://api.example.com/v1\"\n\
wire_api = \"responses\"\n\
experimental_bearer_token = \"sk-custom\"\n"
        .to_vec();
    let without_auth = RouteFilesSnapshot {
        config: config.clone(),
        auth: None,
    };
    let account_only_auth = RouteFilesSnapshot {
        config,
        auth: Some(br#"{"tokens":{"access_token":"oauth"},"account_id":"account-a"}"#.to_vec()),
    };

    assert!(!route_files_changed(&without_auth, &account_only_auth));
}

#[test]
fn route_watcher_uses_raw_fallback_for_invalid_auth_shapes() {
    let config = b"model_provider = \"custom\"\n\
[model_providers.custom]\n\
base_url = \"https://api.example.com/v1\"\n"
        .to_vec();
    let array_auth = RouteFilesSnapshot {
        config: config.clone(),
        auth: Some(br#"["not","an","auth-object"]"#.to_vec()),
    };
    let numeric_auth = RouteFilesSnapshot {
        config,
        auth: Some(b"42".to_vec()),
    };

    assert!(route_files_changed(&array_auth, &numeric_auth));
}

#[tokio::test]
async fn route_file_stamps_change_only_when_route_files_change() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), b"model_provider = \"a\"\n").unwrap();
    let initial = read_route_file_stamps(home.path()).await.unwrap();

    assert_eq!(read_route_file_stamps(home.path()).await.unwrap(), initial);

    std::fs::write(
        home.path().join("auth.json"),
        br#"{"OPENAI_API_KEY":"key"}"#,
    )
    .unwrap();
    let with_auth = read_route_file_stamps(home.path()).await.unwrap();
    assert_ne!(with_auth, initial);

    std::fs::write(
        home.path().join("config.toml"),
        b"model_provider = \"provider-with-a-longer-id\"\n",
    )
    .unwrap();
    assert_ne!(
        read_route_file_stamps(home.path()).await.unwrap(),
        with_auth
    );

    std::fs::remove_file(home.path().join("config.toml")).unwrap();
    assert_eq!(read_route_file_stamps(home.path()).await.unwrap(), None);
}

#[test]
fn route_watcher_requires_a_stably_missing_config() {
    let mut missing_streak = 0;

    assert!(!observe_missing_route_config(&mut missing_streak));
    assert!(observe_missing_route_config(&mut missing_streak));
}

#[test]
fn route_watcher_rate_limits_repeated_read_errors() {
    let started_at = Instant::now();
    let mut limiter = RouteWatchErrorLimiter::new(started_at);

    assert_eq!(limiter.should_log("permission-denied", started_at), Some(0));
    assert_eq!(
        limiter.should_log("permission-denied", started_at + Duration::from_secs(1)),
        None
    );
    assert_eq!(
        limiter.should_log("permission-denied", started_at + Duration::from_secs(2)),
        Some(1)
    );
    assert_eq!(
        limiter.should_log("different-error", started_at + Duration::from_secs(2)),
        Some(0)
    );

    limiter.reset();
    assert_eq!(
        limiter.should_log("permission-denied", started_at + Duration::from_secs(3)),
        Some(0)
    );
}
