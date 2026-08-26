use crate::theme::{contrast_text, ThemeColors};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
    Frame,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use tokio::sync::oneshot;
use unicode_width::UnicodeWidthStr;

const QUESTION_DIALOG_FOOTER_GAP_HEIGHT: u16 = 1;
const QUESTION_HEADER_CANCEL_GAP_WIDTH: u16 = 1;
const QUESTION_DIALOG_MIN_HEIGHT: u16 = 8 + QUESTION_DIALOG_FOOTER_GAP_HEIGHT;
const QUESTION_DIALOG_CHROME_HEIGHT: u16 = 4 + QUESTION_DIALOG_FOOTER_GAP_HEIGHT;

#[derive(Clone, Debug)]
struct QuestionOption {
    label: String,
    description: String,
}

#[derive(Clone, Copy, Debug)]
struct QuestionMouseHitbox {
    area: Rect,
    target: QuestionMouseTarget,
}

#[derive(Clone, Copy, Debug)]
enum QuestionMouseTarget {
    Option(usize),
    Confirm,
    Cancel,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionDialogSnapshot {
    pub questions: Vec<QuestionSnapshot>,
    pub queued_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionSnapshot {
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOptionSnapshot>,
    pub multiple: bool,
    pub custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionOptionSnapshot {
    pub label: String,
    pub description: String,
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

fn delete_to_start_before_cursor(text: &mut String, cursor: &mut usize) {
    let len = char_count(text);
    *cursor = (*cursor).min(len);
    if *cursor == 0 {
        return;
    }

    let end = char_to_byte(text, *cursor);
    text.replace_range(0..end, "");
    *cursor = 0;
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

fn has_command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META)
}

fn has_option_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::ALT)
}

fn is_custom_start_key(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::Home)
        || matches!(event.code, KeyCode::Char('a') if event.modifiers.contains(KeyModifiers::CONTROL))
        || matches!(event.code, KeyCode::Left if has_command_modifier(event.modifiers))
}

fn is_custom_end_key(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::End)
        || matches!(event.code, KeyCode::Char('e') if event.modifiers.contains(KeyModifiers::CONTROL))
        || matches!(event.code, KeyCode::Right if has_command_modifier(event.modifiers))
}

fn is_custom_word_left_key(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::Left if has_option_modifier(event.modifiers))
        || matches!(event.code, KeyCode::Char('b') if has_option_modifier(event.modifiers))
}

fn is_custom_word_right_key(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::Right if has_option_modifier(event.modifiers))
        || matches!(event.code, KeyCode::Char('f') if has_option_modifier(event.modifiers))
}

fn is_custom_line_delete_key(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::Backspace if has_command_modifier(event.modifiers))
        || matches!(event.code, KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL))
}

fn is_custom_word_delete_key(event: KeyEvent) -> bool {
    matches!(event.code, KeyCode::Backspace if has_option_modifier(event.modifiers))
        || matches!(event.code, KeyCode::Char('w') if event.modifiers.contains(KeyModifiers::CONTROL))
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
    tab_hitboxes: Vec<QuestionTabHitbox>,
    mouse_hitboxes: Vec<QuestionMouseHitbox>,
    /// Vertical scroll for the question body when options wrap past the panel.
    body_scroll_y: u16,
    /// Last rendered panel height (for chat bottom scroll padding).
    last_panel_height: u16,
}

