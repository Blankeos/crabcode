use std::collections::HashMap;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, Frame};

use crate::config::McpServerConfig;
use crate::mcp::McpServerView;
use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogDetail, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum McpDialogAction {
    Toggle { server_id: String },
    Auth { server_id: String },
    ClearAuth { server_id: String },
    OpenBrowser,
    CopyLink { url: String },
    None,
}

#[derive(Debug, Clone)]
pub struct McpServerRow {
    pub item: DialogItem,
    pub oauth: bool,
    pub details: Vec<DialogDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum McpDialogPage {
    List,
    Actions { server_id: String },
    Auth { server_id: String },
}

#[derive(Debug)]
pub struct McpDialogState {
    pub dialog: Dialog,
    page: McpDialogPage,
    servers: Vec<DialogItem>,
    oauth: HashMap<String, bool>,
    details: HashMap<String, Vec<DialogDetail>>,
    list_selected: Option<String>,
    list_query: String,
    auth_url: Option<String>,
}

impl McpDialogState {
    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        let rows = items
            .into_iter()
            .map(|item| McpServerRow {
                oauth: true,
                details: Vec::new(),
                item,
            })
            .collect();
        let mut state = Self::empty(title);
        state.set_servers(rows);
        state
    }

    fn empty(title: impl Into<String>) -> Self {
        let mut dialog = Dialog::with_items(title, Vec::new()).with_actions(list_actions());
        dialog.title_note.clear();
        Self {
            dialog,
            page: McpDialogPage::List,
            servers: Vec::new(),
            oauth: HashMap::new(),
            details: HashMap::new(),
            list_selected: None,
            list_query: String::new(),
            auth_url: None,
        }
    }

    pub fn actions_open(&self) -> bool {
        matches!(self.page, McpDialogPage::Actions { .. })
    }

    pub fn auth_open(&self) -> bool {
        matches!(self.page, McpDialogPage::Auth { .. })
    }

    pub fn set_servers(&mut self, rows: Vec<McpServerRow>) {
        self.oauth = rows
            .iter()
            .map(|row| (row.item.id.clone(), row.oauth))
            .collect();
        self.details = rows
            .iter()
            .map(|row| (row.item.id.clone(), row.details.clone()))
            .collect();
        self.servers = rows.into_iter().map(|row| row.item).collect();
        match self.page.clone() {
            McpDialogPage::List => {
                self.dialog.set_items_preserve_ui(self.servers.clone());
                self.apply_list_chrome();
            }
            McpDialogPage::Actions { server_id } => {
                if self.servers.iter().any(|item| item.id == server_id) {
                    self.show_actions(&server_id);
                } else {
                    self.return_to_list();
                }
            }
            McpDialogPage::Auth { server_id } => {
                if !self.servers.iter().any(|item| item.id == server_id) {
                    self.return_to_list();
                }
            }
        }
    }

    pub fn return_to_list(&mut self) {
        if matches!(self.page, McpDialogPage::List) {
            return;
        }
        let selected = self.list_selected.clone();
        let query = std::mem::take(&mut self.list_query);
        self.page = McpDialogPage::List;
        self.auth_url = None;
        self.apply_list_chrome();
        self.dialog.set_items(self.servers.clone());
        self.dialog.restore_search_query(query);
        if let Some(id) = selected {
            self.dialog.select_item_by_key(&id, "");
        }
    }

    pub fn open_auth(&mut self, server_id: &str) -> bool {
        if !self.servers.iter().any(|item| item.id == server_id) {
            return false;
        }
        if matches!(self.page, McpDialogPage::List) {
            self.list_query = self.dialog.search_query.clone();
            self.list_selected = Some(server_id.to_string());
        }
        self.page = McpDialogPage::Auth {
            server_id: server_id.to_string(),
        };
        self.auth_url = None;
        self.dialog.set_search_visible(false);
        self.dialog.detail_rows = vec![DialogDetail::muted("Preparing the link…")];
        self.dialog.set_items(Vec::new());
        self.apply_auth_chrome(server_id);
        true
    }

    pub fn set_auth_url(&mut self, url: String) {
        if !self.auth_open() {
            return;
        }
        self.auth_url = Some(url.clone());
        self.dialog.detail_rows = vec![DialogDetail::link(url)];
        self.dialog.set_items(auth_items());
    }

    pub fn note_browser_opened(&mut self) {}

