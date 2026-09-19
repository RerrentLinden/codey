use super::*;

fn start_recovery_child(root: &Path, session: &str) {
    start_recovery_child_named(root, session, "reader");
}

fn start_recovery_child_named(root: &Path, session: &str, task_name: &str) {
    write_test_runtime_policy(root);
    let mut spawn = input("PreToolUse", session);
    spawn.turn_id = Some("root-turn-a".into());
    spawn.tool_name = Some("agents.spawn_agent".into());
    spawn.tool_input = Some(json!({
        "task_name": task_name, "agent_type": "codey_quick_scan", "message": "Read the source"
    }));
    assert_eq!(
        handle_hook_for_runtime_at(&spawn, root, "runtime-a", 10).unwrap(),
        json!({})
    );
    spawn.hook_event_name = "PostToolUse".into();
    let target = format!("/root/{task_name}");
    spawn.tool_response = Some(json!({"agent_id": target}));
    handle_hook_for_runtime_at(&spawn, root, "runtime-a", 11).unwrap();
    let mut start = input("SubagentStart", session);
    start.agent_id = Some(target);
    start.agent_type = Some("codey_quick_scan".into());
    handle_hook_for_runtime_at(&start, root, "runtime-a", 12).unwrap();
}

fn root_command(session: &str) -> HookInput {
    let mut command = input("PreToolUse", session);
    command.tool_name = Some("functions.exec_command".into());
    command.tool_input = Some(json!({"cmd": "git status --short"}));
    command
}

fn unavailable_wait(session: &str) -> HookInput {
    let mut wait = input("PostToolUse", session);
    wait.tool_name = Some("agents.wait_agent".into());
    wait.tool_response = Some(json!("该工具未在当前线程注册，无法执行 agents.wait_agent"));
    wait
}