#[derive(Clone, Copy, Debug)]
struct QuestionTabHitbox {
    area: Rect,
    index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            tab_hitboxes: Vec::new(),
            mouse_hitboxes: Vec::new(),
            body_scroll_y: 0,
            last_panel_height: 0,
        }
    }

    pub fn last_panel_height(&self) -> u16 {
        self.last_panel_height
    }

    pub fn enqueue(&mut self, questions: Value, response_tx: oneshot::Sender<Value>) {
        let request = QuestionDialogRequest::new(questions, response_tx);
        if self.current.is_none() {
            self.current = Some(request);
            self.tab_hitboxes.clear();
            self.mouse_hitboxes.clear();
            self.body_scroll_y = 0;
        } else {
            self.queue.push_back(request);
        }
    }

    /// Bottom padding for chat scroll so last lines stay above the dialog.
    ///
    /// `below_chat_height` is the terminal rows under the chat viewport (input,
    /// help, status, etc.). Only the portion of the dialog that actually
    /// overlaps chat is used as padding.
    pub fn chat_scroll_bottom_padding(&self, below_chat_height: u16) -> u16 {
        if self.current.is_none() {
            return 0;
        }
        let panel_height = if self.last_panel_height == 0 {
            QUESTION_DIALOG_MIN_HEIGHT
        } else {
            self.last_panel_height
        };
        panel_height.saturating_sub(below_chat_height)
    }

    pub fn has_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn current_snapshot(&self) -> Option<QuestionDialogSnapshot> {
        let request = self.current.as_ref()?;
        Some(QuestionDialogSnapshot {
            questions: request
                .questions
                .iter()
                .map(|question| QuestionSnapshot {
                    header: question.header.clone(),
                    question: question.question.clone(),
                    options: question
                        .options
                        .iter()
                        .map(|option| QuestionOptionSnapshot {
                            label: option.label.clone(),
                            description: option.description.clone(),
                        })
                        .collect(),
                    multiple: question.multiple,
                    custom: question.custom,
                })
                .collect(),
            queued_count: self.queue.len(),
        })
    }

    pub fn submit_current(&mut self) {
        if let Some(request) = self.current.take() {
            let response = request.response();
            let _ = request.response_tx.send(response);
        }
        self.current = self.queue.pop_front();
        self.tab_hitboxes.clear();
        self.mouse_hitboxes.clear();
        self.body_scroll_y = 0;
    }

    pub fn respond_current(&mut self, response: Value) {
        if let Some(request) = self.current.take() {
            let _ = request.response_tx.send(response);
        }
        self.current = self.queue.pop_front();
        self.tab_hitboxes.clear();
        self.mouse_hitboxes.clear();
        self.body_scroll_y = 0;
    }

    pub fn cancel_current(&mut self) {
        if let Some(request) = self.current.take() {
            let response = request.empty_response();
            let _ = request.response_tx.send(response);
        }
        self.current = self.queue.pop_front();
        self.tab_hitboxes.clear();
        self.mouse_hitboxes.clear();
        self.body_scroll_y = 0;
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
        self.tab_hitboxes.clear();
        self.mouse_hitboxes.clear();
        self.body_scroll_y = 0;
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

    fn tab_index_at(&self, point: Position) -> Option<usize> {
        self.tab_hitboxes
            .iter()
            .find(|hitbox| hitbox.area.contains(point))
            .map(|hitbox| hitbox.index)
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

        if let Some(answer) = self.current_answer_mut() {
            answer.cursor = if answer.cursor == 0 {
                count - 1
            } else {
                answer.cursor - 1
            };
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

        if let Some(answer) = self.current_answer_mut() {
            answer.cursor = (answer.cursor + 1) % count;
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

    fn confirm_current_selection(&mut self) {
        let Some(question) = self.current_question() else {
            return;
        };
        if question.options.is_empty() || question.multiple {
            return;
        }

        let options_len = question.options.len();
        let mut selected = false;
        if let Some(answer) = self.current_answer_mut() {
            if answer.cursor < options_len {
                answer.select_cursor();
                selected = true;
            }
        }
        if selected {
            self.editing_custom = false;
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

    fn delete_custom_text_to_start(&mut self) {
        if !self.current_is_text_entry() {
            return;
        }

        if let Some(answer) = self.current_answer_mut() {
            delete_to_start_before_cursor(&mut answer.custom_text, &mut answer.custom_cursor);
        }
        self.sync_custom_selection_from_text();
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
        Self {
            selected: vec![false; question.options.len()],
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
        _ if request.current_is_text_entry() && is_custom_start_key(event) => {
            request.move_custom_cursor_start();
            QuestionDialogAction::Handled
        }
        _ if request.current_is_text_entry() && is_custom_end_key(event) => {
            request.move_custom_cursor_end();
            QuestionDialogAction::Handled
        }
        _ if request.current_is_text_entry() && is_custom_word_left_key(event) => {
            request.move_custom_cursor_word_left();
            QuestionDialogAction::Handled
        }
        _ if request.current_is_text_entry() && is_custom_word_right_key(event) => {
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
        _ if request.current_is_text_entry() && is_custom_line_delete_key(event) => {
            request.delete_custom_text_to_start();
            QuestionDialogAction::Handled
        }
        _ if request.current_is_text_entry() && is_custom_word_delete_key(event) => {
            request.delete_word_backward();
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
            } else {
                request.confirm_current_selection();
                if request.next_question_or_submit() {
                    QuestionDialogAction::Submit
                } else {
                    QuestionDialogAction::Handled
                }
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
    state: &mut QuestionDialogState,
    event: MouseEvent,
) -> QuestionDialogAction {
    if !matches!(
        event.kind,
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Moved
    ) {
        return QuestionDialogAction::NotHandled;
    }

    let point = Position::new(event.column, event.row);
    if let Some(tab_index) = state.tab_index_at(point) {
        if matches!(event.kind, MouseEventKind::Moved) {
            return QuestionDialogAction::NotHandled;
        }

        let Some(request) = state.active_mut() else {
            return QuestionDialogAction::NotHandled;
        };

        request.current_index = tab_index.min(request.questions.len());
        request.sync_editing_for_current_focus();
        return QuestionDialogAction::Handled;
    }

    let target = state
        .mouse_hitboxes
        .iter()
        .find(|hitbox| hitbox.area.contains(point))
        .map(|hitbox| hitbox.target);
    let Some(target) = target else {
        return QuestionDialogAction::NotHandled;
    };

    let Some(request) = state.active_mut() else {
        return QuestionDialogAction::NotHandled;
    };

    if matches!(event.kind, MouseEventKind::Moved) {
        if let QuestionMouseTarget::Option(index) = target {
            if let Some(answer) = request.current_answer_mut() {
                answer.cursor = index;
            }
            request.editing_custom = false;
            return QuestionDialogAction::Handled;
        }
        return QuestionDialogAction::NotHandled;
    }

    match target {
        QuestionMouseTarget::Option(index) => {
            if let Some(answer) = request.current_answer_mut() {
                answer.cursor = index;
            }
            if request.current_is_custom_row() {
                request.begin_custom_editing();
            } else {
                let is_multiple = request
                    .current_question()
                    .map(|question| question.multiple)
                    .unwrap_or(false);
                request.toggle_current();
                if !is_multiple && request.next_question_or_submit() {
                    return QuestionDialogAction::Submit;
                }
            }
            QuestionDialogAction::Handled
        }
        QuestionMouseTarget::Confirm if request.is_confirm_tab() => QuestionDialogAction::Submit,
        QuestionMouseTarget::Confirm => {
            if request.current_is_text_entry() {
                if request.finish_custom_editing() && request.next_question_or_submit() {
                    QuestionDialogAction::Submit
                } else {
                    QuestionDialogAction::Handled
                }
            } else {
                request.confirm_current_selection();
                if request.next_question_or_submit() {
                    QuestionDialogAction::Submit
                } else {
                    QuestionDialogAction::Handled
                }
            }
        }
        QuestionMouseTarget::Cancel => QuestionDialogAction::Cancel,
    }
}

fn line_wrapped_height(line: &Line<'_>, width: u16) -> u16 {
    wrapped_lines_height(std::slice::from_ref(line), width)
}

fn question_body_hitboxes(
    request: &QuestionDialogRequest,
    body_lines: &[Line<'_>],
    body_area: Rect,
    body_scroll_y: u16,
) -> Vec<QuestionMouseHitbox> {
    let Some(question) = request.current_question() else {
        return Vec::new();
    };
    if question.options.is_empty() {
        return Vec::new();
    }

    let option_start = 3 + usize::from(question.multiple);
    let mut y = body_area.y;
    let mut consumed = 0u16;
    let mut hitboxes = Vec::new();
    for (line_index, line) in body_lines.iter().enumerate() {
        let height = line_wrapped_height(line, body_area.width).max(1);
        let line_start = consumed;
        let line_end = consumed.saturating_add(height);
        consumed = line_end;

        if line_end <= body_scroll_y {
            continue;
        }

        let hidden = body_scroll_y.saturating_sub(line_start);
        let visible_in_line = height.saturating_sub(hidden);
        if line_index >= option_start && line_index < option_start + option_row_count(question) {
            let visible_height = visible_in_line.min(body_area.bottom().saturating_sub(y));
            if visible_height > 0 {
                hitboxes.push(QuestionMouseHitbox {
                    area: Rect::new(body_area.x, y, body_area.width, visible_height),
                    target: QuestionMouseTarget::Option(line_index - option_start),
                });
            }
        }
        y = y.saturating_add(visible_in_line);
        if y >= body_area.bottom() {
            break;
        }
    }
    hitboxes
}

/// Keep the selected/custom row visible when wrapped body height exceeds the panel.
fn ensure_body_scroll_visible(
    body_lines: &[Line<'_>],
    body_width: u16,
    body_height: u16,
    focus_line_index: usize,
    scroll_y: &mut u16,
) {
    if body_height == 0 || body_lines.is_empty() {
        *scroll_y = 0;
        return;
    }

    let mut offsets = Vec::with_capacity(body_lines.len() + 1);
    offsets.push(0u16);
    let mut total = 0u16;
    for line in body_lines {
        total = total.saturating_add(line_wrapped_height(line, body_width).max(1));
        offsets.push(total);
    }

    let max_scroll = total.saturating_sub(body_height);
    if *scroll_y > max_scroll {
        *scroll_y = max_scroll;
    }

    let focus = focus_line_index.min(body_lines.len().saturating_sub(1));
    let focus_start = offsets[focus];
    let focus_end = offsets[focus + 1];
    if focus_start < *scroll_y {
        *scroll_y = focus_start;
    } else if focus_end > (*scroll_y).saturating_add(body_height) {
        *scroll_y = focus_end.saturating_sub(body_height);
    }
    if *scroll_y > max_scroll {
        *scroll_y = max_scroll;
    }
}

fn focused_body_line_index(request: &QuestionDialogRequest, body_lines: &[Line<'_>]) -> usize {
    let Some(question) = request.current_question() else {
        return 0;
    };
    if question.options.is_empty() {
        // Free-text body: question text is the interactive content.
        return 1.min(body_lines.len().saturating_sub(1));
    }

    let option_start = 3 + usize::from(question.multiple);
    if request.current_is_custom_row() {
        return body_lines.len().saturating_sub(1);
    }

    let cursor = request
        .current_answer()
        .map(|answer| answer.cursor)
        .unwrap_or(0)
        .min(question.options.len().saturating_sub(1));
    (option_start + cursor).min(body_lines.len().saturating_sub(1))
}

fn push_tab_hitbox(
    hitboxes: &mut Vec<QuestionTabHitbox>,
    header_area: Rect,
    virtual_x: u16,
    scroll_x: u16,
    label: &str,
    index: usize,
) {
    let label_width = UnicodeWidthStr::width(label) as u16;
    if label_width == 0 || header_area.width == 0 {
        return;
    }

    let label_start = virtual_x;
    let label_end = label_start.saturating_add(label_width);
    let viewport_start = scroll_x;
    let viewport_end = scroll_x.saturating_add(header_area.width);

    if label_end > viewport_start && label_start < viewport_end {
        let visible_start = label_start.max(viewport_start);
        let visible_end = label_end.min(viewport_end);
        let screen_start = header_area
            .x
            .saturating_add(visible_start.saturating_sub(scroll_x));
        let screen_end = header_area
            .x
            .saturating_add(visible_end.saturating_sub(scroll_x));
        if screen_end > screen_start {
            hitboxes.push(QuestionTabHitbox {
                area: Rect {
                    x: screen_start,
                    y: header_area.y,
                    width: screen_end - screen_start,
                    height: 1,
                },
                index,
            });
        }
    }
}

fn tab_virtual_items(request: &QuestionDialogRequest) -> Vec<(usize, String, u16)> {
    let mut tabs = Vec::with_capacity(request.questions.len() + 1);
    let mut x = 0u16;

    for idx in 0..request.questions.len() {
        if idx > 0 {
            x = x.saturating_add(2);
        }

        let label = stable_tab_label(&format!("Question {}", idx + 1));
        tabs.push((idx, label.clone(), x));
        x = x.saturating_add(UnicodeWidthStr::width(label.as_str()) as u16);
    }

    if !request.questions.is_empty() {
        x = x.saturating_add(2);
    }

    let confirm_label = stable_tab_label("Confirm");
    tabs.push((request.questions.len(), confirm_label.clone(), x));

    tabs
}

fn tab_content_width(tabs: &[(usize, String, u16)]) -> u16 {
    tabs.last()
        .map(|(_, label, x)| x.saturating_add(UnicodeWidthStr::width(label.as_str()) as u16))
        .unwrap_or(0)
}

fn active_tab_scroll(request: &QuestionDialogRequest, viewport_width: u16) -> u16 {
    if viewport_width == 0 {
        return 0;
    }

    let tabs = tab_virtual_items(request);
    let content_width = tab_content_width(&tabs);
    if content_width <= viewport_width {
        return 0;
    }

    let Some((_, active_label, active_start)) = tabs
        .iter()
        .find(|(idx, _, _)| *idx == request.current_index.min(request.questions.len()))
    else {
        return 0;
    };

    let active_width = UnicodeWidthStr::width(active_label.as_str()) as u16;
    let active_end = active_start.saturating_add(active_width);
    let max_scroll = content_width.saturating_sub(viewport_width);

    if active_width >= viewport_width {
        (*active_start).min(max_scroll)
    } else if active_end > viewport_width {
        active_end.saturating_sub(viewport_width).min(max_scroll)
    } else {
        0
    }
}

fn question_tab_hitboxes(
    request: &QuestionDialogRequest,
    header_area: Rect,
    scroll_x: u16,
) -> Vec<QuestionTabHitbox> {
    let mut hitboxes = Vec::new();
    let header_start = header_area.x;
    let header_end = header_area.x.saturating_add(header_area.width);

    if header_start >= header_end {
        return hitboxes;
    }

    for (idx, label, virtual_x) in tab_virtual_items(request) {
        push_tab_hitbox(&mut hitboxes, header_area, virtual_x, scroll_x, &label, idx);
    }

    hitboxes
}

fn dialog_body_width(area_width: u16) -> u16 {
    // The dialog has a left border plus left/right padding before the body.
    area_width.saturating_sub(3).max(1)
}

fn wrapped_lines_height(lines: &[Line<'_>], width: u16) -> u16 {
    let width = usize::from(width.max(1));

    lines
        .iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            if text.is_empty() {
                1
            } else {
                text.lines()
                    .map(|part| textwrap::wrap(part, width).len().max(1) as u16)
                    .sum()
            }
        })
        .sum()
}

pub fn render_question_dialog(
    f: &mut Frame,
    state: &mut QuestionDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    let (body_lines, tabs_line, footer, tab_scroll_x_base) = {
        let Some(request) = state.active() else {
            state.tab_hitboxes.clear();
            state.mouse_hitboxes.clear();
            state.body_scroll_y = 0;
            state.last_panel_height = 0;
            return;
        };

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

        let tabs_line = question_tabs_line(request, state.queued_count(), &colors);
        let footer = footer_line(request, &colors);
        // tab_scroll_x needs viewport width; computed later after layout.
        (body_lines, tabs_line, footer, request)
    };

    // Re-borrow for hitbox/tab scroll calculations after layout; avoid holding borrow
    // across state mutations by computing panel geometry first.
    let desired_body_height = wrapped_lines_height(&body_lines, dialog_body_width(area.width));
    let desired_height = desired_body_height.saturating_add(QUESTION_DIALOG_CHROME_HEIGHT);
    let panel_height = area
        .height
        .min(desired_height.max(QUESTION_DIALOG_MIN_HEIGHT));
    let dialog_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(panel_height),
        width: area.width,
        height: panel_height,
    };
    // Drop temporary request borrow by ending the block above; safe to mutate now.
    let _ = tab_scroll_x_base;
    state.last_panel_height = panel_height;

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
            Constraint::Length(QUESTION_DIALOG_FOOTER_GAP_HEIGHT),
            Constraint::Length(1),
        ])
        .split(content_area);

    let cancel_text = "esc cancel";
    let cancel_width = (UnicodeWidthStr::width(cancel_text) as u16).min(chunks[0].width);
    let cancel_chunk_width = cancel_width
        .saturating_add(QUESTION_HEADER_CANCEL_GAP_WIDTH)
        .min(chunks[0].width);
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(cancel_chunk_width)])
        .split(chunks[0]);

    // Keep the custom answer row pinned to the bottom of the body so wrapping
    // options cannot clip "( ) Type your own answer" off-screen on resize.
    let pin_custom = state
        .active()
        .and_then(|r| r.current_question())
        .map(|q| !q.options.is_empty() && q.custom)
        .unwrap_or(false);
    let focus_line = {
        let Some(request) = state.active() else {
            return;
        };
        // When the custom row is sticky, keep scroll focused on option rows only.
        if pin_custom && request.current_is_custom_row() {
            usize::MAX
        } else {
            focused_body_line_index(request, &body_lines)
        }
    };
    let (scrollable_lines, sticky_custom) = split_sticky_custom_row(pin_custom, body_lines);
    let sticky_height = sticky_custom
        .as_ref()
        .map(|line| line_wrapped_height(line, chunks[1].width).max(1))
        .unwrap_or(0);
    let scroll_area = if sticky_height > 0 && chunks[1].height > sticky_height {
        Rect::new(
            chunks[1].x,
            chunks[1].y,
            chunks[1].width,
            chunks[1].height.saturating_sub(sticky_height),
        )
    } else if sticky_height > 0 {
        // Extremely short panel: prefer showing the custom row.
        Rect::new(chunks[1].x, chunks[1].y, chunks[1].width, 0)
    } else {
        chunks[1]
    };
    let sticky_area = if sticky_height > 0 {
        Rect::new(
            chunks[1].x,
            chunks[1].y.saturating_add(scroll_area.height),
            chunks[1].width,
            sticky_height.min(chunks[1].height.saturating_sub(scroll_area.height)),
        )
    } else {
        Rect::default()
    };

    ensure_body_scroll_visible(
        &scrollable_lines,
        scroll_area.width,
        scroll_area.height,
        focus_line.min(scrollable_lines.len().saturating_sub(1)),
        &mut state.body_scroll_y,
    );
    let body_scroll_y = state.body_scroll_y;

    let (tab_scroll_x, tab_hitboxes, mouse_hitboxes) = {
        let Some(request) = state.active() else {
            return;
        };
        let tab_scroll_x = active_tab_scroll(request, header_chunks[0].width);
        let tab_hitboxes = question_tab_hitboxes(request, header_chunks[0], tab_scroll_x);
        let mut mouse_hitboxes =
            question_body_hitboxes(request, &scrollable_lines, scroll_area, body_scroll_y);
        if sticky_custom.is_some() {
            if let Some(question) = request.current_question() {
                if !question.options.is_empty() && sticky_area.height > 0 {
                    mouse_hitboxes.push(QuestionMouseHitbox {
                        area: sticky_area,
                        target: QuestionMouseTarget::Option(question.options.len()),
                    });
                }
            }
        }
        mouse_hitboxes.push(QuestionMouseHitbox {
            area: header_chunks[1],
            target: QuestionMouseTarget::Cancel,
        });
        mouse_hitboxes.push(QuestionMouseHitbox {
            area: chunks[3],
            target: QuestionMouseTarget::Confirm,
        });
        (tab_scroll_x, tab_hitboxes, mouse_hitboxes)
    };

    f.render_widget(
        Paragraph::new(tabs_line).scroll((0, tab_scroll_x)),
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

    if scroll_area.height > 0 {
        f.render_widget(
            Paragraph::new(scrollable_lines)
                .style(Style::default().bg(colors.dialog_background))
                .wrap(Wrap { trim: true })
                .scroll((body_scroll_y, 0)),
            scroll_area,
        );
    }
    if let Some(custom_line) = sticky_custom {
        if sticky_area.height > 0 {
            f.render_widget(
                Paragraph::new(vec![custom_line])
                    .style(Style::default().bg(colors.dialog_background))
                    .wrap(Wrap { trim: true }),
                sticky_area,
            );
        }
    }

    f.render_widget(Paragraph::new(footer).alignment(Alignment::Left), chunks[3]);
    state.tab_hitboxes = tab_hitboxes;
    state.mouse_hitboxes = mouse_hitboxes;
}

fn split_sticky_custom_row(
    pin_custom: bool,
    mut body_lines: Vec<Line<'static>>,
) -> (Vec<Line<'static>>, Option<Line<'static>>) {
    if !pin_custom || body_lines.len() < 2 {
        return (body_lines, None);
    }
    // question_body_lines ends with the custom answer row (optionally after a blank).
    let custom = body_lines.pop();
    if body_lines
        .last()
        .map(|line| line.spans.is_empty())
        .unwrap_or(false)
    {
        body_lines.pop();
    }
    (body_lines, custom)
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
        "Skipped".to_string()
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
    spans.push(Span::raw(" move  "));

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
    use ratatui::crossterm::event::{
        KeyEvent, KeyEventKind, KeyEventState, MouseButton, MouseEvent, MouseEventKind,
    };

    fn test_theme() -> ThemeColors {
        crate::theme::Theme::load_builtin_default().get_colors(true)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn mouse_down(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_moved(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn buffer_lines(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect()
    }

    fn is_blank_dialog_row(line: &str) -> bool {
        line.chars()
            .all(|ch| ch.is_whitespace() || matches!(ch, '┃' | '│' | '▌' | '▐'))
    }

    #[test]
    fn response_returns_selected_option_labels() {
        let (tx, _rx) = oneshot::channel();
        let mut request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );
        request.confirm_current_selection();

        assert_eq!(request.response(), json!([["A"]]));
    }

    #[test]
    fn option_response_is_skipped_until_confirmed() {
        let (tx, _rx) = oneshot::channel();
        let request = QuestionDialogRequest::new(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        assert_eq!(request.response(), json!([[]]));
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

        assert_eq!(request.response(), json!([[]]));
        assert_eq!(request.answers[0].custom_text, "");

        request.begin_custom_editing();
        request.insert_char('z');
        request.finish_custom_editing();

        assert_eq!(request.response(), json!([["z"]]));
    }

    #[test]
    fn right_arrow_without_enter_keeps_question_skipped() {
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

        handle_question_dialog_key_event(&mut state, key(KeyCode::Right, KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.current_index, 1);
        assert_eq!(request.response(), json!([[]]));

        let colors = test_theme();
        let confirm_text = confirm_body_lines(request, &colors)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(confirm_text.contains("Skipped"));
    }

    #[test]
    fn single_choice_arrow_navigation_requires_enter_to_answer() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick",
                "header": "Choice",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );

        handle_question_dialog_key_event(&mut state, key(KeyCode::Down, KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].cursor, 1);
        assert_eq!(request.response(), json!([[]]));

        handle_question_dialog_key_event(&mut state, key(KeyCode::Enter, KeyModifiers::NONE));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.current_index, 1);
        assert_eq!(request.response(), json!([["B"]]));
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
    fn mouse_hovering_tabs_does_not_change_active_question() {
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

        let header_area = Rect {
            x: 4,
            y: 2,
            width: 80,
            height: 1,
        };
        state.tab_hitboxes = {
            let request = state.current.as_ref().unwrap();
            question_tab_hitboxes(
                request,
                header_area,
                active_tab_scroll(request, header_area.width),
            )
        };

        for hitbox in state
            .tab_hitboxes
            .iter()
            .skip(1)
            .copied()
            .collect::<Vec<_>>()
        {
            let action = handle_question_dialog_mouse_event(
                &mut state,
                mouse_moved(hitbox.area.x.saturating_add(1), hitbox.area.y),
            );
            assert_eq!(action, QuestionDialogAction::NotHandled);
            assert_eq!(state.current.as_ref().unwrap().current_index, 0);
        }
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
        assert_eq!(state.current.as_ref().unwrap().response(), json!([["A"]]));

        let action =
            handle_question_dialog_key_event(&mut state, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, QuestionDialogAction::Submit));
    }

    #[test]
    fn mouse_clicking_tabs_changes_active_question() {
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

        let header_area = Rect {
            x: 4,
            y: 2,
            width: 80,
            height: 1,
        };
        state.tab_hitboxes = {
            let request = state.current.as_ref().unwrap();
            question_tab_hitboxes(
                request,
                header_area,
                active_tab_scroll(request, header_area.width),
            )
        };

        let second = state.tab_hitboxes[1].area;
        let handled = handle_question_dialog_mouse_event(
            &mut state,
            mouse_down(second.x.saturating_add(1), second.y),
        );
        assert_eq!(handled, QuestionDialogAction::Handled);
        assert_eq!(state.current.as_ref().unwrap().current_index, 1);

        let confirm = state.tab_hitboxes[2].area;
        let handled = handle_question_dialog_mouse_event(
            &mut state,
            mouse_down(confirm.x.saturating_add(1), confirm.y),
        );
        assert_eq!(handled, QuestionDialogAction::Handled);
        assert_eq!(state.current.as_ref().unwrap().current_index, 2);
    }

    #[test]
    fn mouse_clicking_single_option_selects_it_and_advances() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick one",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(64, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let option_area = state
            .mouse_hitboxes
            .iter()
            .find(|hitbox| matches!(hitbox.target, QuestionMouseTarget::Option(1)))
            .unwrap()
            .area;
        assert_eq!(
            handle_question_dialog_mouse_event(
                &mut state,
                mouse_down(option_area.x, option_area.y)
            ),
            QuestionDialogAction::Handled
        );
        assert!(state.current.as_ref().unwrap().answers[0].selected[1]);
        assert!(state.current.as_ref().unwrap().is_confirm_tab());
    }

    #[test]
    fn mouse_clicking_multiple_option_toggles_without_advancing() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick any",
                "multiple": true,
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(64, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let option_area = state
            .mouse_hitboxes
            .iter()
            .find(|hitbox| matches!(hitbox.target, QuestionMouseTarget::Option(1)))
            .unwrap()
            .area;
        assert_eq!(
            handle_question_dialog_mouse_event(
                &mut state,
                mouse_down(option_area.x, option_area.y)
            ),
            QuestionDialogAction::Handled
        );

        let request = state.current.as_ref().unwrap();
        assert!(request.answers[0].selected[1]);
        assert_eq!(request.current_index, 0);
    }

    #[test]
    fn mouse_hovering_option_updates_cursor_without_selecting_it() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick one",
                "options": [{ "label": "A" }, { "label": "B" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(64, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let option_area = state
            .mouse_hitboxes
            .iter()
            .find(|hitbox| matches!(hitbox.target, QuestionMouseTarget::Option(1)))
            .unwrap()
            .area;
        let action = handle_question_dialog_mouse_event(
            &mut state,
            mouse_moved(option_area.x, option_area.y),
        );

        assert_eq!(action, QuestionDialogAction::Handled);
        let answer = &state.current.as_ref().unwrap().answers[0];
        assert_eq!(answer.cursor, 1);
        assert!(!answer.selected[1]);
    }

    #[test]
    fn active_tab_scroll_keeps_late_tabs_visible_in_narrow_viewport() {
        let (tx, _rx) = oneshot::channel();
        let mut request = QuestionDialogRequest::new(
            json!([
                { "question": "Q1", "options": [{ "label": "A" }] },
                { "question": "Q2", "options": [{ "label": "A" }] },
                { "question": "Q3", "options": [{ "label": "A" }] },
                { "question": "Q4", "options": [{ "label": "A" }] },
                { "question": "Q5", "options": [{ "label": "A" }] },
                { "question": "Q6", "options": [{ "label": "A" }] },
                { "question": "Q7", "options": [{ "label": "A" }] },
                { "question": "Q8", "options": [{ "label": "A" }] }
            ]),
            tx,
        );
        request.current_index = 7;

        let header_area = Rect {
            x: 0,
            y: 0,
            width: 35,
            height: 1,
        };
        let scroll_x = active_tab_scroll(&request, header_area.width);
        let hitboxes = question_tab_hitboxes(&request, header_area, scroll_x);

        assert!(scroll_x > 0);
        assert!(hitboxes.iter().any(|hitbox| hitbox.index == 7));
        assert!(!hitboxes.iter().any(|hitbox| hitbox.index == 0));
    }

    #[test]
    fn render_scrolls_tabs_to_active_question() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([
                { "question": "Q1", "options": [{ "label": "A" }] },
                { "question": "Q2", "options": [{ "label": "A" }] },
                { "question": "Q3", "options": [{ "label": "A" }] },
                { "question": "Q4", "options": [{ "label": "A" }] },
                { "question": "Q5", "options": [{ "label": "A" }] },
                { "question": "Q6", "options": [{ "label": "A" }] },
                { "question": "Q7", "options": [{ "label": "A" }] },
                { "question": "Q8", "options": [{ "label": "A" }] }
            ]),
            tx,
        );
        state.current.as_mut().unwrap().current_index = 7;
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(58, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Question 8"));
    }

    #[test]
    fn render_keeps_one_cell_gap_between_tabs_and_cancel_hint() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick",
                "options": [{ "label": "A" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(32, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let header = lines
            .iter()
            .find(|line| line.contains("esc cancel"))
            .expect("question dialog should render the cancel hint");
        let cancel_start = header.find("esc cancel").unwrap();
        let before_cancel = header[..cancel_start].chars().collect::<Vec<_>>();

        assert_eq!(before_cancel.last(), Some(&' '));
        assert!(before_cancel
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|ch| !ch.is_whitespace()));
    }

    #[test]
    fn render_leaves_one_row_gap_above_help_footer() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "Pick",
                "options": [{ "label": "A" }]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(48, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let footer_idx = lines
            .iter()
            .position(|line| line.contains("cycle tabs"))
            .expect("question dialog should render the help footer");

        assert!(footer_idx > 1);
        assert!(lines[footer_idx - 2].contains("Type your own answer"));
        assert!(is_blank_dialog_row(&lines[footer_idx - 1]));
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
        let colors = test_theme();
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
        let colors = test_theme();
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
        let colors = test_theme();
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
        let colors = test_theme();
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
        let colors = test_theme();
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
        let colors = test_theme();
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

        let colors = test_theme();
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
    fn option_custom_text_supports_common_terminal_navigation_sequences() {
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

        handle_question_dialog_key_event(&mut state, key(KeyCode::Down, KeyModifiers::NONE));
        handle_question_dialog_key_event(&mut state, key(KeyCode::Enter, KeyModifiers::NONE));
        state.insert_text("hello brave world");

        handle_question_dialog_key_event(&mut state, key(KeyCode::Char('b'), KeyModifiers::ALT));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 12);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Backspace, KeyModifiers::ALT));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_text, "hello world");
        assert_eq!(request.answers[0].custom_cursor, 6);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Char('f'), KeyModifiers::ALT));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 11);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Home, KeyModifiers::NONE));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 0);

        handle_question_dialog_key_event(&mut state, key(KeyCode::End, KeyModifiers::NONE));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 11);
    }

    #[test]
    fn custom_text_supports_command_arrow_navigation() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(json!([{ "question": "Explain", "header": "Details" }]), tx);
        state.insert_text("hello world");

        handle_question_dialog_key_event(&mut state, key(KeyCode::Left, KeyModifiers::SUPER));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 0);

        handle_question_dialog_key_event(&mut state, key(KeyCode::Right, KeyModifiers::SUPER));
        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_cursor, 11);
    }

    #[test]
    fn custom_text_supports_command_backspace_delete_to_start() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(json!([{ "question": "Explain", "header": "Details" }]), tx);
        state.insert_text("hello world");
        for _ in 0..5 {
            handle_question_dialog_key_event(&mut state, key(KeyCode::Left, KeyModifiers::NONE));
        }

        handle_question_dialog_key_event(&mut state, key(KeyCode::Backspace, KeyModifiers::SUPER));

        let request = state.current.as_ref().unwrap();
        assert_eq!(request.answers[0].custom_text, "world");
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
    fn render_expands_for_wrapped_options_so_custom_row_is_visible() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "What should be the first source of truth for scraped PRC data?",
                "header": "Question",
                "options": [
                    {
                        "label": "D1 / SQLite",
                        "description": "Flexible future queries, migrations, search indexes, dedupe; my likely recommendation."
                    },
                    {
                        "label": "Static JSON only",
                        "description": "Simpler deploy: generate files and serve/cache them, no DB."
                    },
                    {
                        "label": "Hybrid: D1 + raw artifacts",
                        "description": "D1 for metadata/querying, plus cached raw PDFs/parsed JSON in R2/KV/static files."
                    },
                    {
                        "label": "Decide after prototype",
                        "description": "Build an adapter so storage can be swapped after measuring data shape."
                    }
                ]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(95, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Type your own answer"));
    }

    #[test]
    fn sticky_custom_row_stays_visible_when_body_overflows_on_short_terminal() {
        use ratatui::{backend::TestBackend, Terminal};

        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([{
                "question": "When designing a long-running Rust TUI application like crabcode that needs to coordinate streaming LLM responses, tool execution, SQLite preference persistence, and reactive Ratatui rendering on a single event loop, which architectural tradeoff do you consider most important for the next major refactor?",
                "header": "Architecture",
                "options": [
                    {
                        "label": "Single-threaded actor bus",
                        "description": "Keep one event loop and ordered channels; prioritize determinism and simpler reasoning about UI state."
                    },
                    {
                        "label": "Hybrid workers + main UI",
                        "description": "Move blocking/network work off the UI thread while keeping Ratatui rendering on the main thread."
                    },
                    {
                        "label": "Headless core + thin TUI",
                        "description": "Extract a reusable session engine so CLI/headless and TUI share one state machine."
                    }
                ]
            }]),
            tx,
        );
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        // Narrow + short: wrapped options exceed body height and previously clipped
        // the custom answer row off the bottom.
        let backend = TestBackend::new(72, 18);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_question_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains("Type your own answer"),
            "custom row should stay pinned visible on overflow:\n{rendered}"
        );
    }

    #[test]
    fn current_snapshot_exposes_questions_for_remote_clients() {
        let (tx, _rx) = oneshot::channel();
        let mut state = QuestionDialogState::new();
        state.enqueue(
            json!([
                {
                    "question": "Pick an approach",
                    "header": "Approach",
                    "options": [
                        { "label": "Small", "description": "Minimal change" },
                        { "label": "Full", "description": "Complete change" }
                    ]
                }
            ]),
            tx,
        );

        let snapshot = state.current_snapshot().unwrap();
        assert_eq!(snapshot.questions.len(), 1);
        assert_eq!(snapshot.questions[0].header, "Approach");
        assert_eq!(snapshot.questions[0].question, "Pick an approach");
        assert_eq!(snapshot.questions[0].options.len(), 2);
        assert_eq!(snapshot.questions[0].options[0].label, "Small");
        assert!(snapshot.questions[0].custom);
        assert_eq!(snapshot.queued_count, 0);
    }

    #[test]
    fn option_line_has_no_cursor_marker() {
        let option = QuestionOption {
            label: "A".to_string(),
            description: String::new(),
        };
        let colors = test_theme();
        let line = option_line(&option, true, true, false, &colors);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(text.starts_with("(*) "));
    }
}
