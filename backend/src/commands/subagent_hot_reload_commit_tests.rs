use super::subagent_hot_reload_commit_is_current;

#[test]
fn commits_only_to_the_same_live_runtime_and_current_config() {
    assert!(subagent_hot_reload_commit_is_current(
        false, false, 7, 7, true, true, false
    ));
    for stale in [
        (true, false, 7, 7, true, true, false),
        (false, true, 7, 7, true, true, false),
        (false, false, 7, 8, true, true, false),
        (false, false, 7, 7, false, true, false),
        (false, false, 7, 7, true, false, false),
        (false, false, 7, 7, true, true, true),
    ] {
        assert!(!subagent_hot_reload_commit_is_current(
            stale.0, stale.1, stale.2, stale.3, stale.4, stale.5, stale.6,
        ));
    }
}
