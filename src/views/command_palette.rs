use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

use crate::command::custom::CustomCommandSource;
use crate::command::registry::Registry;
use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};

const APP_ACTION_PROVIDER: &str = "__command_palette_app_action";

#[derive(Debug, Clone, PartialEq)]
pub enum CommandPaletteAction {
    RunCommand(String),
    RunAppAction(CommandPaletteAppAction),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPaletteAppAction {
    ToggleAgentMode,
    OpenAgentsDialog,
    OpenFind,
    SetThinkingVisible(bool),
    CycleReasoningEffort,
    SetCompactMode(bool),
    OpenStorage,
    OpenSkillsDialog,
    OpenMcpDialog,
    OpenJobs,
}

#[derive(Debug)]
pub struct CommandPaletteState {
    pub dialog: Dialog,
}

impl CommandPaletteState {
    pub fn new() -> Self {
        Self {
            dialog: Dialog::with_items("Command Palette", Vec::new()).with_actions(base_actions()),
        }
    }

    pub fn refresh_items(
        &mut self,
        registry: &Registry,
        is_chat: bool,
        thinking_visible: bool,
        compact_mode: bool,
    ) {
        let was_visible = self.dialog.is_visible();
        let search_query = self.dialog.search_query.clone();
        let selected = self
            .dialog
            .get_selected()
            .map(|item| (item.id.clone(), item.provider_id.clone()));

        let mut items = core_palette_items(registry, is_chat, thinking_visible, compact_mode);
        items.insert(
            items
                .iter()
                .position(|item| item.group == "Model")
                .unwrap_or(items.len()),
            app_action_item(
                "open-skills-dialog",
                "Skills",
                "Model",
                "View and select available skills",
                None,
                &[],
            ),
        );

        items.extend(custom_command_items(registry, is_chat));

        self.dialog = Dialog::with_items("Command Palette", items).with_actions(base_actions());
        self.dialog.restore_search_query(search_query);

        if was_visible {
            self.dialog.show();
        }

        if let Some((id, provider_id)) = selected {
            let _ = self.dialog.select_item_by_key(&id, &provider_id);
        }
    }

    pub fn show(&mut self) {
        self.dialog.show();
    }
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_command_palette() -> CommandPaletteState {
    CommandPaletteState::new()
}

pub fn render_command_palette(
    f: &mut Frame,
    state: &mut CommandPaletteState,
    area: Rect,
    colors: ThemeColors,
) {
    state.dialog.render(f, area, colors);
}

pub fn handle_command_palette_key_event(
    state: &mut CommandPaletteState,
    event: KeyEvent,
) -> CommandPaletteAction {
    if !state.dialog.is_visible() {
        return CommandPaletteAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            state.dialog.hide();
            if let Some(selected) = state.dialog.get_selected() {
                return action_for_item(selected);
            }
        }
        _ => {
            state.dialog.handle_key_event(event);
        }
    }

    CommandPaletteAction::None
}

pub fn handle_command_palette_mouse_event(
    state: &mut CommandPaletteState,
    event: MouseEvent,
) -> CommandPaletteAction {
    if !state.dialog.is_visible() {
        return CommandPaletteAction::None;
    }

    let clicked_item = if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        state.dialog.item_index_at_position(event.column, event.row)
    } else {
        None
    };

    state.dialog.handle_mouse_event(event);

    if clicked_item.is_some() && state.dialog.is_visible() {
        if let Some(selected) = state.dialog.get_selected() {
            let action = action_for_item(selected);
            state.dialog.hide();
            return action;
        }
    }

    CommandPaletteAction::None
}

fn base_actions() -> Vec<DialogAction> {
    vec![
        DialogAction {
            label: "Run".to_string(),
            key: "enter".to_string(),
        },
        DialogAction {
            label: "Close".to_string(),
            key: "esc".to_string(),
        },
    ]
}

fn command_palette_tip(command_name: &str) -> Option<String> {
    match command_name {
        "models" => Some("ctrl+x m".to_string()),
        "themes" => Some("ctrl+x t".to_string()),
        "sessions" => Some("ctrl+x l".to_string()),
        "new" => Some("ctrl+x n".to_string()),
        "exit" => Some("ctrl+x q".to_string()),
        _ => None,
    }
}

