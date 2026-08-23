use super::*;

#[test]
fn packaged_activation_detects_a_reused_process_id() {
    let existing_process_ids = HashSet::from([41_u32, 42_u32]);

    assert!(activation_reused_existing_process(
        &existing_process_ids,
        42
    ));
    assert!(!activation_reused_existing_process(
        &existing_process_ids,
        43
    ));
}

#[test]
fn process_creation_identity_rejects_pid_reuse_when_timestamps_are_available() {
    assert!(process_creation_identity_matches(Some(100), Some(100)));
    assert!(!process_creation_identity_matches(Some(100), Some(101)));
    assert!(process_creation_identity_matches(Some(100), None));
    assert!(process_creation_identity_matches(None, Some(101)));
}

#[test]
fn official_provider_inherits_the_codex_builtin_model_catalog() {
    assert!(!should_install_codey_model_catalog(true, true));
    assert!(!should_install_codey_model_catalog(true, false));
}

#[test]
fn third_party_provider_installs_the_codey_model_catalog_when_available() {
    assert!(should_install_codey_model_catalog(false, true));
    assert!(!should_install_codey_model_catalog(false, false));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_launch_forces_a_new_app_instance() {
    let command = build_fresh_macos_open_command(
        std::path::Path::new("/Applications/ChatGPT.app"),
        9229,
        &["--inspect-brk=127.0.0.1:19321".to_string()],
    );
    assert_eq!(command.first().map(String::as_str), Some("open"));
    assert!(command.iter().any(|part| part == "-n"));
    assert!(command.iter().any(|part| part == "-W"));
    assert!(
        command
            .iter()
            .any(|part| part == "--remote-debugging-port=9229")
    );
    assert!(
        command
            .iter()
            .any(|part| part == "--inspect-brk=127.0.0.1:19321")
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_running_check_does_not_match_an_unrelated_app_path() {
    let running = macos_codex_is_running(std::path::Path::new(
        "/Applications/Definitely Not Codex.app",
    ))
    .await
    .unwrap();
    assert!(!running);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_running_check_matches_only_the_app_main_executable() {
    let processes = crate::process_tree::parse_unix_process_snapshot(
        b"100 1 100 Thu Jul 23 19:23:12 2026 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --remote-debugging-port=9229\n\
          101 100 100 Thu Jul 23 19:23:13 2026 /Applications/ChatGPT.app/Contents/Resources/codex app-server\n\
          102 101 102 Thu Jul 23 19:23:14 2026 /Applications/ChatGPT.app/Contents/Frameworks/Chromium Helper\n",
    );
    assert!(macos_main_executable_is_running(
        &processes,
        Path::new("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
    ));
    assert!(!macos_main_executable_is_running(
        &processes[1..],
        Path::new("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
    ));
}

#[test]
fn owned_codex_tree_includes_bundle_helpers_and_external_descendants() {
    let processes = crate::process_tree::parse_unix_process_snapshot(
        b"100 1 100 Thu Jul 23 19:23:12 2026 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --inspect\n\
          101 100 100 Thu Jul 23 19:23:13 2026 /Applications/ChatGPT.app/Contents/Resources/codex app-server\n\
          102 101 102 Thu Jul 23 19:23:14 2026 node ./mcp/server.mjs\n\
          103 1 103 Thu Jul 23 19:23:15 2026 /Applications/ChatGPT.app/Contents/Frameworks/browser_crashpad_handler\n\
          200 1 200 Thu Jul 23 19:23:16 2026 unrelated\n",
    );
    assert_eq!(
        owned_unix_codex_process_ids(
            &processes,
            Path::new("/Applications/ChatGPT.app"),
            None,
            None,
            Some("--inspect"),
        ),
        HashSet::from([100, 101, 102, 103])
    );
}

#[tokio::test]
async fn unix_shutdown_terminates_the_spawned_process_group() {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30 & wait"]);
    command.process_group(0);
    let mut child = command.spawn().expect("spawn process tree");
    let process_id = child.id().expect("child process id");

    terminate_unix_codex_processes(
        Path::new("/definitely-not-a-real-codex-app"),
        Some(process_id),
        Some(process_id),
        None,
    )
    .await
    .expect("terminate process tree");

    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("root process was left running")
        .expect("wait for root process");
}

#[tokio::test]
async fn exit_watcher_reports_a_naturally_exited_child() {
    let child = Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .expect("spawn short-lived child");
    let child = Arc::new(Mutex::new(Some(child)));
    let exited = Arc::new(AtomicBool::new(false));
    let (_shutdown, exit_rx, task) = spawn_codex_exit_watcher(child, exited.clone());

    tokio::time::timeout(Duration::from_secs(2), exit_rx)
        .await
        .expect("watcher timed out")
        .expect("watcher was cancelled");
    task.await.expect("watcher task failed");
    assert!(exited.load(Ordering::Acquire));
}

#[tokio::test]
async fn exit_watcher_returns_the_child_to_stop_on_shutdown() {
    let child = Command::new("sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("spawn long-lived child");
    let child = Arc::new(Mutex::new(Some(child)));
    let exited = Arc::new(AtomicBool::new(false));
    let (shutdown, _exit_rx, task) = spawn_codex_exit_watcher(child.clone(), exited.clone());

    shutdown.send(()).expect("send watcher shutdown");
    task.await.expect("watcher task failed");

    assert!(!exited.load(Ordering::Acquire));
    let mut process = child
        .lock()
        .await
        .take()
        .expect("watcher should return the child");
    process.kill().await.expect("kill child");
    process.wait().await.expect("reap child");
}

#[test]
fn cdp_watchdog_requires_consecutive_failures_before_reinjecting() {
    let mut failures = 0;

    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Unhealthy
    ));
    assert_eq!(failures, 1);
    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Healthy
    ));
    assert_eq!(failures, 0);
    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Unhealthy
    ));
    assert!(watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Unhealthy
    ));
}

#[test]
fn cdp_watchdog_does_not_reinject_after_renderer_timeouts() {
    let mut failures = 0;

    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Inconclusive
    ));
    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Inconclusive
    ));
    assert_eq!(failures, 0);

    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Unhealthy
    ));
    assert!(!watchdog_should_reinject(
        &mut failures,
        InjectionHealth::Inconclusive
    ));
    assert_eq!(failures, 0);
}

#[test]
fn cdp_watchdog_immediately_rediscovers_an_unavailable_target() {
    let mut failures = 1;

    assert!(watchdog_should_reinject(
        &mut failures,
        InjectionHealth::TargetUnavailable
    ));
    assert_eq!(failures, 0);
}
