use crate::theme::{contrast_text, ThemeColors};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
    Frame,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use tokio::sync::oneshot;

#[derive(Clone, Debug)]
struct QuestionOption {
    label: String,
    description: String,
}

#[derive(Clone, Debug)]
struct QuestionItem {
    header: String,
    question: String,
    options: Vec<QuestionOption>,
    multiple: bool,
    custom: bool,
}

#[derive(Clone, Debug)]
struct QuestionAnswerState {
    selected: Vec<bool>,
    cursor: usize,
    custom_text: String,
    custom_cursor: usize,
    custom_selected: bool,
}

fn char_kind(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_ascii_punctuation() {
        1
    } else {
        2
    }
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn char_to_byte(text: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }

    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn insert_char_at_cursor(text: &mut String, cursor: &mut usize, ch: char) {
    let len = char_count(text);
    *cursor = (*cursor).min(len);
    let byte_idx = char_to_byte(text, *cursor);
    text.insert(byte_idx, ch);
    *cursor += 1;
}

fn delete_char_before_cursor(text: &mut String, cursor: &mut usize) {
    let len = char_count(text);
    *cursor = (*cursor).min(len);
    if *cursor == 0 {
        return;
    }

    let start = char_to_byte(text, *cursor - 1);
    let end = char_to_byte(text, *cursor);
    text.replace_range(start..end, "");
    *cursor -= 1;
}

fn delete_word_before_cursor(text: &mut String, cursor: &mut usize) {
    let mut chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());
    if *cursor == 0 {
        return;
    }

    let end = *cursor;
    let mut start = end;
    while start > 0 && chars[start - 1].is_whitespace() {
        start -= 1;
    }

    if start > 0 {
        let kind = char_kind(chars[start - 1]);
        while start > 0 && !chars[start - 1].is_whitespace() && char_kind(chars[start - 1]) == kind
        {
            start -= 1;
        }
    }

    chars.drain(start..end);
    *text = chars.into_iter().collect();
    *cursor = start;
}

fn move_word_left(text: &str, cursor: &mut usize) {
    let chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());

    while *cursor > 0 && chars[*cursor - 1].is_whitespace() {
        *cursor -= 1;
    }

    if *cursor > 0 {
        let kind = char_kind(chars[*cursor - 1]);
        while *cursor > 0
            && !chars[*cursor - 1].is_whitespace()
            && char_kind(chars[*cursor - 1]) == kind
        {
            *cursor -= 1;
        }
    }
}

fn move_word_right(text: &str, cursor: &mut usize) {
    let chars: Vec<char> = text.chars().collect();
    *cursor = (*cursor).min(chars.len());

    while *cursor < chars.len() && chars[*cursor].is_whitespace() {
        *cursor += 1;
    }

    if *cursor < chars.len() {
        let kind = char_kind(chars[*cursor]);
        while *cursor < chars.len()
            && !chars[*cursor].is_whitespace()
            && char_kind(chars[*cursor]) == kind
        {
            *cursor += 1;
        }
    }
}

struct QuestionDialogRequest {
    questions: Vec<QuestionItem>,
    answers: Vec<QuestionAnswerState>,
    response_tx: oneshot::Sender<Value>,
    current_index: usize,
    editing_custom: bool,
}

pub struct QuestionDialogState {
    current: Option<QuestionDialogRequest>,
    queue: VecDeque<QuestionDialogRequest>,
}

pub enum QuestionDialogAction {
    Submit,
    Cancel,
    Handled,
    NotHandled,
}

pub fn init_question_dialog() -> QuestionDialogState {
    QuestionDialogState::new()
}

impl QuestionDialogState {
    pub fn new() -> Self {
        Self {
            current: None,
            queue: VecDeque::new(),
        }
    }

    pub fn enqueue(&mut self, questions: Value, response_tx: oneshot::Sender<Value>) {
        let request = QuestionDialogRequest::new(questions, response_tx);
        if self.current.is_none() {
            self.current = Some(request);
        } else {
            self.queue.push_back(request);
        }
    }

    pub fn has_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn submit_current(&mut self) {
        if let Some(request) = self.current.take() {
            let response = request.response();
            let _ = request.response_tx.send(response);
        }
        self.current = self.queue.pop_front();
    }

    pub fn cancel_current(&mut self) {
        if let Some(request) = self.current.take() {
            let response = request.empty_response();
            let _ = request.response_tx.send(response);
        }
        self.current = self.queue.pop_front();
    }

    pub fn clear_with_empty(&mut self) {
        if let Some(request) = self.current.take() {
            let response = request.empty_response();
            let _ = request.response_tx.send(response);
        }

        while let Some(request) = self.queue.pop_front() {
            let response = request.empty_response();
            let _ = request.response_tx.send(response);
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        let Some(request) = self.current.as_mut() else {
            return;
        };

        for ch in text.chars().filter(|ch| *ch != '\r') {
            request.insert_char(ch);
        }
    }

    fn active_mut(&mut self) -> Option<&mut QuestionDialogRequest> {
        self.current.as_mut()
    }

    fn active(&self) -> Option<&QuestionDialogRequest> {
        self.current.as_ref()
    }

    fn queued_count(&self) -> usize {
        self.queue.len()
    }
}

impl QuestionDialogRequest {
    fn new(questions: Value, response_tx: oneshot::Sender<Value>) -> Self {
        let questions = parse_questions(questions);
        let editing_custom = questions
            .first()
            .map(|question| question.options.is_empty())
            .unwrap_or(false);
        let answers = questions
            .iter()
            .map(QuestionAnswerState::for_question)
            .collect();

        Self {
            questions,
            answers,
            response_tx,
            current_index: 0,
            editing_custom,
        }
    }

