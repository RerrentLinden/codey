use super::*;

#[test]
fn maintenance_status_exposes_structured_session_metrics() {
    let provider_sync = ProviderSyncResult {
        status: ProviderSyncStatus::Synced,
        message: "ok".to_string(),
        target_provider: "openai".to_string(),
        backup_dir: None,
        changed_session_files: 3,
        skipped_locked_rollout_files: Vec::new(),
        sqlite_rows_updated: 7,
        sqlite_provider_rows_updated: 2,
        sqlite_user_event_rows_updated: 3,
        sqlite_cwd_rows_updated: 2,
        updated_workspace_roots: 1,
        encrypted_content_warning: None,
    };
    let cleanup = Ok(SessionIndexCleanupReport {
        scanned_entries: 5,
        live_threads: 3,
        pruned_entries: 2,
        backup_dir: None,
    });

    let summary = session_maintenance_summary(&provider_sync, &cleanup);
    let status = MaintenanceStatus {
        session_status: summary.status,
        session_files_fixed: summary.files_fixed,
        sqlite_rows_updated: summary.sqlite_rows_updated,
        ghost_tasks_pruned: summary.ghost_tasks_pruned,
        performance_status: "ready".to_string(),
        performance_detail: String::new(),
    };
    let value = serde_json::to_value(status).unwrap();

    assert_eq!(value["sessionFilesFixed"], 3);
    assert_eq!(value["sqliteRowsUpdated"], 7);
    assert_eq!(value["ghostTasksPruned"], 2);
}