    /// Back from the login page to that server's actions.
    pub fn leave_auth(&mut self) {
        let McpDialogPage::Auth { server_id } = &self.page else {
            return;
        };
        let server_id = server_id.clone();
        self.auth_url = None;
        if self.servers.iter().any(|item| item.id == server_id) {
            self.show_actions(&server_id);
        } else {
            self.return_to_list();
        }
    }

    fn open_actions(&mut self, server_id: String) {
        if matches!(self.page, McpDialogPage::List) {
            self.list_query = self.dialog.search_query.clone();
            self.list_selected = Some(server_id.clone());
        }
        self.show_actions(&server_id);
    }

    fn show_actions(&mut self, server_id: &str) {
        let enabled = self.server_enabled(server_id);
        let oauth = self.oauth.get(server_id).copied().unwrap_or(false);
        let already = matches!(
            &self.page,
            McpDialogPage::Actions { server_id: current } if current == server_id
        );
        self.page = McpDialogPage::Actions {
            server_id: server_id.to_string(),
        };
        self.auth_url = None;
        self.dialog.set_search_visible(false);
        self.dialog.detail_rows = self.details.get(server_id).cloned().unwrap_or_default();
        let items = action_items(enabled, oauth);
        if already {
            self.dialog.set_items_preserve_ui(items);
        } else {
            self.dialog.set_items(items);
        }
        self.apply_actions_chrome(server_id);
    }

    fn apply_list_chrome(&mut self) {
        self.dialog.title = "MCP".to_string();
        self.dialog.title_note.clear();
        self.dialog.detail_rows.clear();
        self.dialog.actions = list_actions();
        self.dialog.set_search_visible(true);
    }

    fn apply_actions_chrome(&mut self, server_id: &str) {
        self.dialog.title = server_id.to_string();
        self.dialog.title_note = "MCP".to_string();
        self.dialog.actions = vec![
            DialogAction {
                label: "select".to_string(),
                key: "enter".to_string(),
            },
            DialogAction {
                label: "back".to_string(),
                key: "esc".to_string(),
            },
        ];
    }

    fn apply_auth_chrome(&mut self, server_id: &str) {
        self.dialog.title = "Reauthenticate".to_string();
        self.dialog.title_note = server_id.to_string();
        self.dialog.actions = vec![
            DialogAction {
                label: "select".to_string(),
                key: "enter".to_string(),
            },
            DialogAction {
                label: "back".to_string(),
                key: "esc".to_string(),
            },
        ];
    }

    fn server_enabled(&self, id: &str) -> bool {
        self.servers
            .iter()
            .find(|item| item.id == id)
            .is_some_and(|item| item.active)
    }

    fn toggle_selected(&self) -> McpDialogAction {
        self.dialog
            .get_selected()
            .map(|item| McpDialogAction::Toggle {
                server_id: item.id.clone(),
            })
            .unwrap_or(McpDialogAction::None)
    }

    fn selected_menu_action(&mut self) -> McpDialogAction {
        match &self.page {
            McpDialogPage::Actions { server_id } => {
                let server_id = server_id.clone();
                let Some(item) = self.dialog.get_selected() else {
                    return McpDialogAction::None;
                };
                match item.id.as_str() {
                    "reauth" => {
                        self.open_auth(&server_id);
                        McpDialogAction::Auth { server_id }
                    }
                    "clear" => McpDialogAction::ClearAuth { server_id },
                    "toggle" => McpDialogAction::Toggle { server_id },
                    _ => McpDialogAction::None,
                }
            }
            McpDialogPage::Auth { .. } => {
                let Some(item) = self.dialog.get_selected() else {
                    return McpDialogAction::None;
                };
                match item.id.as_str() {
                    "open" => McpDialogAction::OpenBrowser,
                    "copy" => self
                        .auth_url
                        .clone()
                        .map(|url| McpDialogAction::CopyLink { url })
                        .unwrap_or(McpDialogAction::None),
                    _ => McpDialogAction::None,
                }
            }
            McpDialogPage::List => McpDialogAction::None,
        }
    }
}

fn list_actions() -> Vec<DialogAction> {
    vec![
        DialogAction {
            label: "toggle".to_string(),
            key: "space".to_string(),
        },
        DialogAction {
            label: "actions".to_string(),
            key: "enter".to_string(),
        },
    ]
}