    fn current_question(&self) -> Option<&QuestionItem> {
        self.questions.get(self.current_index)
    }

    fn current_answer(&self) -> Option<&QuestionAnswerState> {
        self.answers.get(self.current_index)
    }

    fn current_answer_mut(&mut self) -> Option<&mut QuestionAnswerState> {
        self.answers.get_mut(self.current_index)
    }

    fn focus_count(&self) -> usize {
        self.questions.len() + 1
    }

    fn is_confirm_tab(&self) -> bool {
        self.current_index == self.questions.len()
    }

    fn sync_editing_for_current_focus(&mut self) {
        self.editing_custom = self
            .current_question()
            .map(|question| question.options.is_empty())
            .unwrap_or(false);
    }

    fn current_is_text_entry(&self) -> bool {
        self.current_question()
            .map(|q| q.options.is_empty())
            .unwrap_or(false)
            || self.editing_custom
    }

    fn current_is_custom_row(&self) -> bool {
        let Some(question) = self.current_question() else {
            return false;
        };
        let Some(answer) = self.current_answer() else {
            return false;
        };

        question.custom && !question.options.is_empty() && answer.cursor == question.options.len()
    }

    fn previous_option(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        let count = option_row_count(question);
        if count == 0 {
            return;
        }

        let multiple = question.multiple;
        let options_len = question.options.len();
        if let Some(answer) = self.current_answer_mut() {
            answer.cursor = if answer.cursor == 0 {
                count - 1
            } else {
                answer.cursor - 1
            };
            if !multiple && answer.cursor < options_len {
                answer.select_cursor();
            }
        }
        self.editing_custom = false;
    }

    fn next_option(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        let count = option_row_count(question);
        if count == 0 {
            return;
        }

        let multiple = question.multiple;
        let options_len = question.options.len();
        if let Some(answer) = self.current_answer_mut() {
            answer.cursor = (answer.cursor + 1) % count;
            if !multiple && answer.cursor < options_len {
                answer.select_cursor();
            }
        }
        self.editing_custom = false;
    }

    fn previous_question(&mut self) {
        let focus_count = self.focus_count();
        if focus_count == 0 {
            return;
        }

        self.current_index = if self.current_index == 0 {
            focus_count - 1
        } else {
            self.current_index - 1
        };
        self.sync_editing_for_current_focus();
    }

    fn next_question(&mut self) {
        let focus_count = self.focus_count();
        if focus_count == 0 {
            return;
        }

        self.current_index = (self.current_index + 1) % focus_count;
        self.sync_editing_for_current_focus();
    }

    fn next_question_or_submit(&mut self) -> bool {
        if self.is_confirm_tab() {
            true
        } else if self.current_index < self.questions.len() {
            self.current_index += 1;
            self.sync_editing_for_current_focus();
            false
        } else {
            true
        }
    }

    fn begin_custom_editing(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };

        if !question.options.is_empty() && !self.current_is_custom_row() {
            return;
        }

        let custom_cursor = self
            .current_answer()
            .map(|answer| char_count(&answer.custom_text))
            .unwrap_or(0);

        if let Some(answer) = self.current_answer_mut() {
            answer.custom_cursor = custom_cursor;
        }
        self.editing_custom = true;
    }

    fn finish_custom_editing(&mut self) -> bool {
        let Some(question) = self.current_question() else {
            return false;
        };
        let has_options = !question.options.is_empty();
        let multiple = question.multiple;

        let mut should_confirm = true;
        if let Some(answer) = self.current_answer_mut() {
            let has_text = !answer.custom_text.trim().is_empty();

            if has_text {
                answer.custom_selected = true;
                if !multiple {
                    answer.selected.fill(false);
                }
            } else if has_options {
                answer.custom_selected = false;
                should_confirm = false;
            } else {
                answer.custom_selected = true;
            }
        }

        if has_options {
            self.editing_custom = false;
        }

        should_confirm
    }

    fn toggle_current(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        if question.options.is_empty() {
            self.editing_custom = true;
            return;
        }

        let options_len = question.options.len();
        let multiple = question.multiple;
        if let Some(answer) = self.current_answer_mut() {
            if answer.cursor < options_len {
                if multiple {
                    if let Some(selected) = answer.selected.get_mut(answer.cursor) {
                        *selected = !*selected;
                    }
                } else {
                    answer.select_cursor();
                    answer.custom_selected = false;
                }
                self.editing_custom = false;
            } else {
                if multiple && !answer.custom_text.trim().is_empty() {
                    answer.custom_selected = !answer.custom_selected;
                }
            }
        }
    }

