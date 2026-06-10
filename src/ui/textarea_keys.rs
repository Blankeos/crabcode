use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, TextArea};

pub(crate) fn has_command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META)
}

fn line_end_col(textarea: &TextArea<'static>, row: usize) -> usize {
    textarea
        .lines()
        .get(row)
        .map(|line| line.chars().count())
        .unwrap_or(0)
}

pub(crate) fn delete_to_line_start(textarea: &mut TextArea<'static>) {
    if textarea.is_selecting() {
        textarea.delete_char();
        return;
    }

    let (cursor_row, cursor_col) = textarea.cursor();

    if let Some(line) = textarea.lines().get(cursor_row) {
        let delete_count = cursor_col.min(line.chars().count());
        for _ in 0..delete_count {
            textarea.delete_char();
        }
    }
}

pub(crate) fn command_backspace_to_line_start(textarea: &mut TextArea<'static>) {
    if textarea.is_selecting() {
        textarea.delete_char();
        return;
    }

    let (cursor_row, cursor_col) = textarea.cursor();

    if cursor_col == 0 {
        if cursor_row > 0 {
            let previous_row = cursor_row - 1;
            let previous_col = line_end_col(textarea, previous_row);
            textarea.move_cursor(CursorMove::Jump(previous_row as u16, previous_col as u16));
        }
        return;
    }

    delete_to_line_start(textarea);
}

pub(crate) fn input_textarea(textarea: &mut TextArea<'static>, event: KeyEvent) -> bool {
    let cmd = has_command_modifier(event.modifiers);
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);

    match event.code {
        KeyCode::Left if cmd => textarea.move_cursor(CursorMove::Head),
        KeyCode::Right if cmd => textarea.move_cursor(CursorMove::End),
        KeyCode::Backspace if cmd => {
            command_backspace_to_line_start(textarea);
        }
        KeyCode::Char('a') if ctrl => textarea.move_cursor(CursorMove::Head),
        KeyCode::Char('e') if ctrl => textarea.move_cursor(CursorMove::End),
        KeyCode::Char('u') if ctrl => {
            delete_to_line_start(textarea);
        }
        _ => return textarea.input(event),
    };

    true
}