fn action_for_item(item: &DialogItem) -> CommandPaletteAction {
    if is_app_action(item) {
        return match item.id.as_str() {
            "toggle-agent-mode" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::ToggleAgentMode)
            }
            "open-agents-dialog" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenAgentsDialog)
            }
            "open-find" => CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenFind),
            "collapse-thinking" => CommandPaletteAction::RunAppAction(
                CommandPaletteAppAction::SetThinkingVisible(false),
            ),
            "expand-thinking" => CommandPaletteAction::RunAppAction(
                CommandPaletteAppAction::SetThinkingVisible(true),
            ),
            "cycle-reasoning-effort" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::CycleReasoningEffort)
            }
            "enable-compact-mode" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::SetCompactMode(true))
            }
            "disable-compact-mode" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::SetCompactMode(false))
            }
            "open-storage" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenStorage)
            }
            "open-skills-dialog" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenSkillsDialog)
            }
            "open-mcp-dialog" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenMcpDialog)
            }
            "open-jobs" => CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenJobs),
            _ => CommandPaletteAction::None,
        };
    }

    CommandPaletteAction::RunCommand(item.id.clone())
}

fn is_app_action(item: &DialogItem) -> bool {
    item.provider_id
        .split_whitespace()
        .next()
        .is_some_and(|provider_id| provider_id == APP_ACTION_PROVIDER)
}

fn core_palette_items(
    registry: &Registry,
    is_chat: bool,
    thinking_visible: bool,
    compact_mode: bool,
) -> Vec<DialogItem> {
    let mut items = Vec::new();

    for (command, name, group, description) in [
        ("new", "New Session", "Workspace", "Start a blank session"),
        (
            "sessions",
            "Open Sessions",
            "Workspace",
            "Browse and switch sessions",
        ),
        (
            "rename",
            "Rename Session",
            "Workspace",
            "Rename the current session",
        ),
        (
            "timeline",
            "Open Timeline",
            "Workspace",
            "Jump between messages",
        ),
        (
            "copy",
            "Copy Session Transcript",
            "Workspace",
            "Copy the current transcript",
        ),
        (
            "compact",
            "Compact Context",
            "Workspace",
            "Summarize this session to reduce context",
        ),
        (
            "fork",
            "Fork Session",
            "Workspace",
            "Create a new session from this transcript",
        ),
        (
            "move",
            "Move Session",
            "Workspace",
            "Move to another project dir",
        ),
        (
            "home",
            "Go Home",
            "Workspace",
            "Return to a blank home screen",
        ),
        ("models", "Change Model", "Model", "Choose the active model"),
        (
            "variants",
            "Switch model variant",
            "Model",
            "Choose reasoning effort for the active model",
        ),
        (
            "connect",
            "Connect Provider",
            "Model",
            "Add or update provider credentials",
        ),
        (
            "remote",
            "Start Remote Host",
            "Application",
            "Close the TUI and run crabcode serve",
        ),
        (
            "refreshmodels",
            "Refresh Model Cache",
            "Model",
            "Refresh models.dev provider data",
        ),
        (
            "themes",
            "Change Theme",
            "Appearance",
            "Choose a color theme",
        ),
        (
            "title",
            "Configure Terminal Title",
            "Appearance",
            "Choose and reorder terminal title items",
        ),
        (
            "status",
            "Status",
            "Application",
            "Show MCP, formatter, and plugin status",
        ),
        ("exit", "Quit Crabcode", "Application", "Exit the app"),
    ] {
        let Some(registered) = registry.get(command) else {
            continue;
        };
        if !is_chat && registered.chat_only {
            continue;
        }

        items.push(DialogItem {
            id: command.to_string(),
            name: name.to_string(),
            group: group.to_string(),
            description: description.to_string(),
            tip: command_palette_tip(command),
            provider_id: registered.hidden_tokens.join(" "),
            active: false,
        });
    }

    items.insert(
        2.min(items.len()),
        app_action_item(
            "toggle-agent-mode",
            "Agent Cycle",
            "Agent",
            "Cycle through primary agents",
            Some("tab"),
            &[],
        ),
    );

    if registry.get("agents").is_some() {
        items.insert(
            3.min(items.len()),
            app_action_item(
                "open-agents-dialog",
                "Switch Agent",
                "Agent",
                "Choose the active primary agent",
                None,
                &["agents", "agent mode", "primary agent"],
            ),
        );
    }

    if is_chat {
        items.insert(
            items
                .iter()
                .position(|item| item.group == "Workspace")
                .map(|idx| idx + 1)
                .unwrap_or(items.len()),
            app_action_item(
                "open-find",
                "Search / Find",
                "Workspace",
                "Search messages in the current chat",
                Some("ctrl+f"),
                &["find", "search", "chat search", "message search"],
            ),
        );

        let (id, name, description, hidden_tokens) = if thinking_visible {
            (
                "collapse-thinking",
                "Collapse Thinking",
                "Collapse assistant reasoning details",
                ["Hide thinking"],
            )
        } else {
            (
                "expand-thinking",
                "Expand Thinking",
                "Expand assistant reasoning details",
                ["Show thinking"],
            )
        };

        items.insert(
            items
                .iter()
                .position(|item| item.group == "Appearance")
                .unwrap_or(items.len()),
            app_action_item(
                id,
                name,
                "Appearance",
                description,
                Some("ctrl+x e"),
                &hidden_tokens,
            ),
        );

        let (id, name, description, hidden_tokens) = if compact_mode {
            (
                "disable-compact-mode",
                "Disable Compact Mode",
                "Show timestamps and full assistant metadata",
                ["compact mode", "expand"],
            )
        } else {
            (
                "enable-compact-mode",
                "Enable Compact Mode",
                "Hide timestamps and reduce assistant metadata",
                ["compact mode", "collapse"],
            )
        };

        items.insert(
            items
                .iter()
                .position(|item| item.group == "Appearance")
                .unwrap_or(items.len()),
            app_action_item(id, name, "Appearance", description, None, &hidden_tokens),
        );
    }

    items.insert(
        items
            .iter()
            .position(|item| item.group == "Appearance")
            .unwrap_or(items.len()),
        app_action_item(
            "cycle-reasoning-effort",
            "Variant Cycle",
            "Model",
            "Cycle reasoning effort for the active model",
            Some("ctrl+t"),
            &["reasoning effort", "cycle reasoning effort"],
        ),
    );

    items.insert(
        items
            .iter()
            .position(|item| item.group == "Application")
            .unwrap_or(items.len()),
        app_action_item(
            "open-mcp-dialog",
            "MCP Servers",
            "Application",
            "View and toggle configured MCP servers",
            None,
            &["mcp", "model context protocol", "servers"],
        ),
    );

    items.insert(
        items
            .iter()
            .position(|item| item.group == "Application")
            .unwrap_or(items.len()),
        app_action_item(
            "open-jobs",
            "Background Jobs",
            "Application",
            "List and manage background/interactive shell jobs",
            Some("ctrl+x j"),
            &["jobs", "background", "bash_output", "bash_kill", "process"],
        ),
    );

    items.insert(
        items
            .iter()
            .position(|item| item.group == "Application")
            .unwrap_or(items.len()),
        app_action_item(
            "open-storage",
            "Storage",
            "Application",
            "Inspect Crabcode disk usage",
            None,
            &[],
        ),
    );

    items
}