    fn insert_char(&mut self, ch: char) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            insert_char_at_cursor(&mut answer.custom_text, &mut answer.custom_cursor, ch);
        }
        self.sync_custom_selection_from_text();
    }

    fn delete_char(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            delete_char_before_cursor(&mut answer.custom_text, &mut answer.custom_cursor);
        }
        self.sync_custom_selection_from_text();
    }

    fn delete_word_backward(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            delete_word_before_cursor(&mut answer.custom_text, &mut answer.custom_cursor);
        }
        self.sync_custom_selection_from_text();
    }

    fn clear_custom_text(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            answer.custom_text.clear();
            answer.custom_cursor = 0;
            answer.custom_selected = false;
        }
    }

    fn sync_custom_selection_from_text(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        let text_only = question.options.is_empty();
        let multiple = question.multiple;
        let editing_custom_row = self.editing_custom && self.current_is_custom_row();

        if let Some(answer) = self.current_answer_mut() {
            let has_text = !answer.custom_text.trim().is_empty();
            if text_only || editing_custom_row {
                answer.custom_selected = has_text;
                if has_text && !multiple {
                    answer.selected.fill(false);
                }
            } else if !has_text {
                answer.custom_selected = false;
            }
        }
    }

    fn move_custom_cursor_left(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            answer.custom_cursor = answer.custom_cursor.saturating_sub(1);
        }
    }

    fn move_custom_cursor_right(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            answer.custom_cursor = (answer.custom_cursor + 1).min(char_count(&answer.custom_text));
        }
    }

    fn move_custom_cursor_word_left(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            move_word_left(&answer.custom_text, &mut answer.custom_cursor);
        }
    }

    fn move_custom_cursor_word_right(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            move_word_right(&answer.custom_text, &mut answer.custom_cursor);
        }
    }

    fn move_custom_cursor_start(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            answer.custom_cursor = 0;
        }
    }

    fn move_custom_cursor_end(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            answer.custom_cursor = char_count(&answer.custom_text);
        }
    }

    fn stop_editing_custom(&mut self) {
        if self
            .current_question()
            .map(|q| !q.options.is_empty())
            .unwrap_or(false)
        {
            self.editing_custom = false;
        }
    }

    fn response(&self) -> Value {
        Value::Array(
            self.questions
                .iter()
                .zip(self.answers.iter())
                .map(|(question, answer)| answer.to_value(question))
                .collect(),
        )
    }

    fn empty_response(&self) -> Value {
        Value::Array(
            self.questions
                .iter()
                .map(|_| Value::Array(Vec::new()))
                .collect(),
        )
    }
}

impl QuestionAnswerState {
    fn for_question(question: &QuestionItem) -> Self {
        let mut selected = vec![false; question.options.len()];
        if !question.multiple && !selected.is_empty() {
            selected[0] = true;
        }

        Self {
            selected,
            cursor: 0,
            custom_text: String::new(),
            custom_cursor: 0,
            custom_selected: question.options.is_empty(),
        }
    }

    fn select_cursor(&mut self) {
        if self.cursor < self.selected.len() {
            self.selected.fill(false);
            self.selected[self.cursor] = true;
            self.custom_selected = false;
        } else {
            self.selected.fill(false);
            self.custom_selected = true;
        }
    }

    fn to_value(&self, question: &QuestionItem) -> Value {
        let mut answers = Vec::new();
        for (idx, selected) in self.selected.iter().enumerate() {
            if *selected {
                if let Some(option) = question.options.get(idx) {
                    answers.push(Value::String(option.label.clone()));
                }
            }
        }

        let custom = self.custom_text.trim();
        if !custom.is_empty() && (self.custom_selected || question.options.is_empty()) {
            answers.push(Value::String(custom.to_string()));
        }

        Value::Array(answers)
    }
}