#[test]
fn root_entry_points_recover_without_a_stop_hook_or_status_receipt() {
    for event in ["PreToolUse", "UserPromptSubmit", "PostToolUse"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = "no-stop";
        start_recovery_child(root, session);
        let command = root_command(session);
        assert_eq!(
            handle_hook_for_runtime_at(&command, root, "runtime-a", 1_000).unwrap()["hookSpecificOutput"]
                ["permissionDecision"],
            "deny"
        );
        let mut trigger = input(event, session);
        if event == "PreToolUse" {
            trigger = root_command(session);
        }
        if event == "PostToolUse" {
            trigger = unavailable_wait(session);
        }
        handle_hook_for_runtime_at(&trigger, root, "runtime-a", 1_000 + STOP_STALL_GRACE_MILLIS)
            .unwrap();
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
            0,
            "{event}"
        );
        assert_eq!(
            handle_hook_for_runtime_at(
                &command,
                root,
                "runtime-a",
                1_001 + STOP_STALL_GRACE_MILLIS
            )
            .unwrap(),
            json!({}),
            "{event}"
        );
        let ledger: Value = serde_json::from_slice(
            &fs::read(session_state_dir(root, session).join("orchestrator-ledger-v1.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(ledger["reservations"]["reader"]["outcome"], "lost");
        assert!(ledger["reservations"]["reader"]["fenced_at_ms"].is_number());
    }
}

#[test]
fn unavailable_status_tools_recover_after_short_grace_and_reject_late_child_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let session = "unavailable";
    start_recovery_child(root, session);
    let wait = unavailable_wait(session);
    assert_eq!(
        handle_hook_for_runtime_at(&wait, root, "runtime-a", 1_000).unwrap()["decision"],
        "block"
    );
    let command = root_command(session);
    assert_eq!(
        handle_hook_for_runtime_at(
            &command,
            root,
            "runtime-a",
            1_000 + UNAVAILABLE_STATUS_GRACE_MILLIS - 1
        )
        .unwrap()["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
    assert_eq!(
        handle_hook_for_runtime_at(
            &command,
            root,
            "runtime-a",
            1_000 + UNAVAILABLE_STATUS_GRACE_MILLIS
        )
        .unwrap(),
        json!({})
    );

    let mut child = root_command(session);
    child.agent_id = Some("/root/reader".into());
    child.agent_type = Some("codey_quick_scan".into());
    child.tool_name = Some("mcp__codey_fastctx__inspect_local_file".into());
    attest_test_child(&child, root, "runtime-a");
    assert_eq!(
        handle_hook_for_runtime_at(&child, root, "runtime-a", 32_000).unwrap()["hookSpecificOutput"]
            ["permissionDecision"],
        "deny"
    );

    let mut late = input("PostToolUse", session);
    late.tool_name = Some("agents.agent_status".into());
    late.tool_input = Some(json!({"target": "/root/reader"}));
    late.tool_response = Some(json!({"agent_id": "/root/reader", "status": "running"}));
    assert_eq!(
        handle_hook_for_runtime_at(&late, root, "runtime-a", 33_000).unwrap(),
        json!({})
    );
    assert_eq!(
        active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
        0
    );
}

#[test]
fn working_status_tool_cancels_unavailability_recovery() {
    for tool in [
        "agents.wait_agent",
        "agents.list_agents",
        "agents.agent_status",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = "working-fallback";
        start_recovery_child(root, session);
        handle_hook_for_runtime_at(&unavailable_wait(session), root, "runtime-a", 1_000).unwrap();
        let mut status = input("PostToolUse", session);
        status.tool_name = Some(tool.into());
        status.tool_input = Some(if tool == "agents.agent_status" {
            json!({"target": "/root/reader"})
        } else {
            json!({})
        });
        status.tool_response = Some(if tool == "agents.list_agents" {
            json!({"agents": [{"agent_id": "/root/reader", "agent_status": "running"}]})
        } else {
            json!({"agent_id": "/root/reader", "status": "running"})
        });
        handle_hook_for_runtime_at(&status, root, "runtime-a", 2_000).unwrap();
        assert_eq!(
            handle_hook_for_runtime_at(&root_command(session), root, "runtime-a", 31_000).unwrap()
                ["hookSpecificOutput"]["permissionDecision"],
            "deny",
            "{tool}"
        );
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
            1
        );
    }
}

#[test]
fn single_agent_status_is_anonymous_reconciliation_and_settles_only_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let session = "single-status";
    start_recovery_child(root, session);
    start_recovery_child_named(root, session, "sibling");
    let mut status = input("PreToolUse", session);
    status.tool_name = Some("agents.agent_status".into());
    status.tool_input = Some(json!({"target": "/root/reader"}));
    assert_eq!(
        handle_hook_for_runtime_at(&status, root, "runtime-a", 1_000).unwrap(),
        json!({})
    );
    status.hook_event_name = "PostToolUse".into();
    status.tool_response = Some(json!({"agent_id": "/root/reader", "agent_status": "completed"}));
    assert_eq!(
        handle_hook_for_runtime_at(&status, root, "runtime-a", 1_001).unwrap()["decision"],
        "block"
    );
    assert_eq!(
        active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
        1
    );
    assert!(
        agent_marker_path(
            &session_state_dir(root, session),
            "runtime-a",
            "/root/sibling"
        )
        .exists()
    );
}

#[test]
fn status_tool_unavailability_does_not_come_from_child_payloads() {
    for response in [
        json!({"message": "Please retry later"}),
        json!({"agent_id": "/root/a", "error": "unknown tool agents.wait_agent"}),
        json!({"updates": [{"message": "unknown tool agents.wait_agent"}]}),
        json!({"output": {"error": {"code": "tool_not_found"}}}),
        json!("unknown tool agents.spawn_agent"),
    ] {
        assert!(
            !status_tool_is_unavailable("agents.wait_agent", Some(&response)),
            "{response}"
        );
    }
    for response in [
        json!("unknown tool agents.wait_agent"),
        json!({"error": {"code": "tool_not_registered"}}),
        json!({"message": "agents.wait_agent is not registered"}),
    ] {
        assert!(
            status_tool_is_unavailable("agents.wait_agent", Some(&response)),
            "{response}"
        );
    }
}

#[test]
fn corrupt_ledger_and_marker_recovery_persists_across_later_hooks() {
    for corruption in ["ledger", "marker"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = "corrupt-recovery";
        start_recovery_child(root, session);
        let dir = session_state_dir(root, session);
        let path = if corruption == "ledger" {
            dir.join("orchestrator-ledger-v1.json")
        } else {
            agent_marker_path(&dir, "runtime-a", "/root/reader")
        };
        fs::write(&path, b"{").unwrap();
        let command = root_command(session);
        assert!(handle_hook_for_runtime_at(&command, root, "runtime-a", 1_000).is_err());
        assert_eq!(
            handle_hook_for_runtime_at(
                &command,
                root,
                "runtime-a",
                1_000 + STOP_STALL_GRACE_MILLIS
            )
            .unwrap(),
            json!({})
        );
        assert_eq!(
            handle_hook_for_runtime_at(
                &command,
                root,
                "runtime-a",
                1_001 + STOP_STALL_GRACE_MILLIS
            )
            .unwrap(),
            json!({})
        );
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
            0
        );
        if corruption == "ledger" {
            let quarantines = fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("orchestrator-ledger-v1.corrupt-")
                })
                .count();
            assert_eq!(quarantines, 1);
        }
    }
}

