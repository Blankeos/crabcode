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
    CycleReasoningEffort,
    OpenStorage,
    OpenSkillsDialog,
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

    pub fn refresh_items(&mut self, registry: &Registry, is_chat: bool) {
        let was_visible = self.dialog.is_visible();
        let search_query = self.dialog.search_query.clone();
        let selected = self
            .dialog
            .get_selected()
            .map(|item| (item.id.clone(), item.provider_id.clone()));

        let mut items = core_palette_items(registry, is_chat);
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
            ),
        );

        items.extend(custom_command_items(registry, is_chat));

        self.dialog = Dialog::with_items("Command Palette", items).with_actions(base_actions());
        self.dialog.set_search_query(search_query);

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
    if item.provider_id == APP_ACTION_PROVIDER {
        return match item.id.as_str() {
            "toggle-agent-mode" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::ToggleAgentMode)
            }
            "cycle-reasoning-effort" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::CycleReasoningEffort)
            }
            "open-storage" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenStorage)
            }
            "open-skills-dialog" => {
                CommandPaletteAction::RunAppAction(CommandPaletteAppAction::OpenSkillsDialog)
            }
            _ => CommandPaletteAction::None,
        };
    }

    CommandPaletteAction::RunCommand(item.id.clone())
}

fn core_palette_items(registry: &Registry, is_chat: bool) -> Vec<DialogItem> {
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
            "home",
            "Go Home",
            "Workspace",
            "Return to a blank home screen",
        ),
        ("models", "Change Model", "Model", "Choose the active model"),
        (
            "connect",
            "Connect Provider",
            "Model",
            "Add or update provider credentials",
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
            provider_id: String::new(),
        });
    }

    items.insert(
        2.min(items.len()),
        app_action_item(
            "toggle-agent-mode",
            "Toggle Agent Mode",
            "Workspace",
            "Switch between Build and Plan",
            Some("tab"),
        ),
    );

    items.insert(
        items
            .iter()
            .position(|item| item.group == "Appearance")
            .unwrap_or(items.len()),
        app_action_item(
            "cycle-reasoning-effort",
            "Cycle Reasoning Effort",
            "Model",
            "Switch reasoning effort for the active model",
            Some("ctrl+t"),
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
) -> DialogItem {
    DialogItem {
        id: id.to_string(),
        name: name.to_string(),
        group: group.to_string(),
        description: description.to_string(),
        tip: tip.map(str::to_string),
        provider_id: APP_ACTION_PROVIDER.to_string(),
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

        state.refresh_items(&registry, false);

        assert!(state.dialog.items.iter().any(|item| item.id == "models"));
        assert!(!state.dialog.items.iter().any(|item| item.id == "copy"));
    }

    #[test]
    fn palette_includes_chat_only_commands_in_chat() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true);

        assert!(state.dialog.items.iter().any(|item| item.id == "copy"));
    }

    #[test]
    fn palette_uses_command_center_labels_without_slashes() {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        let mut state = init_command_palette();

        state.refresh_items(&registry, true);

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

        state.refresh_items(&registry, true);

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