pub fn handle_question_dialog_key_event(
    state: &mut QuestionDialogState,
    event: KeyEvent,
) -> QuestionDialogAction {
    let Some(request) = state.active_mut() else {
        return QuestionDialogAction::NotHandled;
    };

    match event.code {
        KeyCode::Esc => {
            let editing_option_custom = request.editing_custom
                && request
                    .current_question()
                    .map(|q| !q.options.is_empty())
                    .unwrap_or(false);
            if editing_option_custom {
                request.stop_editing_custom();
                QuestionDialogAction::Handled
            } else {
                QuestionDialogAction::Cancel
            }
        }
        KeyCode::Left
            if request.current_is_text_entry()
                && (event.modifiers.contains(KeyModifiers::SUPER)
                    || event.modifiers.contains(KeyModifiers::META)) =>
        {
            request.move_custom_cursor_start();
            QuestionDialogAction::Handled
        }
        KeyCode::Right
            if request.current_is_text_entry()
                && (event.modifiers.contains(KeyModifiers::SUPER)
                    || event.modifiers.contains(KeyModifiers::META)) =>
        {
            request.move_custom_cursor_end();
            QuestionDialogAction::Handled
        }
        KeyCode::Left
            if request.current_is_text_entry() && event.modifiers.contains(KeyModifiers::ALT) =>
        {
            request.move_custom_cursor_word_left();
            QuestionDialogAction::Handled
        }
        KeyCode::Right
            if request.current_is_text_entry() && event.modifiers.contains(KeyModifiers::ALT) =>
        {
            request.move_custom_cursor_word_right();
            QuestionDialogAction::Handled
        }
        KeyCode::Left if request.current_is_text_entry() => {
            request.move_custom_cursor_left();
            QuestionDialogAction::Handled
        }
        KeyCode::Right if request.current_is_text_entry() => {
            request.move_custom_cursor_right();
            QuestionDialogAction::Handled
        }
        KeyCode::Left if !request.current_is_text_entry() && request.focus_count() > 1 => {
            request.previous_question();
            QuestionDialogAction::Handled
        }
        KeyCode::Right if !request.current_is_text_entry() && request.focus_count() > 1 => {
            request.next_question();
            QuestionDialogAction::Handled
        }
        KeyCode::Up if !request.current_is_text_entry() => {
            request.previous_option();
            QuestionDialogAction::Handled
        }
        KeyCode::Down if !request.current_is_text_entry() => {
            request.next_option();
            QuestionDialogAction::Handled
        }
        KeyCode::Char('k') if !request.current_is_text_entry() => {
            request.previous_option();
            QuestionDialogAction::Handled
        }
        KeyCode::Char('j') if !request.current_is_text_entry() => {
            request.next_option();
            QuestionDialogAction::Handled
        }
        KeyCode::BackTab if !request.current_is_text_entry() => {
            request.previous_question();
            QuestionDialogAction::Handled
        }
        KeyCode::Tab
            if !request.current_is_text_entry()
                && event.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            request.previous_question();
            QuestionDialogAction::Handled
        }
        KeyCode::Tab if !request.current_is_text_entry() => {
            request.next_question();
            QuestionDialogAction::Handled
        }
        KeyCode::PageUp if !request.current_is_text_entry() => {
            request.previous_question();
            QuestionDialogAction::Handled
        }
        KeyCode::PageDown if !request.current_is_text_entry() => {
            request.next_question();
            QuestionDialogAction::Handled
        }
        KeyCode::Char(' ') if !request.current_is_text_entry() => {
            request.toggle_current();
            QuestionDialogAction::Handled
        }
        KeyCode::Tab | KeyCode::BackTab if request.current_is_text_entry() => {
            QuestionDialogAction::Handled
        }
        KeyCode::Backspace
            if request.current_is_text_entry()
                && (event.modifiers.contains(KeyModifiers::SUPER)
                    || event.modifiers.contains(KeyModifiers::META)) =>
        {
            request.clear_custom_text();
            QuestionDialogAction::Handled
        }
        KeyCode::Backspace
            if request.current_is_text_entry() && event.modifiers.contains(KeyModifiers::ALT) =>
        {
            request.delete_word_backward();
            QuestionDialogAction::Handled
        }
        KeyCode::Char('u')
            if request.current_is_text_entry()
                && event.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            request.clear_custom_text();
            QuestionDialogAction::Handled
        }
        KeyCode::Backspace if request.current_is_text_entry() => {
            request.delete_char();
            QuestionDialogAction::Handled
        }
        KeyCode::Enter => {
            if request.current_is_text_entry() {
                if request.finish_custom_editing() && request.next_question_or_submit() {
                    QuestionDialogAction::Submit
                } else {
                    QuestionDialogAction::Handled
                }
            } else if request.is_confirm_tab() {
                QuestionDialogAction::Submit
            } else if request.current_is_custom_row() {
                request.begin_custom_editing();
                QuestionDialogAction::Handled
            } else if request.next_question_or_submit() {
                QuestionDialogAction::Submit
            } else {
                QuestionDialogAction::Handled
            }
        }
        KeyCode::Char(ch)
            if !event.modifiers.contains(KeyModifiers::CONTROL)
                && !event.modifiers.contains(KeyModifiers::ALT) =>
        {
            request.insert_char(ch);
            QuestionDialogAction::Handled
        }
        _ => QuestionDialogAction::NotHandled,
    }
}

pub fn handle_question_dialog_mouse_event(
    _state: &mut QuestionDialogState,
    _event: MouseEvent,
) -> bool {
    false
}

pub fn render_question_dialog(
    f: &mut Frame,
    state: &mut QuestionDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    let Some(request) = state.active() else {
        return;
    };

    let option_count = request
        .current_question()
        .map(option_row_count)
        .unwrap_or(request.questions.len()) as u16;
    let extra_body_lines = request
        .current_question()
        .map(|question| 1 + u16::from(question.multiple))
        .unwrap_or_else(|| u16::from(request.is_confirm_tab()));
    let desired_height = 8u16
        .saturating_add(option_count)
        .saturating_add(extra_body_lines)
        .min(18);
    let panel_height = area.height.min(desired_height.max(8));
    let dialog_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(panel_height),
        width: area.width,
        height: panel_height,
    };

    f.render_widget(Clear, dialog_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        dialog_area,
    );

    let border = Block::default()
        .style(Style::default().bg(colors.dialog_background))
        .borders(Borders::LEFT)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(colors.info))
        .padding(Padding::new(1, 1, 1, 1));
    let content_area = border.inner(dialog_area);
    f.render_widget(border, dialog_area);

    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content_area);

    let cancel_text = "esc cancel";
    let cancel_width = (cancel_text.len() as u16).min(chunks[0].width);
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(cancel_width)])
        .split(chunks[0]);

    f.render_widget(
        Paragraph::new(question_tabs_line(request, state.queued_count(), &colors)),
        header_chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            cancel_text,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    let body_lines = if request.is_confirm_tab() {
        confirm_body_lines(request, &colors)
    } else if let (Some(question), Some(answer)) =
        (request.current_question(), request.current_answer())
    {
        question_body_lines(
            question,
            answer,
            request.current_index,
            request.editing_custom,
            &colors,
        )
    } else {
        Vec::new()
    };
    f.render_widget(
        Paragraph::new(body_lines)
            .style(Style::default().bg(colors.dialog_background))
            .wrap(Wrap { trim: true }),
        chunks[1],
    );

    let footer = footer_line(request, &colors);
    f.render_widget(Paragraph::new(footer).alignment(Alignment::Left), chunks[2]);
}

