use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    prelude::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use tui_textarea::{Input as TuiInput, TextArea};

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
                    let input = TuiInput::from(event);
                    self.text_area.input(input);
                }
                InputAction::Continue
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
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
            Paragraph::new("").style(Style::default().bg(Color::Rgb(20, 20, 30))),
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

        let title_line = Line::from(vec![
            Span::styled(
                "API key",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(40)),
            Span::styled(
                "esc",
                Style::default()
                    .fg(Color::Rgb(255, 140, 0))
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        frame.render_widget(Paragraph::new(title_line), chunks[0]);
        frame.render_widget(&self.text_area, chunks[1]);

        let footer_line = Line::from(vec![Span::styled(
            "enter submit",
            Style::default()
                .fg(Color::Rgb(150, 120, 100))
                .add_modifier(Modifier::DIM),
        )]);

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