fn custom_command_items(registry: &Registry, is_chat: bool) -> Vec<DialogItem> {
    let mut items: Vec<DialogItem> = registry
        .list_commands()
        .into_iter()
        .filter(|command| registry.is_custom_command(&command.name))
        .filter(|command| is_chat || !command.chat_only)
        .filter(|command| !is_skill_backed_command(registry, &command.name))
        .map(|command| {
            let custom = registry.custom_command(&command.name);
            DialogItem {
                id: command.name.clone(),
                name: humanize_command_name(&command.name),
                group: "Commands".to_string(),
                description: if command.description.trim().is_empty() {
                    "Run configured command".to_string()
                } else {
                    command.description.clone()
                },
                tip: custom.and_then(custom_command_source_tip),
                provider_id: String::new(),
                active: false,
            }
        })
        .collect();

    items.sort_by(|left, right| left.name.cmp(&right.name));
    items
}

fn is_skill_backed_command(registry: &Registry, command_name: &str) -> bool {
    if registry.is_custom_command(command_name) {
        return false;
    }

    if command_name == "skills" {
        return true;
    }

    crate::skill::get_skill_store()
        .and_then(|store| store.get(command_name))
        .is_some()
}

fn custom_command_source_tip(command: &crate::command::custom::CustomCommand) -> Option<String> {
    match &command.source {
        CustomCommandSource::Config(_) => Some("config".to_string()),
        CustomCommandSource::File(_) => Some("file".to_string()),
    }
}