fn parse_questions(value: Value) -> Vec<QuestionItem> {
    let values = match value {
        Value::Array(items) => items,
        Value::Object(_) => vec![value],
        Value::String(text) => vec![json!({ "question": text, "header": "Question" })],
        _ => Vec::new(),
    };

    let mut questions: Vec<QuestionItem> = values
        .into_iter()
        .filter_map(|value| parse_question(value).or_else(|| Some(default_question())))
        .collect();

    if questions.is_empty() {
        questions.push(default_question());
    }

    questions
}

fn parse_question(value: Value) -> Option<QuestionItem> {
    let obj = value.as_object()?;
    let question = string_field(obj, &["question", "text", "prompt"])
        .unwrap_or_else(|| "Question".to_string());
    let header = string_field(obj, &["header", "title"]).unwrap_or_else(|| "Question".to_string());
    let mut options: Vec<QuestionOption> = obj
        .get("options")
        .and_then(|v| v.as_array())
        .map(|options| options.iter().filter_map(parse_option).collect())
        .unwrap_or_else(Vec::new);
    options.retain(|option| !is_custom_answer_sentinel_label(&option.label));
    let multiple = multiple_field(obj).unwrap_or_else(|| question_mentions_multiple(&question));
    let custom = true;

    Some(QuestionItem {
        header,
        question,
        options,
        multiple,
        custom,
    })
}

fn parse_option(value: &Value) -> Option<QuestionOption> {
    if let Some(label) = value.as_str() {
        return Some(QuestionOption {
            label: label.to_string(),
            description: String::new(),
        });
    }

    let obj = value.as_object()?;
    let label = string_field(obj, &["label", "value", "text"])?;
    let description = string_field(obj, &["description", "detail"]).unwrap_or_default();
    Some(QuestionOption { label, description })
}

fn normalized_option_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_custom_answer_sentinel_label(label: &str) -> bool {
    matches!(
        normalized_option_label(label).as_str(),
        "type your own answer"
            | "type your own"
            | "enter your own answer"
            | "write your own answer"
            | "provide your own answer"
            | "custom answer"
            | "enter custom answer"
            | "write custom answer"
    )
}

fn default_question() -> QuestionItem {
    QuestionItem {
        header: "Question".to_string(),
        question: "The agent needs your input.".to_string(),
        options: Vec::new(),
        multiple: false,
        custom: true,
    }
}

fn string_field(obj: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| obj.get(*name).and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

fn bool_field(obj: &serde_json::Map<String, Value>, names: &[&str]) -> Option<bool> {
    names
        .iter()
        .find_map(|name| obj.get(*name).and_then(|v| v.as_bool()))
}

fn boolish_field(obj: &serde_json::Map<String, Value>, names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| {
        obj.get(*name).and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "multiple" | "multi" | "multiselect" | "multi_select"
                | "multiple_choice" | "checkbox" | "checkboxes" | "select_all" => Some(true),
                "false" | "no" | "single" | "radio" | "single_choice" => Some(false),
                _ => None,
            },
            Value::Number(value) => value.as_u64().map(|value| value > 1),
            _ => None,
        })
    })
}

fn multiple_field(obj: &serde_json::Map<String, Value>) -> Option<bool> {
    boolish_field(
        obj,
        &[
            "multiple",
            "allow_multiple",
            "allowMultiple",
            "multi",
            "multiselect",
            "multi_select",
            "multipleChoice",
            "multiple_choice",
            "checkbox",
            "checkboxes",
            "type",
            "kind",
            "mode",
            "selection",
            "selection_type",
            "selectionType",
            "max_selections",
            "maxSelections",
        ],
    )
}

fn question_mentions_multiple(question: &str) -> bool {
    let question = question.to_ascii_lowercase();
    [
        "select all that apply",
        "choose all that apply",
        "pick all that apply",
        "select multiple",
        "choose multiple",
        "pick multiple",
        "multiple answers",
        "multiple selections",
    ]
    .iter()
    .any(|phrase| question.contains(phrase))
}

fn option_row_count(question: &QuestionItem) -> usize {
    question.options.len() + usize::from(question.custom && !question.options.is_empty())
}

fn text_with_cursor(text: &str, cursor: usize) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    chars.insert(cursor, '_');
    chars.into_iter().collect()
}

fn stable_tab_label(label: &str) -> String {
    format!(" {} ", label.trim())
}

fn is_generic_question_label(text: &str) -> bool {
    let text = text.trim();
    text.is_empty() || text.eq_ignore_ascii_case("question")
}

fn question_display_text(question: &QuestionItem, idx: usize) -> String {
    if !is_generic_question_label(&question.question) {
        return question.question.trim().to_string();
    }

    if !is_generic_question_label(&question.header) {
        return question.header.trim().to_string();
    }

    format!("Question {}", idx + 1)
}

