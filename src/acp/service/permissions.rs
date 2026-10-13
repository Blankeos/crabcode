use super::notifications::tool_kind;
use super::permission_preview::permission_tool_content;
use super::tool_locations::tool_locations;
use super::AcpSession;
use crate::config::configuration::LoadedConfig;
use agent_client_protocol::schema::v1::PermissionOption;
use agent_client_protocol::schema::v1::PermissionOptionKind;
use agent_client_protocol::schema::v1::RequestPermissionOutcome;
use agent_client_protocol::schema::v1::RequestPermissionRequest;
use agent_client_protocol::schema::v1::ToolCallStatus;
use agent_client_protocol::schema::v1::ToolCallUpdate;
use agent_client_protocol::schema::v1::ToolCallUpdateFields;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use std::path::Path;
use std::path::PathBuf;

fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption::new("once", "Allow once", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            "always",
            "Always allow for this session",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
    ]
}

fn permission_tool_call_id(tool_call_id: Option<&str>) -> String {
    tool_call_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("permission:{}", cuid2::create_id()))
}

pub(super) fn configured_tool_permissions(
    cwd: &Path,
    config: &LoadedConfig,
) -> crate::tools::ToolPermissions {
    let mut policies = crate::tools::AgentToolPolicies::default();
    for (mode, tools) in config.merged_config.agent_registry.tool_policy_map() {
        policies = policies.with_custom_tools(mode, tools);
    }
    crate::tools::ToolPermissions::new(cwd)
        .with_agent_policies(policies)
        .with_permission_rules(config.merged_config.permission_rules.clone())
        .with_agent_permission_rules(config.merged_config.agent_registry.permission_rules_map())
}

pub(super) fn tool_permissions(session: &AcpSession) -> crate::tools::ToolPermissions {
    session.tool_permissions.clone()
}

pub(super) async fn request_permission(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    prompt: &crate::tools::PermissionPrompt,
) -> crate::tools::PermissionResponse {
    let tool_call_id = permission_tool_call_id(prompt.tool_call_id.as_deref());
    let input = serde_json::json!({
        "tool": prompt.tool_id,
        "permission": prompt.permission,
        "patterns": prompt.patterns,
        "target": prompt.target,
        "command": prompt.command,
        "workdir": prompt.workdir,
        "reason": prompt.reason,
        "input": prompt.raw_input,
    });
    let cwd = PathBuf::from(&prompt.workspace);
    let content = permission_tool_content(prompt, &cwd);
    let tool_call = ToolCallUpdate::new(tool_call_id, {
        let mut fields = ToolCallUpdateFields::new()
            .title(permission_title(prompt))
            .kind(tool_kind(&prompt.tool_id))
            .status(ToolCallStatus::Pending)
            .locations(tool_locations(&prompt.tool_id, &prompt.raw_input, &cwd))
            .raw_input(input);
        if !content.is_empty() {
            fields = fields.content(content);
        }
        fields
    });
    let request =
        RequestPermissionRequest::new(session_id.to_string(), tool_call, permission_options());
    let Ok(response) = connection.send_request(request).block_task().await else {
        return crate::tools::PermissionResponse::Deny;
    };
    match response.outcome {
        RequestPermissionOutcome::Selected(selection)
            if selection.option_id.to_string() == "once" =>
        {
            crate::tools::PermissionResponse::AllowOnce
        }
        RequestPermissionOutcome::Selected(selection)
            if selection.option_id.to_string() == "always" =>
        {
            crate::tools::PermissionResponse::AllowAlways
        }
        RequestPermissionOutcome::Selected(_) | RequestPermissionOutcome::Cancelled => {
            crate::tools::PermissionResponse::Deny
        }
        _ => crate::tools::PermissionResponse::Deny,
    }
}

fn permission_title(prompt: &crate::tools::PermissionPrompt) -> String {
    prompt
        .command
        .as_deref()
        .or(prompt.target.as_deref())
        .unwrap_or(&prompt.reason)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_session;
    use super::*;
    use agent_client_protocol::schema::v1::PermissionOptionKind;

    #[test]
    fn acp_permission_prefers_originating_tool_call_id() {
        assert_eq!(permission_tool_call_id(Some("call_123")), "call_123");
    }

    #[test]
    fn acp_permission_generates_fallback_id_without_origin() {
        assert!(permission_tool_call_id(None).starts_with("permission:"));
    }

    #[test]
    fn acp_permission_offers_explicit_session_wide_allow() {
        let options = permission_options();
        let always = options
            .iter()
            .find(|option| option.option_id.to_string() == "always")
            .expect("always option");
        assert_eq!(always.name, "Always allow for this session");
        assert_eq!(always.kind, PermissionOptionKind::AllowAlways);
    }

    #[tokio::test]
    async fn acp_session_reuses_permission_grants_across_turns() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let external = temp.path().join("external");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&external).expect("external");

        let mut session = test_session();
        session.cwd = workspace.clone();
        session.tool_permissions = crate::tools::ToolPermissions::new(&workspace);
        let first_turn = tool_permissions(&session);
        let second_turn = tool_permissions(&session);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": external.join("one.txt") });

        let pending = tokio::spawn({
            let permissions = first_turn.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move {
                permissions
                    .preflight("build", "read", &params, Some(&tx))
                    .await
            }
        });
        let prompt = match rx.recv().await {
            Some(crate::llm::ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("expected permission request"),
        };
        let _ = prompt
            .response_tx
            .send(crate::tools::PermissionResponse::AllowAlways);
        assert!(pending.await.expect("permission task").is_ok());

        let next_params = serde_json::json!({ "file_path": external.join("nested/two.txt") });
        assert!(second_turn
            .preflight("build", "read", &next_params, Some(&tx))
            .await
            .is_ok());
        assert!(rx.try_recv().is_err());
    }
}
