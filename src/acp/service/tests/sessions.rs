use super::*;
use std::path::Path;

#[test]
fn session_info_includes_hierarchy_metadata() {
    let now = std::time::SystemTime::now();
    let info = acp_session_info(
        crate::session::manager::SessionInfo {
            id: "child".to_string(),
            parent_id: Some("parent".to_string()),
            title: "Child".to_string(),
            created_at: now,
            updated_at: now,
            message_count: 0,
            workspace_id: 1,
            workspace_path: "/tmp".to_string(),
            workspace_name: "tmp".to_string(),
            workspace_sort_order: 0,
            status: crate::session::types::SessionStatus::Idle,
            pinned_at: None,
            archived_at: None,
        },
        Some("root".to_string()),
    );
    let meta = info.meta.expect("session metadata");
    assert_eq!(meta["crabcode"]["parentSessionId"], "parent");
    assert_eq!(meta["crabcode"]["rootSessionId"], "root");
}

#[test]
fn session_list_cursor_pages_all_sessions() {
    assert_eq!(
        session_page(None, 250).unwrap(),
        (0, 100, Some("100".to_string()))
    );
    assert_eq!(
        session_page(Some("100"), 250).unwrap(),
        (100, 200, Some("200".to_string()))
    );
    assert_eq!(session_page(Some("200"), 250).unwrap(), (200, 250, None));
    assert!(session_page(Some("invalid"), 250).is_err());
}

#[test]
fn rejects_non_absolute_workspaces() {
    assert!(workspace_path(Path::new("relative")).is_err());
}