fn question_tabs_line<'a>(
    request: &QuestionDialogRequest,
    queued_count: usize,
    colors: &ThemeColors,
) -> Line<'a> {
    let mut spans = Vec::new();

    for idx in 0..request.questions.len() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }

        let active = idx == request.current_index;
        let label = stable_tab_label(&format!("Question {}", idx + 1));

        if active {
            spans.push(Span::styled(
                label,
                Style::default()
                    .bg(colors.warning)
                    .fg(contrast_text(colors.warning))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
        }
    }

    if !request.questions.is_empty() {
        spans.push(Span::raw("  "));
    }

    let confirm_label = stable_tab_label("Confirm");
    if request.is_confirm_tab() {
        spans.push(Span::styled(
            confirm_label,
            Style::default()
                .bg(colors.warning)
                .fg(contrast_text(colors.warning))
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            confirm_label,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
    }

    if queued_count > 0 {
        spans.push(Span::styled(
            format!("  +{} queued", queued_count),
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
    }

    Line::from(spans)
}

fn question_body_lines<'a>(
    question: &QuestionItem,
    answer: &QuestionAnswerState,
    question_index: usize,
    editing_custom: bool,
    colors: &ThemeColors,
) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "Question: ",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            question_display_text(question, question_index),
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    if question.multiple {
        lines.push(Line::from(vec![Span::styled(
            "Select all that apply.",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]));
    }
    lines.push(Line::from(""));

    if question.options.is_empty() {
        let text = if editing_custom {
            text_with_cursor(&answer.custom_text, answer.custom_cursor)
        } else {
            answer.custom_text.clone()
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(colors.info)),
            Span::styled(text, Style::default().fg(colors.text)),
        ]));
        return lines;
    }

    for (idx, option) in question.options.iter().enumerate() {
        lines.push(option_line(
            option,
            answer.cursor == idx,
            answer.selected.get(idx).copied().unwrap_or(false),
            question.multiple,
            colors,
        ));
    }

    if question.custom {
        let idx = question.options.len();
        let mut label = "Type your own answer".to_string();
        if !answer.custom_text.is_empty() {
            label.push_str(": ");
            if editing_custom {
                label.push_str(&text_with_cursor(&answer.custom_text, answer.custom_cursor));
            } else {
                label.push_str(&answer.custom_text);
            }
        } else if editing_custom {
            label.push_str(": _");
        }

        let option = QuestionOption {
            label,
            description: String::new(),
        };
        lines.push(option_line(
            &option,
            answer.cursor == idx,
            answer.custom_selected,
            question.multiple,
            colors,
        ));
    }

    lines
}

fn answer_summary(question: &QuestionItem, answer: &QuestionAnswerState) -> String {
    let values = answer.to_value(question);
    let labels: Vec<String> = values
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    if labels.is_empty() {
        "No answer".to_string()
    } else {
        labels.join(", ")
    }
}

fn confirm_body_lines<'a>(request: &QuestionDialogRequest, colors: &ThemeColors) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "Confirm answers",
        Style::default()
            .fg(colors.text)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    for (idx, (question, answer)) in request
        .questions
        .iter()
        .zip(request.answers.iter())
        .enumerate()
    {
        let label = question_display_text(question, idx);
        let summary = answer_summary(question, answer);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}. ", idx + 1),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(label, Style::default().fg(colors.text)),
            Span::styled(
                " - ",
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(summary, Style::default().fg(colors.text_weak)),
        ]));
    }

    lines
}

fn option_line<'a>(
    option: &QuestionOption,
    cursor: bool,
    selected: bool,
    multiple: bool,
    colors: &ThemeColors,
) -> Line<'a> {
    let check = if multiple {
        if selected {
            "[x] "
        } else {
            "[ ] "
        }
    } else if selected {
        "(*) "
    } else {
        "( ) "
    };

    let selected_style = Style::default()
        .bg(colors.info)
        .fg(contrast_text(colors.info))
        .add_modifier(Modifier::BOLD);
    let label_style = if cursor {
        selected_style
    } else {
        Style::default().fg(colors.text)
    };
    let weak_style = if cursor {
        selected_style
    } else {
        Style::default()
            .fg(colors.text_weak)
            .add_modifier(Modifier::DIM)
    };

    let mut spans = vec![
        Span::styled(check, weak_style),
        Span::styled(option.label.clone(), label_style),
    ];
    if !option.description.is_empty() {
        spans.push(Span::styled(" - ", weak_style));
        spans.push(Span::styled(option.description.clone(), weak_style));
    }

    Line::from(spans)
}