fn app_action_item(
    id: &str,
    name: &str,
    group: &str,
    description: &str,
    tip: Option<&str>,
    hidden_tokens: &[&str],
) -> DialogItem {
    let provider_id = std::iter::once(APP_ACTION_PROVIDER)
        .chain(hidden_tokens.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");

    DialogItem {
        id: id.to_string(),
        name: name.to_string(),
        group: group.to_string(),
        description: description.to_string(),
        tip: tip.map(str::to_string),
        provider_id,
        active: false,
    }
}

fn humanize_command_name(name: &str) -> String {
    let parts: Vec<String> = name
        .split(|ch: char| matches!(ch, '-' | '_' | '/' | ':' | '.'))
        .filter(|part| !part.is_empty())
        .map(capitalize_ascii)
        .collect();

    if parts.is_empty() {
        name.to_string()
    } else {
        parts.join(" ")
    }
}

fn capitalize_ascii(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut out = String::new();
    out.extend(first.to_uppercase());
    out.push_str(chars.as_str());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::custom::{CustomCommand, CustomCommandSource};
    use crate::command::handlers::register_all_commands;
    use std::path::PathBuf;

    #[test]
    fn palette_hides_chat_only_commands_outside_chat() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, false, true, false);

        assert!(state.dialog.items.iter().any(|item| item.id == "models"));
        assert!(!state.dialog.items.iter().any(|item| item.id == "copy"));
        assert!(!state.dialog.items.iter().any(|item| item.id == "fork"));
        assert!(!state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "collapse-thinking" || item.id == "expand-thinking"));
    }

    #[test]
    fn palette_includes_chat_only_commands_in_chat() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, false);

        assert!(state.dialog.items.iter().any(|item| item.id == "copy"));
        assert!(state.dialog.items.iter().any(|item| item.id == "fork"));
        assert!(state.dialog.items.iter().any(|item| item.id == "move"));
        assert!(state.dialog.items.iter().any(|item| item.id == "open-find"));
    }

    #[test]
    fn palette_search_matches_hidden_command_tokens() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, false);
        state.dialog.set_search_query("branch");

        let matches = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|item| (item.id.as_str(), item.name.as_str()))
            .collect::<Vec<_>>();

        assert!(matches.contains(&("fork", "Fork Session")));
        assert!(!matches.iter().any(|(_, name)| name.contains("branch")));
    }

    #[test]
    fn palette_shows_collapse_thinking_when_thinking_is_visible() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, false);

        assert!(state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "collapse-thinking" && item.name == "Collapse Thinking"));
        assert!(!state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "expand-thinking"));
    }

    #[test]
    fn palette_shows_expand_thinking_when_thinking_is_hidden() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, false, false);

        assert!(state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "expand-thinking" && item.name == "Expand Thinking"));
        assert!(!state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "collapse-thinking"));
    }

    #[test]
    fn palette_shows_enable_compact_mode_when_disabled() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, false);

        assert!(state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "enable-compact-mode" && item.name == "Enable Compact Mode"));
        assert!(!state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "disable-compact-mode"));
    }

    #[test]
    fn palette_shows_disable_compact_mode_when_enabled() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, true);

        assert!(state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "disable-compact-mode" && item.name == "Disable Compact Mode"));
        assert!(!state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "enable-compact-mode"));
    }

    #[test]
    fn palette_search_matches_hidden_thinking_tokens() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, false, false);
        state.dialog.set_search_query("show thinking");

        let matches = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|item| (item.id.as_str(), item.name.as_str()))
            .collect::<Vec<_>>();

        assert!(matches.contains(&("expand-thinking", "Expand Thinking")));
        assert!(!matches
            .iter()
            .any(|(_, name)| name.contains("Show thinking")));

        state.refresh_items(&registry, true, true, false);
        state.dialog.set_search_query("hide thinking");

        let matches = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|item| (item.id.as_str(), item.name.as_str()))
            .collect::<Vec<_>>();

        assert!(matches.contains(&("collapse-thinking", "Collapse Thinking")));
        assert!(!matches
            .iter()
            .any(|(_, name)| name.contains("Hide thinking")));
    }

    #[test]
    fn palette_thinking_items_map_to_visibility_actions() {
        let collapse = app_action_item(
            "collapse-thinking",
            "Collapse Thinking",
            "Appearance",
            "Collapse assistant reasoning details",
            None,
            &["Hide thinking"],
        );
        let expand = app_action_item(
            "expand-thinking",
            "Expand Thinking",
            "Appearance",
            "Expand assistant reasoning details",
            None,
            &["Show thinking"],
        );

        assert_eq!(
            action_for_item(&collapse),
            CommandPaletteAction::RunAppAction(CommandPaletteAppAction::SetThinkingVisible(false))
        );
        assert_eq!(
            action_for_item(&expand),
            CommandPaletteAction::RunAppAction(CommandPaletteAppAction::SetThinkingVisible(true))
        );
    }

    #[test]
    fn palette_find_item_maps_to_open_find_action() {
        let item = app_action_item(
            "open-find",
            "Search / Find",
            "Workspace",
            "Search messages in the current chat",
            Some("ctrl+f"),
            &["find", "search"],
        );

        assert_eq!(
            action_for_item(&item),
            CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenFind)
        );
    }

    #[test]
    fn palette_includes_variants_and_status_commands() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, false, true, false);

        let variants = state
            .dialog
            .items
            .iter()
            .find(|item| item.id == "variants")
            .expect("variants should be listed");
        assert_eq!(variants.name, "Switch model variant");
        assert_eq!(variants.group, "Model");
        assert_eq!(
            action_for_item(variants),
            CommandPaletteAction::RunCommand("variants".to_string())
        );

        let status = state
            .dialog
            .items
            .iter()
            .find(|item| item.id == "status")
            .expect("status should be listed");
        assert_eq!(status.name, "Status");
        assert_eq!(status.group, "Application");
        assert_eq!(
            action_for_item(status),
            CommandPaletteAction::RunCommand("status".to_string())
        );
    }

    #[test]
    fn palette_search_matches_reasoning_effort_for_variant_actions() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, false, true, false);
        state.dialog.set_search_query("reasoning effort");

        let matches = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|item| (item.id.as_str(), item.name.as_str()))
            .collect::<Vec<_>>();

        assert!(matches.contains(&("variants", "Switch model variant")));
        assert!(matches.contains(&("cycle-reasoning-effort", "Variant Cycle")));
    }

    #[test]
    fn palette_includes_mcp_dialog_action() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, false, true, false);

        let item = state
            .dialog
            .items
            .iter()
            .find(|item| item.id == "open-mcp-dialog")
            .expect("MCP dialog should be listed");
        assert_eq!(item.name, "MCP Servers");
        assert_eq!(item.group, "Application");
        assert_eq!(
            action_for_item(item),
            CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenMcpDialog)
        );
    }

    #[test]
    fn palette_includes_agent_picker_action() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, false, true, false);

        let item = state
            .dialog
            .items
            .iter()
            .find(|item| item.id == "open-agents-dialog")
            .expect("agent picker should be listed");
        assert_eq!(item.name, "Switch Agent");
        assert_eq!(item.group, "Agent");
        assert_eq!(
            action_for_item(item),
            CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenAgentsDialog)
        );
    }

    #[test]
    fn palette_uses_command_center_labels_without_slashes() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, false);

        assert!(state
            .dialog
            .items
            .iter()
            .all(|item| !item.name.starts_with('/')));
        assert!(state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "models" && item.name == "Change Model"));
        assert!(!state.dialog.items.iter().any(|item| item.id == "skills"));
    }

    #[test]
    fn palette_includes_config_commands_grouped_as_commands() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        registry.register_custom(CustomCommand {
            name: "checkcodex-oauth".to_string(),
            description: Some("Check Codex OAuth".to_string()),
            agent: None,
            model: None,
            subtask: None,
            template: "check auth".to_string(),
            source: CustomCommandSource::Config(PathBuf::from("crabcode.jsonc")),
            workdir: PathBuf::from("."),
        });
        let mut state = init_command_palette();

        state.refresh_items(&registry, true, true, false);

        let custom = state
            .dialog
            .items
            .iter()
            .find(|item| item.id == "checkcodex-oauth")
            .expect("custom command should be listed");
        assert_eq!(custom.group, "Commands");
        assert_eq!(custom.name, "Checkcodex Oauth");
        assert_eq!(custom.tip.as_deref(), Some("config"));
    }
}
