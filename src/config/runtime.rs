//! Shared config → runtime wiring used by both print mode and the interactive TUI.
//!
//! Keep permission, discovery, and instruction construction here so new top-level
//! settings cannot be applied in only one entrypoint.

use std::path::{Path, PathBuf};

use crate::config::configuration::MergedConfig;
use crate::model::discovery::Discovery;
use crate::tools::{
    AgentToolPolicies, PermissionPolicyAction, PermissionRule, PermissionRules, ToolPermissions,
};

/// Options that differ between print mode and the interactive app.
#[derive(Debug, Clone, Default)]
pub struct ConfigRuntimeOptions {
    /// When true, deny interactive-only tools (`question`, `update_plan`).
    pub print_mode: bool,
    /// Skip permission prompts (print-mode `--dangerously-skip-permissions`).
    pub dangerously_skip_permissions: bool,
}

/// Runtime pieces derived from merged config.
pub struct ConfigRuntime {
    pub tool_permissions: ToolPermissions,
    pub discovery: Option<Discovery>,
    pub custom_instructions: String,
}

impl ConfigRuntime {
    /// Build permissions, discovery, and instructions from already-loaded config.
    pub fn from_merged(
        merged: &MergedConfig,
        cwd: impl Into<PathBuf>,
        options: ConfigRuntimeOptions,
    ) -> Self {
        let cwd = cwd.into();
        let custom_instructions = merged.instructions.join("\n\n");

        let mut agent_policies = AgentToolPolicies::default();
        for (mode, tools) in merged.agent_registry.tool_policy_map() {
            agent_policies = agent_policies.with_custom_tools(mode, tools);
        }

        let mut permission_rules = merged.permission_rules.clone();
        if options.print_mode {
            permission_rules = deny_print_mode_interactive_tools(permission_rules);
        }

        let tool_permissions = ToolPermissions::new(cwd)
            .with_agent_policies(agent_policies)
            .with_global_tool_config(merged.tools.clone())
            .with_permission_rules(permission_rules)
            .with_agent_permission_rules(merged.agent_registry.permission_rules_map())
            .dangerously_skip_permissions(options.dangerously_skip_permissions);

        let discovery = Discovery::new_with_config(
            Some(merged.custom_providers.clone()),
            merged.disabled_providers.clone(),
            merged.enabled_providers.clone(),
        )
        .ok();

        Self {
            tool_permissions,
            discovery,
            custom_instructions,
        }
    }

    /// Convenience overload taking a `Path`.
    pub fn from_merged_at(
        merged: &MergedConfig,
        cwd: &Path,
        options: ConfigRuntimeOptions,
    ) -> Self {
        Self::from_merged(merged, cwd.to_path_buf(), options)
    }
}

fn deny_print_mode_interactive_tools(mut rules: PermissionRules) -> PermissionRules {
    for tool_id in ["question", "update_plan"] {
        rules.push(PermissionRule {
            permission: tool_id.to_string(),
            pattern: "*".to_string(),
            action: PermissionPolicyAction::Deny,
        });
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::configuration::MergedConfig;
    use std::collections::HashSet;

    #[test]
    fn applies_global_tool_disable() {
        let mut merged = MergedConfig::default();
        merged.tools.insert("bash".into(), false);

        let rt =
            ConfigRuntime::from_merged(&merged, "/tmp/workspace", ConfigRuntimeOptions::default());

        assert!(!rt
            .tool_permissions
            .is_tool_allowed_for_agent("build", "bash"));
        assert!(!rt
            .tool_permissions
            .is_tool_visible_for_agent("build", "bash"));
        // Unmentioned tools remain available.
        assert!(rt
            .tool_permissions
            .is_tool_allowed_for_agent("build", "read"));
    }

    #[test]
    fn joins_custom_instructions() {
        let mut merged = MergedConfig::default();
        merged.instructions = vec!["first".into(), "second".into()];

        let rt =
            ConfigRuntime::from_merged(&merged, "/tmp/workspace", ConfigRuntimeOptions::default());

        assert_eq!(rt.custom_instructions, "first\n\nsecond");
    }

    #[test]
    fn print_mode_denies_interactive_only_tools() {
        let merged = MergedConfig::default();

        let print_rt = ConfigRuntime::from_merged(
            &merged,
            "/tmp/workspace",
            ConfigRuntimeOptions {
                print_mode: true,
                ..Default::default()
            },
        );
        let tui_rt =
            ConfigRuntime::from_merged(&merged, "/tmp/workspace", ConfigRuntimeOptions::default());

        assert!(!print_rt
            .tool_permissions
            .is_tool_visible_for_agent("build", "question"));
        assert!(!print_rt
            .tool_permissions
            .is_tool_visible_for_agent("build", "update_plan"));
        assert!(tui_rt
            .tool_permissions
            .is_tool_visible_for_agent("build", "question"));
    }

    #[test]
    fn threads_provider_filters_into_discovery() {
        let mut merged = MergedConfig::default();
        merged.disabled_providers = HashSet::from(["openai".into()]);
        merged.enabled_providers = Some(HashSet::from(["anthropic".into()]));

        let rt =
            ConfigRuntime::from_merged(&merged, "/tmp/workspace", ConfigRuntimeOptions::default());

        let discovery = rt.discovery.expect("discovery should construct");
        assert!(!discovery.provider_is_enabled("openai"));
        assert!(!discovery.provider_is_enabled("other")); // not in allowlist
        assert!(discovery.provider_is_enabled("anthropic"));
    }

    #[test]
    fn tui_and_print_share_same_tool_and_instruction_wiring() {
        let mut merged = MergedConfig::default();
        merged.tools.insert("bash".into(), false);
        merged.instructions = vec!["Always begin with CUSTOM-INSTRUCTION.".into()];

        let tui =
            ConfigRuntime::from_merged(&merged, "/tmp/workspace", ConfigRuntimeOptions::default());
        let print = ConfigRuntime::from_merged(
            &merged,
            "/tmp/workspace",
            ConfigRuntimeOptions {
                print_mode: true,
                ..Default::default()
            },
        );

        assert_eq!(tui.custom_instructions, print.custom_instructions);
        assert!(!tui
            .tool_permissions
            .is_tool_allowed_for_agent("build", "bash"));
        assert!(!print
            .tool_permissions
            .is_tool_allowed_for_agent("build", "bash"));
    }
}