fn footer_line<'a>(request: &QuestionDialogRequest, colors: &ThemeColors) -> Line<'a> {
    let key_style = Style::default().fg(colors.info);

    if request.current_is_text_entry() {
        let esc_label = if request
            .current_question()
            .map(|question| question.options.is_empty())
            .unwrap_or(false)
        {
            " dismiss"
        } else {
            " cancel edit"
        };
        return Line::from(vec![
            Span::styled("enter", key_style),
            Span::raw(" confirm  "),
            Span::styled("esc", key_style),
            Span::raw(esc_label),
        ]);
    }

    let mut spans = Vec::new();
    if request.focus_count() > 1 {
        spans.push(Span::styled("⇆", key_style));
        spans.push(Span::raw(" cycle tabs  "));
    }

    if request.is_confirm_tab() {
        spans.push(Span::styled("enter", key_style));
        spans.push(Span::raw(" submit  "));
        spans.push(Span::styled("esc", key_style));
        spans.push(Span::raw(" dismiss"));
        return Line::from(spans);
    }

    let Some(question) = request.current_question() else {
        return Line::from(spans);
    };
    let Some(answer) = request.current_answer() else {
        return Line::from(spans);
    };

    spans.push(Span::styled("↑↓", key_style));
    spans.push(Span::raw(" select  "));

    if question.multiple && answer.cursor < question.options.len() {
        spans.push(Span::styled("space", key_style));
        spans.push(Span::raw(" toggle  "));
    }

    spans.push(Span::styled("enter", key_style));
    if question.custom && !question.options.is_empty() && answer.cursor == question.options.len() {
        spans.push(Span::raw(" edit  "));
    } else {
        spans.push(Span::raw(" confirm  "));
    }

    spans.push(Span::styled("esc", key_style));
    spans.push(Span::raw(" dismiss"));

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn response_returns_selected_option_labels() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        assert_eq!(request.response(), json!([["A"]]));
    }

    #[test]
    fn response_accepts_custom_text() {
        let (tx, _rx) = oneshot::channel();
        let mut request =
            QuestionDialogRequest::new(json!([{ "question": "Explain", "header": "Details" }]), tx);
        request.insert_char('h');
        request.insert_char('i');

        assert_eq!(request.response(), json!([["hi"]]));
    }

    #[test]
    fn option_custom_answer_requires_enter_before_typing() {
        let (tx, _rx) = oneshot::channel();
        let mut request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "custom": true,
                "options": [{ "label": "A" }]
            }]),
            tx,
        );

        request.next_option();
        request.insert_char('z');

        assert_eq!(request.response(), json!([["A"]]));
        assert_eq!(request.answers[0].custom_text, "");

        request.begin_custom_editing();
        request.insert_char('z');
        request.finish_custom_editing();

        assert_eq!(request.response(), json!([["z"]]));
    }

    #[test]
    fn duplicate_custom_answer_option_is_removed() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "options": [
                    { "label": "A" },
                    { "label": "Type your own answer" },
                    { "label": "B" }
                ]
            }]),
            tx,
        );

        assert_eq!(request.questions[0].options.len(), 2);
        assert_eq!(request.questions[0].options[0].label, "A");
        assert_eq!(request.questions[0].options[1].label, "B");
        assert_eq!(option_row_count(&request.questions[0]), 3);
    }

    #[test]
    fn tab_cycles_between_questions_without_submitting() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([
                {
                    "question": "Pick one",
                    "header": "One",
                    "options": [{ "label": "A" }]
                },
                {
                    "question": "Pick two",
                    "header": "Two",
                    "options": [{ "label": "B" }]
                }
            ]),
            tx,
        );

        handle_question_dialog_key_event(&mut state, key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.current.as_ref().unwrap().current_index, 1);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.current.as_ref().unwrap().current_index, 2);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.current.as_ref().unwrap().current_index, 0);

        handle_question_dialog_key_event(&mut state, key(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(state.current.as_ref().unwrap().current_index, 2);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(state.current.as_ref().unwrap().current_index, 1);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(state.current.as_ref().unwrap().current_index, 2);
    }

    #[test]
    fn enter_moves_to_confirm_then_submit() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "options": [{ "label": "A" }]
            }]),
            tx,
        );

        let action =
            handle_question_dialog_key_event(&mut state, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, QuestionDialogAction::Handled));
        assert_eq!(state.current.as_ref().unwrap().current_index, 1);

        let action =
            handle_question_dialog_key_event(&mut state, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, QuestionDialogAction::Submit));
    }

    #[test]
    fn tab_labels_use_question_numbers() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([
                {
                    "question": "This is a very long generated question that should not become a giant tab",
                    "header": "Question",
                    "options": [{ "label": "A" }]
                },
                {
                    "question": "Short",
                    "header": "Short",
                    "options": [{ "label": "B" }]
                }
            ]),
            tx,
        );
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let line = question_tabs_line(&request, 0, &colors);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert_eq!(line.spans[0].content.as_ref(), " Question 1 ");
        assert_eq!(line.spans[2].content.as_ref(), " Question 2 ");
        assert!(text.contains("Confirm"));
        assert!(!text.contains("generated question"));
        assert!(!text.contains("Short"));
    }

    #[test]
    fn question_body_shows_full_prompt_under_tabs() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "This is a very long generated question that should not become a giant tab",
                "header": "Question",
                "options": [{ "label": "A" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let body = question_body_lines(
            &request.questions[0],
            &request.answers[0],
            0,
            request.editing_custom,
            &colors,
        );
        let first_line = body[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let question_line = body[1]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(first_line, "");
        assert_eq!(
            question_line,
            "Question: This is a very long generated question that should not become a giant tab"
        );
    }

    #[test]
    fn generic_question_prompt_falls_back_to_numbered_label() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([
                {
                    "question": "Question",
                    "header": "Question",
                    "options": [{ "label": "A" }]
                },
                {
                    "question": "Question",
                    "header": "Question",
                    "options": [{ "label": "B" }]
                }
            ]),
            tx,
        );
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let body = question_body_lines(
            &request.questions[1],
            &request.answers[1],
            1,
            request.editing_custom,
            &colors,
        );
        let question_line = body[1]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let confirm = confirm_body_lines(&request, &colors);
        let confirm_text = confirm
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(question_line, "Question: Question 2");
        assert!(confirm_text.contains("1. Question 1"));
        assert!(confirm_text.contains("2. Question 2"));
        assert!(!confirm_text.contains("1. Question -"));
    }

    #[test]
    fn confirm_body_does_not_truncate_questions_or_answers() {
        let (tx, _rx) = oneshot::channel();
        let mut request = QuestionDialogRequest::new(
            json!([{
                "question": "This is a very long generated question that should not be truncated in confirm",
                "header": "Question"
            }]),
            tx,
        );
        for ch in "this is a long custom answer that should not be truncated".chars() {
            request.insert_char(ch);
        }
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let body = confirm_body_lines(&request, &colors);
        let text = body
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains(
            "This is a very long generated question that should not be truncated in confirm"
        ));
        assert!(text.contains("this is a long custom answer that should not be truncated"));
    }

    #[test]
    fn tab_labels_do_not_pad_to_fixed_width() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "One",
                "options": [{ "label": "A" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let line = question_tabs_line(&request, 0, &colors);

        assert_eq!(line.spans[0].content.as_ref(), " Question 1 ");
        assert_eq!(line.spans[2].content.as_ref(), " Confirm ");
    }

    #[test]
    fn footer_uses_simple_cycle_tabs_label() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([
                {
                    "question": "Pick one",
                    "header": "One",
                    "options": [{ "label": "A" }]
                },
                {
                    "question": "Pick two",
                    "header": "Two",
                    "options": [{ "label": "B" }]
                }
            ]),
            tx,
        );
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let line = footer_line(&request, &colors);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(text.contains("cycle tabs"));
        assert!(!text.contains("tab/shift-tab"));
        assert!(!text.contains("←/→"));
    }

    #[test]
    fn multiple_aliases_render_checkbox_question() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick all project areas",
                "header": "Areas",
                "type": "multiple_choice",
                "options": [{ "label": "CLI" }, { "label": "TUI" }]
            }]),
            tx,
        );

        assert!(request.questions[0].multiple);

        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let footer = footer_line(&request, &colors);
        let footer_text: String = footer
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(footer_text.contains("space"));
        assert!(footer_text.contains("toggle"));

        let body = question_body_lines(
            &request.questions[0],
            &request.answers[0],
            0,
            request.editing_custom,
            &colors,
        );
        let body_text = body
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(body_text.contains("Select all that apply."));
        assert!(body_text.contains("[ ] "));
    }

    #[test]
    fn multiple_can_be_inferred_from_question_text() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Select all that apply",
                "header": "Choices",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        assert!(request.questions[0].multiple);
    }

    #[test]
    fn multiple_choice_toggles_with_space() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "multiple": true,
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        handle_question_dialog_key_event(&mut state, key(KeyCode::Char(' '), KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Down, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Char(' '), KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.response(), json!([["A", "B"]]));

        handle_question_dialog_key_event(&mut state, key(KeyCode::Char(' '), KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.response(), json!([["A"]]));
    }

    #[test]
    fn multiple_choice_auto_checks_typed_custom_answer() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Select all that apply",
                "header": "Choice",
                "multiple": true,
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        handle_question_dialog_key_event(&mut state, key(KeyCode::Down, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Down, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Enter, KeyModifiers::NONE));
        state.insert_text("custom");
        handle_question_dialog_key_event(&mut state, key(KeyCode::Esc, KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.response(), json!([["custom"]]));
        assert!(request.answers[0].custom_selected);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Up, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Char(' '), KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.response(), json!([["B", "custom"]]));
        assert!(request.answers[0].custom_selected);
    }

    #[test]
    fn custom_text_supports_cursor_insertion() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(json!([{ "question": "Explain", "header": "Details" }]), tx);

        for ch in ['a', 'b', 'c'] {
            handle_question_dialog_key_event(
                &mut state,
                key(KeyCode::Char(ch), KeyModifiers::NONE),
            );
        }
        handle_question_dialog_key_event(&mut state, key(KeyCode::Left, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Left, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Char('X'), KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_text, "aXbc");
        assert_eq!(request.answers[0].custom_cursor, 2);
    }

    #[test]
    fn custom_text_supports_option_arrow_word_motion() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(json!([{ "question": "Explain", "header": "Details" }]), tx);
        state.insert_text("hello brave world");

        handle_question_dialog_key_event(&mut state, key(KeyCode::Left, KeyModifiers::ALT));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 12);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Backspace, KeyModifiers::ALT));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_text, "hello world");
        assert_eq!(request.answers[0].custom_cursor, 6);
    }

    #[test]
    fn custom_text_supports_command_backspace_clear() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(json!([{ "question": "Explain", "header": "Details" }]), tx);
        state.insert_text("hello world");

        handle_question_dialog_key_event(&mut state, key(KeyCode::Backspace, KeyModifiers::SUPER));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_text, "");
        assert_eq!(request.answers[0].custom_cursor, 0);
    }

    #[test]
    fn option_questions_always_include_custom_row() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "custom": false,
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        assert!(request.questions[0].custom);
        assert_eq!(option_row_count(&request.questions[0]), 3);
    }

    #[test]
    fn option_line_has_no_cursor_marker() {
        let option = QuestionOption {
            label: "A".to_string(),
            description: String::new(),
        };
        let colors = crate::theme::Theme::load_from_file("src/theme.json")
            .unwrap()
            .get_colors(true);
        let line = option_line(&option, true, true, false, &colors);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(text.starts_with("(*) "));
    }
}