#[test]
fn child_hooks_cannot_release_a_stalled_root_gate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let session = "child-recovery";
    start_recovery_child(root, session);
    handle_hook_for_runtime_at(&root_command(session), root, "runtime-a", 1_000).unwrap();
    for event in ["PreToolUse", "PostToolUse", "UserPromptSubmit", "Stop"] {
        let mut child = input(event, session);
        child.agent_id = Some("/root/reader".into());
        child.agent_type = Some("codey_quick_scan".into());
        handle_hook_for_runtime_at(&child, root, "runtime-a", 1_000 + STOP_STALL_GRACE_MILLIS)
            .unwrap();
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
            1,
            "{event}"
        );
    }
}

#[test]
fn corruption_recovery_cannot_remove_a_newer_runtimes_ledger() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let session = "retired-runtime-recovery";
    start_recovery_child(root, session);
    crate::subagent_orchestrator::active_reservation_count(root, "runtime-b", session, 100)
        .unwrap();
    let ledger_path = session_state_dir(root, session).join("orchestrator-ledger-v1.json");
    let expected = fs::read(&ledger_path).unwrap();
    let command = root_command(session);
    for now_ms in [1_000, 1_000 + STOP_STALL_GRACE_MILLIS] {
        let error = handle_hook_for_runtime_at(&command, root, "runtime-a", now_ms).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("CODEY_SUBAGENT_STALE_RUNTIME_EVENT")
        );
        assert_eq!(fs::read(&ledger_path).unwrap(), expected);
    }
}

#[test]
fn post_status_absolute_recovery_retains_the_next_turn_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let session = "post-absolute-recovery";
    start_recovery_child(root, session);
    handle_hook_for_runtime_at(&root_command(session), root, "runtime-a", 1_000).unwrap();
    let mut status = input("PostToolUse", session);
    status.tool_name = Some("agents.agent_status".into());
    status.tool_input = Some(json!({"target": "/root/reader"}));
    status.tool_response = Some(json!({"agent_id": "/root/reader", "status": "running"}));
    assert_eq!(
        handle_hook_for_runtime_at(
            &status,
            root,
            "runtime-a",
            1_000 + STOP_ABSOLUTE_GRACE_MILLIS
        )
        .unwrap(),
        json!({})
    );
    let next_turn = handle_hook_for_runtime_at(
        &input("UserPromptSubmit", session),
        root,
        "runtime-a",
        1_001 + STOP_ABSOLUTE_GRACE_MILLIS,
    )
    .unwrap();
    assert!(
        next_turn["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("绝对上限")
    );
}

#[test]
fn single_target_status_rejects_mismatched_or_mixed_response_identities() {
    for response in [
        json!({"agent_id": "/root/sibling", "status": "completed"}),
        json!({"agents": [
            {"agent_id": "/root/reader", "status": "completed"},
            {"agent_id": "/root/sibling", "status": "completed"}
        ]}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = "mismatched-status";
        start_recovery_child(root, session);
        start_recovery_child_named(root, session, "sibling");
        let mut status = input("PostToolUse", session);
        status.tool_name = Some("agents.agent_status".into());
        status.tool_input = Some(json!({"target": "/root/reader"}));
        status.tool_response = Some(response);
        assert_eq!(
            handle_hook_for_runtime_at(&status, root, "runtime-a", 1_000).unwrap()["decision"],
            "block"
        );
        assert_eq!(
            active_agent_count_for_runtime(root, "runtime-a", session).unwrap(),
            2
        );
    }
}