fn action_items(enabled: bool, oauth: bool) -> Vec<DialogItem> {
    let mut items = Vec::new();
    if oauth {
        items.push(menu_item(
            "reauth",
            "Reauthenticate",
            "Sign in again",
            false,
        ));
        items.push(menu_item(
            "clear",
            "Clear authentication",
            "Remove the saved login",
            false,
        ));
    }
    items.push(menu_item(
        "toggle",
        if enabled { "Disable" } else { "Enable" },
        if enabled {
            "Stop using this server"
        } else {
            "Start using this server"
        },
        enabled,
    ));
    items
}

fn auth_items() -> Vec<DialogItem> {
    vec![
        menu_item("open", "Open browser", "", false),
        menu_item("copy", "Copy link", "", false),
    ]
}

fn menu_item(id: &str, name: &str, description: &str, active: bool) -> DialogItem {
    DialogItem {
        id: id.to_string(),
        name: name.to_string(),
        group: String::new(),
        description: description.to_string(),
        tip: None,
        provider_id: String::new(),
        active,
    }
}

pub fn mcp_server_details(
    name: &str,
    config: Option<&McpServerConfig>,
    live: Option<&McpServerView>,
    config_location: Option<String>,
) -> Vec<DialogDetail> {
    let status = live.map(|view| view.status.as_str()).unwrap_or(
        if config.is_some_and(|config| config.enabled()) {
            "enabled"
        } else {
            "disabled"
        },
    );
    let failure = live.and_then(|view| view.detail.as_deref()).unwrap_or("");
    let status_ok = status == "connected";
    let status_value = if status == "failed" && !failure.is_empty() {
        format!("failed · {failure}")
    } else {
        status_label(status).to_string()
    };
    let (endpoint_label, endpoint) = match config {
        Some(McpServerConfig::Remote(remote)) => ("URL".to_string(), remote.url.clone()),
        Some(McpServerConfig::Local(local)) => ("Command".to_string(), local.command.join(" ")),
        None => ("URL".to_string(), "—".to_string()),
    };
    let capabilities = match live {
        Some(view) if !view.capabilities.is_empty() => view.capabilities.clone(),
        Some(view) if view.tool_count > 0 => "tools".to_string(),
        _ => "—".to_string(),
    };
    let tools = match live {
        Some(view) if view.status == "connected" => view.tool_count.to_string(),
        _ => "—".to_string(),
    };
    let auth = config
        .map(|config| crate::mcp::auth_status(name, config).to_string())
        .unwrap_or_else(|| "—".to_string());
    let auth_ok = auth == "authenticated";
    vec![
        marked("Status", status_value, status_ok),
        marked("Auth", auth, auth_ok),
        DialogDetail::new(endpoint_label, endpoint),
        DialogDetail::new(
            "Config",
            config_location.unwrap_or_else(|| "Not in a config file".to_string()),
        ),
        DialogDetail::new("Capabilities", capabilities),
        DialogDetail::new("Tools", tools),
    ]
}

fn marked(label: &str, value: String, ok: bool) -> DialogDetail {
    if ok {
        DialogDetail::ok(label, value)
    } else {
        DialogDetail::new(label, value)
    }
}

pub(crate) fn status_label(status: &str) -> &str {
    match status {
        "needs_auth" => "needs authentication",
        other => other,
    }
}

pub fn init_mcp_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> McpDialogState {
    McpDialogState::with_items(title, items)
}

