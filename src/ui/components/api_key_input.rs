use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    prelude::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use tui_textarea::TextArea;

use crate::theme::ThemeColors;
use crate::ui::textarea_keys::input_textarea;

#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    Submitted {
        api_key: String,
        provider_name: String,
    },
    Cancelled,
    Continue,
}

#[derive(Debug)]
pub struct ApiKeyInput {
    pub visible: bool,
    pub provider_name: String,
    pub text_area: TextArea<'static>,
}

impl ApiKeyInput {
    pub fn new() -> Self {
        let mut text_area = TextArea::default();
        text_area.set_placeholder_text("Paste here");
        Self {
            visible: false,
            provider_name: String::new(),
            text_area,
        }
    }

    pub fn show(&mut self, provider_name: impl Into<String>) {
        self.visible = true;
        self.provider_name = provider_name.into();
        self.text_area = TextArea::default();
        self.text_area.set_placeholder_text("Paste here");
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.provider_name.clear();
        self.text_area = TextArea::default();
        self.text_area.set_placeholder_text("Paste here");
    }

    pub fn get_api_key(&self) -> String {
        self.text_area.lines().join("\n")
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> InputAction {
        if !self.visible {
            return InputAction::Continue;
        }

        match event.code {
            KeyCode::Esc => {
                self.hide();
                InputAction::Cancelled
            }
            KeyCode::Enter => {
                let api_key = self.get_api_key();
                if !api_key.trim().is_empty() {
                    let provider_name = self.provider_name.clone();
                    self.hide();
                    InputAction::Submitted {
                        api_key,
                        provider_name,
                    }
                } else {
                    InputAction::Continue
                }
            }
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => InputAction::Continue,
            _ => {
                if event.kind == KeyEventKind::Press {
                    input_textarea(&mut self.text_area, event);
                }
                InputAction::Continue
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, colors: &ThemeColors) {
        if !self.visible {
            return;
        }

        const DIALOG_WIDTH: u16 = 50;
        const DIALOG_HEIGHT: u16 = 10;

        let dialog_width = area.width.min(DIALOG_WIDTH);
        let dialog_height = area.height.min(DIALOG_HEIGHT);

        let dialog_area = Rect {
            x: (area.width - dialog_width) / 2,
            y: (area.height - dialog_height) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        frame.render_widget(Clear, dialog_area);

        const PADDING: u16 = 2;
        let content_area = Rect {
            x: dialog_area.x + PADDING,
            y: dialog_area.y + PADDING,
            width: dialog_area.width.saturating_sub(PADDING * 2),
            height: dialog_area.height.saturating_sub(PADDING * 2),
        };

        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
            dialog_area,
        );

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(3),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(content_area);

        let esc_text = "esc";
        let esc_area_width = (esc_text.len() as u16).saturating_add(1);
        let header_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(esc_area_width),
            ])
            .split(chunks[0]);

        let title_paragraph = Paragraph::new(Line::from(vec![Span::styled(
            "API key",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(title_paragraph, header_chunks[0]);

        let esc_paragraph = Paragraph::new(Line::from(vec![Span::styled(
            esc_text,
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(esc_paragraph, header_chunks[1]);

        frame.render_widget(&self.text_area, chunks[1]);

        // Hardware cursor follows the text field so terminal cursor effects
        // (e.g. ghostty shaders) and IME popups track typing, instead of
        // staying stuck on the chat input behind the dialog.
        if chunks[1].width > 0 && chunks[1].height > 0 {
            use unicode_width::UnicodeWidthStr;
            let (row, col) = self.text_area.cursor();
            let line = self.text_area.lines().get(row).cloned().unwrap_or_default();
            let prefix: String = line.chars().take(col).collect();
            let x = chunks[1]
                .x
                .saturating_add((prefix.width() as u16).min(chunks[1].width.saturating_sub(1)));
            let y = chunks[1]
                .y
                .saturating_add((row as u16).min(chunks[1].height.saturating_sub(1)));
            frame.set_cursor_position((x, y));
        }

        let footer_line = Line::from(vec![
            Span::styled(
                "enter",
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " submit",
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ),
        ]);

        frame.render_widget(Paragraph::new(footer_line), chunks[2]);
    }
}

impl Default for ApiKeyInput {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ApiKeyInput {
    fn clone(&self) -> Self {
        Self {
            visible: self.visible,
            provider_name: self.provider_name.clone(),
            text_area: self.text_area.clone(),
        }
    }
}
