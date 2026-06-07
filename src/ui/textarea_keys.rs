use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, TextArea};

pub(crate) fn has_command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META)
}

pub(crate) fn input_textarea(textarea: &mut TextArea<'static>, event: KeyEvent) -> bool {
    let cmd = has_command_modifier(event.modifiers);
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);

    match event.code {
        KeyCode::Left if cmd => textarea.move_cursor(CursorMove::Head),
        KeyCode::Right if cmd => textarea.move_cursor(CursorMove::End),
        KeyCode::Backspace if cmd => {
            textarea.delete_line_by_head();
        }
        KeyCode::Char('a') if ctrl => textarea.move_cursor(CursorMove::Head),
        KeyCode::Char('e') if ctrl => textarea.move_cursor(CursorMove::End),
        KeyCode::Char('u') if ctrl => {
            textarea.delete_line_by_head();
        }
        _ => return textarea.input(event),
    };

    true
}