pub fn render_mcp_dialog(
    f: &mut Frame,
    dialog_state: &mut McpDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_mcp_dialog_key_event(
    dialog_state: &mut McpDialogState,
    event: KeyEvent,
) -> McpDialogAction {
    if !dialog_state.dialog.is_visible() {
        return McpDialogAction::None;
    }

    if dialog_state.auth_open() {
        return match event.code {
            KeyCode::Esc => {
                dialog_state.leave_auth();
                McpDialogAction::None
            }
            KeyCode::Enter => dialog_state.selected_menu_action(),
            KeyCode::Up | KeyCode::Down => {
                dialog_state.dialog.handle_key_event(event);
                McpDialogAction::None
            }
            _ => McpDialogAction::None,
        };
    }

    if dialog_state.actions_open() {
        return match event.code {
            KeyCode::Esc => {
                dialog_state.return_to_list();
                McpDialogAction::None
            }
            KeyCode::Enter => dialog_state.selected_menu_action(),
            KeyCode::Up | KeyCode::Down => {
                dialog_state.dialog.handle_key_event(event);
                McpDialogAction::None
            }
            _ => McpDialogAction::None,
        };
    }

    match event.code {
        KeyCode::Char(' ') if event.modifiers == KeyModifiers::NONE => {
            dialog_state.toggle_selected()
        }
        KeyCode::Enter => {
            if let Some(id) = dialog_state
                .dialog
                .get_selected()
                .map(|item| item.id.clone())
            {
                dialog_state.open_actions(id);
            }
            McpDialogAction::None
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
            McpDialogAction::None
        }
    }
}

pub fn handle_mcp_dialog_mouse_event(
    dialog_state: &mut McpDialogState,
    event: MouseEvent,
) -> McpDialogAction {
    if dialog_state.auth_open() && matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        if let Some(url) = dialog_state.dialog.detail_link_at(event.column, event.row) {
            return McpDialogAction::CopyLink { url };
        }
    }
    let clicked_item = matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
        && dialog_state
            .dialog
            .item_index_at_position(event.column, event.row)
            .is_some();
    if !dialog_state.dialog.handle_mouse_event(event) {
        return McpDialogAction::None;
    }
    if !dialog_state.dialog.is_visible() {
        dialog_state.return_to_list();
        return McpDialogAction::None;
    }
    if !clicked_item {
        return McpDialogAction::None;
    }
    if dialog_state.actions_open() || dialog_state.auth_open() {
        return dialog_state.selected_menu_action();
    }
    if let Some(id) = dialog_state
        .dialog
        .get_selected()
        .map(|item| item.id.clone())
    {
        dialog_state.open_actions(id);
    }
    McpDialogAction::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn server(id: &str, enabled: bool) -> DialogItem {
        DialogItem {
            id: id.to_string(),
            name: id.to_string(),
            group: "MCP".to_string(),
            description: String::new(),
            tip: None,
            provider_id: String::new(),
            active: enabled,
        }
    }

    fn open_list() -> McpDialogState {
        let mut state =
            init_mcp_dialog("MCP", vec![server("sentry", true), server("local", false)]);
        state.dialog.show();
        state
    }

    #[test]
    fn space_toggles_and_letters_search_instead_of_authenticating() {
        let mut state = open_list();
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Char(' '))),
            McpDialogAction::Toggle {
                server_id: "sentry".to_string()
            }
        );
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Char('a'))),
            McpDialogAction::None
        );
        assert_eq!(state.dialog.search_query, "a");
        assert!(!state.actions_open());
    }

    #[test]
    fn enter_opens_actions_then_a_reauth_page_with_the_link() {
        let mut state = open_list();
        state.details.insert(
            "sentry".to_string(),
            vec![DialogDetail::ok("Status", "connected")],
        );
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::None
        );
        assert!(state.actions_open());
        assert_eq!(
            state.dialog.detail_rows,
            vec![DialogDetail::ok("Status", "connected")]
        );
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::Auth {
                server_id: "sentry".to_string()
            }
        );
        assert!(state.auth_open());
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::None
        );

        state.set_auth_url("https://auth.example/authorize".to_string());
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::OpenBrowser
        );
        assert!(state.auth_open());
        handle_mcp_dialog_key_event(&mut state, key(KeyCode::Down));
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::CopyLink {
                url: "https://auth.example/authorize".to_string()
            }
        );

        handle_mcp_dialog_key_event(&mut state, key(KeyCode::Esc));
        assert!(state.actions_open());
        assert!(!state.auth_open());
    }

    #[test]
    fn action_menu_clears_auth_disables_and_goes_back() {
        let mut state = open_list();
        handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter));

        handle_mcp_dialog_key_event(&mut state, key(KeyCode::Down));
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::ClearAuth {
                server_id: "sentry".to_string()
            }
        );
        assert!(state.actions_open());

        handle_mcp_dialog_key_event(&mut state, key(KeyCode::Down));
        assert_eq!(
            handle_mcp_dialog_key_event(&mut state, key(KeyCode::Enter)),
            McpDialogAction::Toggle {
                server_id: "sentry".to_string()
            }
        );
        assert!(state.actions_open());

        handle_mcp_dialog_key_event(&mut state, key(KeyCode::Esc));
        assert!(!state.actions_open());
        assert!(state.dialog.is_visible());
        assert_eq!(
            state.dialog.get_selected().map(|item| item.id.as_str()),
            Some("sentry")
        );
    }
}
