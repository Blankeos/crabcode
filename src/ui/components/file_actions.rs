//! Cursor-anchored file actions, styled like the chat selection action bar.
use crate::{theme, ui::hyperlink::FileHyperlinkTarget};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAction {
    Open,
    Reveal,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileActionEvent {
    None,
    Dismiss,
    Choose(FileAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkActionTarget {
    File(FileHyperlinkTarget),
    Url(String),
}

#[derive(Clone, Debug)]
pub struct FileActions {
    pub target: LinkActionTarget,
    anchor: Position,
    selected: usize,
}

pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Reveal in File Explorer"
    } else {
        "Reveal in File Manager"
    }
}

const FILE_MENU: [FileAction; 3] = [FileAction::Open, FileAction::Reveal, FileAction::Copy];
const URL_MENU: [FileAction; 2] = [FileAction::Open, FileAction::Copy];

impl FileActions {
    pub fn new(target: FileHyperlinkTarget, anchor: Position) -> Self {
        Self {
            target: LinkActionTarget::File(target),
            anchor,
            selected: 0,
        }
    }

    pub fn for_url(url: String, anchor: Position) -> Self {
        Self {
            target: LinkActionTarget::Url(url),
            anchor,
            selected: 0,
        }
    }

    pub fn file_target(&self) -> Option<&FileHyperlinkTarget> {
        match &self.target {
            LinkActionTarget::File(target) => Some(target),
            LinkActionTarget::Url(_) => None,
        }
    }

    /// Clipboard payload deliberately excludes editor-only line/column suffixes.
    pub fn copy_payload(&self) -> String {
        match &self.target {
            LinkActionTarget::File(target) => target.path.to_string_lossy().into_owned(),
            LinkActionTarget::Url(url) => url.clone(),
        }
    }

    pub fn actions(&self) -> &'static [FileAction] {
        match self.target {
            LinkActionTarget::File(_) => &FILE_MENU,
            LinkActionTarget::Url(_) => &URL_MENU,
        }
    }

    fn labels(&self) -> Vec<(&'static str, &'static str)> {
        match self.target {
            LinkActionTarget::File(_) => vec![
                ("o", "Open"),
                ("r", reveal_label()),
                ("y", "Copy absolute path"),
            ],
            LinkActionTarget::Url(_) => vec![("o", "Open"), ("y", "Copy URL")],
        }
    }

    fn choose_if_present(&self, action: FileAction) -> FileActionEvent {
        if self.actions().contains(&action) {
            FileActionEvent::Choose(action)
        } else {
            FileActionEvent::None
        }
    }

    // Use rows instead of a long single line so each action stays reachable on
    // narrow terminals. Shortcuts and the focused row share the selection-bar styling.
    pub fn area(&self, screen: Rect) -> Rect {
        let width = self
            .labels()
            .iter()
            .map(|(_, label)| label.len() as u16 + 4)
            .max()
            .unwrap_or(0)
            .min(screen.width);
        let height = (self.actions().len() as u16).min(screen.height);
        let x = self
            .anchor
            .x
            .clamp(screen.x, screen.right().saturating_sub(width).max(screen.x));
        let preferred_y = self.anchor.y.saturating_add(1);
        let y = if preferred_y.saturating_add(height) <= screen.bottom() {
            preferred_y
        } else {
            self.anchor.y.saturating_sub(height)
        };
        Rect::new(
            x,
            y.clamp(
                screen.y,
                screen.bottom().saturating_sub(height).max(screen.y),
            ),
            width,
            height,
        )
    }

    pub fn action_at(&self, screen: Rect, position: Position) -> Option<FileAction> {
        let area = self.area(screen);
        if !area.contains(position) {
            return None;
        }
        self.actions().get((position.y - area.y) as usize).copied()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> FileActionEvent {
        let count = self.actions().len();
        match key.code {
            KeyCode::Esc => FileActionEvent::Dismiss,
            KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                self.selected = (self.selected + count - 1) % count;
                FileActionEvent::None
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.selected = (self.selected + count - 1) % count;
                FileActionEvent::None
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                self.selected = (self.selected + 1) % count;
                FileActionEvent::None
            }
            KeyCode::Home => {
                self.selected = 0;
                FileActionEvent::None
            }
            KeyCode::End => {
                self.selected = count - 1;
                FileActionEvent::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                FileActionEvent::Choose(self.actions()[self.selected])
            }
            KeyCode::Char('o') => self.choose_if_present(FileAction::Open),
            KeyCode::Char('r') => self.choose_if_present(FileAction::Reveal),
            KeyCode::Char('y') => self.choose_if_present(FileAction::Copy),
            _ => FileActionEvent::None,
        }
    }

    pub fn handle_mouse(&mut self, screen: Rect, mouse: MouseEvent) -> FileActionEvent {
        let action = self.action_at(screen, Position::new(mouse.column, mouse.row));
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => action
                .map(FileActionEvent::Choose)
                .unwrap_or(FileActionEvent::Dismiss),
            MouseEventKind::Down(_) if action.is_none() => FileActionEvent::Dismiss,
            MouseEventKind::Moved => {
                if let Some(action) = action {
                    self.selected = self
                        .actions()
                        .iter()
                        .position(|item| *item == action)
                        .unwrap();
                }
                FileActionEvent::None
            }
            _ => FileActionEvent::None,
        }
    }

    pub fn render(&self, frame: &mut Frame, colors: &theme::ThemeColors) {
        let area = self.area(frame.area());
        frame.render_widget(Clear, area);
        let bg = colors.info;
        let fg = theme::contrast_text(bg);
        let base = Style::default().fg(fg).bg(bg);
        let labels = self.labels();
        for (index, (key, label)) in labels.iter().take(area.height as usize).enumerate() {
            let row = if index == self.selected {
                base.add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                base
            };
            let line = Line::from(vec![
                Span::raw(" "),
                Span::styled(*key, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {label} ")),
            ]);
            // Paragraph styles its entire area, including padding after short labels.
            frame.render_widget(
                Paragraph::new(line).style(row),
                Rect::new(area.x, area.y + index as u16, area.width, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn popup() -> FileActions {
        FileActions::new(
            FileHyperlinkTarget {
                path: "file.txt".into(),
                line: Some(4),
                column: Some(2),
            },
            Position::new(79, 23),
        )
    }
    #[test]
    fn renders_all_actions_over_existing_content() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut popup = popup();
        popup.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let colors = theme::Theme::bundled_themes()[0].get_colors(true);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let screen = Rect::new(0, 0, 80, 24);
        let area = popup.area(screen);
        let labels = popup.labels();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("underlying content"), screen);
                popup.render(frame, &colors);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (row, (key, label)) in labels.iter().enumerate() {
            let rendered: String = (area.x..area.right())
                .map(|x| buffer[(x, area.y + row as u16)].symbol())
                .collect();
            assert_eq!(rendered.trim(), format!("{key} {label}"));
        }
        assert!(buffer[(area.x + 1, area.y + 1)]
            .modifier
            .contains(Modifier::REVERSED));
        assert!(!buffer[(area.x + 1, area.y)]
            .modifier
            .contains(Modifier::REVERSED));
    }

    #[test]
    fn selected_highlight_fills_entire_row_for_every_action() {
        use ratatui::{backend::TestBackend, Terminal};

        let colors = theme::Theme::bundled_themes()[0].get_colors(true);
        for width in [12, 80] {
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            let mut popup = popup();
            for selected in 0..3 {
                popup.selected = selected;
                let area = popup.area(Rect::new(0, 0, width, 24));
                terminal.draw(|frame| popup.render(frame, &colors)).unwrap();
                let buffer = terminal.backend().buffer();
                for row in 0..area.height {
                    for x in area.x..area.right() {
                        let cell = &buffer[(x, area.y + row)];
                        assert_eq!(cell.bg, colors.info);
                        assert_eq!(cell.fg, theme::contrast_text(colors.info));
                        assert_eq!(
                            cell.modifier.contains(Modifier::REVERSED),
                            row as usize == selected,
                            "width {width}, selected {selected}, cell ({x}, {row})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn geometry_and_action_hit_testing() {
        let popup = popup();
        let screen = Rect::new(0, 0, 80, 24);
        let area = popup.area(screen);
        assert!(area.right() <= screen.right());
        assert!(area.bottom() <= screen.bottom());
        for (row, action) in popup.actions().iter().enumerate() {
            assert_eq!(
                popup.action_at(screen, Position::new(area.x, area.y + row as u16)),
                Some(*action)
            );
        }
        assert_eq!(
            popup.action_at(screen, Position::new(area.right(), area.y)),
            None
        );
        for screen in [
            Rect::new(5, 6, 1, 1),
            Rect::default(),
            Rect::new(3, 4, 12, 2),
        ] {
            let area = popup.area(screen);
            assert!(area.right() <= screen.right());
            assert!(area.bottom() <= screen.bottom());
        }
    }
    #[test]
    fn keyboard_navigation_and_escape() {
        let mut popup = popup();
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            FileActionEvent::None
        );
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FileActionEvent::Choose(FileAction::Reveal)
        );
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            FileActionEvent::Dismiss
        );
    }
    #[test]
    fn keyboard_wraps_and_shortcuts_select_actions() {
        let mut popup = popup();
        popup.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FileActionEvent::Choose(FileAction::Copy)
        );
        popup.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FileActionEvent::Choose(FileAction::Open)
        );
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            FileActionEvent::Choose(FileAction::Reveal)
        );
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            FileActionEvent::Choose(FileAction::Copy)
        );
    }

    #[test]
    fn mouse_release_does_not_activate_and_outside_click_dismisses() {
        let mut popup = popup();
        let screen = Rect::new(0, 0, 80, 24);
        let area = popup.area(screen);
        let mouse = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(popup.handle_mouse(screen, mouse), FileActionEvent::None);
        assert_eq!(
            popup.handle_mouse(
                screen,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    ..mouse
                }
            ),
            FileActionEvent::Choose(FileAction::Open)
        );
        assert_eq!(
            popup.handle_mouse(
                screen,
                MouseEvent {
                    column: 0,
                    row: 0,
                    kind: MouseEventKind::Down(MouseButton::Left),
                    ..mouse
                }
            ),
            FileActionEvent::Dismiss
        );
    }

    #[test]
    fn url_menu_offers_open_and_copy_without_reveal() {
        let mut popup =
            FileActions::for_url("https://example.com/docs".into(), Position::new(2, 2));
        let screen = Rect::new(0, 0, 80, 24);
        assert_eq!(popup.area(screen).height, 2);
        assert_eq!(popup.actions(), &[FileAction::Open, FileAction::Copy]);
        assert_eq!(
            popup
                .labels()
                .into_iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>(),
            vec!["Open", "Copy URL"]
        );
        assert_eq!(popup.copy_payload(), "https://example.com/docs");
        assert!(popup.file_target().is_none());
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            FileActionEvent::None
        );
        assert_eq!(
            popup.action_at(
                screen,
                Position::new(popup.area(screen).x, popup.area(screen).y + 1)
            ),
            Some(FileAction::Copy)
        );
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            FileActionEvent::None
        );
        assert_eq!(
            popup.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FileActionEvent::Choose(FileAction::Copy)
        );
    }
}
