use crate::theme::ThemeColors;
use crate::ui::selection::non_selectable_style;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_DIFF_LINES: usize = 40;
const CONTEXT_LINES: usize = 3;
const TAB_WIDTH: usize = 4;
const GUTTER_DIFF_BG_ALPHA: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineType {
    Remove,
    Add,
    Context,
}

pub struct DiffLine {
    pub line_type: DiffLineType,
    pub line_number: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
}

/// Compute a unified line-based diff between old and new text.
/// Returns at most `MAX_DIFF_LINES` with `CONTEXT_LINES` of context around changes.
pub fn compute_unified_diff(old_text: &str, new_text: &str) -> Vec<DiffLine> {
    compute_unified_diff_with_start(old_text, new_text, 1, 1)
}

/// Compute a unified line-based diff with explicit old/new starting line numbers.
pub fn compute_unified_diff_with_start(
    old_text: &str,
    new_text: &str,
    old_start_line: usize,
    new_start_line: usize,
) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let raw_diff = diff::slice(&old_lines, &new_lines);

    // First pass: collect all lines with their type
    let mut all_lines: Vec<DiffLine> = Vec::new();
    let mut old_line = old_start_line.max(1);
    let mut new_line = new_start_line.max(1);
    for result in raw_diff {
        match result {
            diff::Result::Left(line) => {
                all_lines.push(DiffLine {
                    line_type: DiffLineType::Remove,
                    line_number: Some(old_line),
                    text: (*line).to_string(),
                });
                old_line += 1;
            }
            diff::Result::Both(line, _) => {
                all_lines.push(DiffLine {
                    line_type: DiffLineType::Context,
                    line_number: Some(new_line),
                    text: (*line).to_string(),
                });
                old_line += 1;
                new_line += 1;
            }
            diff::Result::Right(line) => {
                all_lines.push(DiffLine {
                    line_type: DiffLineType::Add,
                    line_number: Some(new_line),
                    text: (*line).to_string(),
                });
                new_line += 1;
            }
        }
    }

    // If the diff is short enough, return it all
    if all_lines.len() <= MAX_DIFF_LINES {
        return all_lines;
    }

    // Otherwise, find change regions and include context around them
    let mut change_indices: Vec<usize> = Vec::new();
    for (i, line) in all_lines.iter().enumerate() {
        if line.line_type != DiffLineType::Context {
            change_indices.push(i);
        }
    }

    if change_indices.is_empty() {
        // No changes? Return first context lines
        return all_lines.into_iter().take(MAX_DIFF_LINES).collect();
    }

    // Build a set of indices to keep
    let mut keep = vec![false; all_lines.len()];
    for &idx in &change_indices {
        let start = idx.saturating_sub(CONTEXT_LINES);
        let end = (idx + CONTEXT_LINES + 1).min(all_lines.len());
        for i in start..end {
            keep[i] = true;
        }
    }

    // Merge adjacent kept regions and add ellipsis markers
    let mut result: Vec<DiffLine> = Vec::new();
    let mut in_ellipsis = false;
    for (i, line) in all_lines.into_iter().enumerate() {
        if keep[i] {
            result.push(line);
            in_ellipsis = false;
        } else if !in_ellipsis {
            result.push(DiffLine {
                line_type: DiffLineType::Context,
                line_number: None,
                text: "⋯".to_string(),
            });
            in_ellipsis = true;
        }
    }

    result
}

pub fn compute_diff_stats(old_text: &str, new_text: &str) -> DiffStats {
    let mut stats = DiffStats {
        added: 0,
        removed: 0,
    };

    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    for result in diff::slice(&old_lines, &new_lines) {
        match result {
            diff::Result::Left(_) => stats.removed += 1,
            diff::Result::Right(_) => stats.added += 1,
            diff::Result::Both(_, _) => {}
        }
    }

    stats
}

/// Render a unified diff as ratatui Lines with proper colors and gutter.
/// Every line is padded to `max_width` so the background spans the full row.
pub fn render_unified_diff(
    diff_lines: &[DiffLine],
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    render_unified_diff_with_indent(diff_lines, max_width, colors, "")
}

/// Render a unified diff with a fixed left indent before the line-number gutter.
pub fn render_unified_diff_with_indent(
    diff_lines: &[DiffLine],
    max_width: usize,
    colors: &ThemeColors,
    indent: &str,
) -> Vec<Line<'static>> {
    render_unified_diff_with_indent_and_syntax(diff_lines, max_width, colors, indent, None, None, 1)
}

fn render_unified_diff_with_indent_and_syntax(
    diff_lines: &[DiffLine],
    max_width: usize,
    colors: &ThemeColors,
    indent: &str,
    old_syntax_lines: Option<&[Vec<Span<'static>>]>,
    new_syntax_lines: Option<&[Vec<Span<'static>>]>,
    start_line: usize,
) -> Vec<Line<'static>> {
    let max_line_number = diff_lines
        .iter()
        .filter_map(|line| line.line_number)
        .max()
        .unwrap_or(1);
    let line_number_width = max_line_number.to_string().len().max(1);
    let indent_width = UnicodeWidthStr::width(indent);
    let gutter_width = line_number_width + 2; // line number, spacer, sign
    let content_width = max_width.saturating_sub(indent_width + gutter_width).max(1);

    let mut lines: Vec<Line<'static>> = Vec::new();

    if max_width < indent_width + gutter_width + 1 {
        return lines;
    }

    for diff_line in diff_lines {
        let (sign, fg, bg) = match diff_line.line_type {
            DiffLineType::Remove => ('-', colors.diff_remove, colors.diff_remove_bg),
            DiffLineType::Add => ('+', colors.diff_add, colors.diff_add_bg),
            DiffLineType::Context => (' ', colors.text_weak, colors.background),
        };
        let gutter_bg = diff_gutter_bg(diff_line.line_type, bg, colors.background);

        let indent_style = non_selectable_style(Style::default().bg(gutter_bg));
        let gutter_style = non_selectable_style(
            Style::default()
                .fg(colors.diff_gutter)
                .bg(gutter_bg)
                .add_modifier(Modifier::DIM),
        );
        let sign_style = non_selectable_style(Style::default().fg(fg).bg(gutter_bg));
        let content_style = Style::default().fg(fg).bg(bg);
        let pad_style = Style::default().bg(bg);

        // Handle ellipsis specially
        if diff_line.text == "⋯" {
            let number = " ".repeat(line_number_width);
            let full_line = format!("{}{}  ⋯", indent, number);
            let remaining = max_width.saturating_sub(UnicodeWidthStr::width(full_line.as_str()));
            let padding = "─".repeat(remaining);
            let mut spans = vec![
                Span::styled(indent.to_string(), indent_style),
                Span::styled(format!("{}  ", number), gutter_style),
                Span::styled(
                    format!("⋯{}", padding),
                    content_style.add_modifier(Modifier::DIM),
                ),
            ];
            // Pad to full width if the ellipsis line is shorter
            let visible_width: usize = spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            if visible_width < max_width {
                spans.push(Span::styled(
                    " ".repeat(max_width - visible_width),
                    non_selectable_style(pad_style),
                ));
            }
            lines.push(Line::from(spans));
            continue;
        }

        let syntax_spans =
            syntax_spans_for_diff_line(diff_line, start_line, old_syntax_lines, new_syntax_lines);
        let wrapped_syntax_spans = syntax_spans.map(|spans| {
            let styled = spans
                .iter()
                .map(|span| {
                    let mut style = span.style.bg(bg);
                    if style.fg.is_none() {
                        style = style.fg(colors.text);
                    }
                    if matches!(diff_line.line_type, DiffLineType::Remove) {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    Span::styled(span.content.clone().into_owned(), style)
                })
                .collect::<Vec<_>>();
            wrap_styled_spans(&styled, content_width)
        });
        let wrapped_plain = wrapped_syntax_spans
            .is_none()
            .then(|| textwrap::wrap(&diff_line.text, content_width));
        let chunk_count = wrapped_syntax_spans
            .as_ref()
            .map(|chunks| chunks.len())
            .or_else(|| wrapped_plain.as_ref().map(|chunks| chunks.len()))
            .unwrap_or(0);

        for chunk_idx in 0..chunk_count {
            let number_text = if chunk_idx == 0 {
                diff_line
                    .line_number
                    .map(|line_number| format!("{line_number:>line_number_width$} "))
                    .unwrap_or_else(|| format!("{:line_number_width$} ", ""))
            } else {
                format!("{:line_number_width$} ", "")
            };
            let sign_text = if chunk_idx == 0 {
                sign.to_string()
            } else {
                " ".to_string()
            };
            let mut spans = vec![
                Span::styled(indent.to_string(), indent_style),
                Span::styled(number_text, gutter_style),
                Span::styled(sign_text, sign_style),
            ];
            if let Some(chunks) = wrapped_syntax_spans.as_ref() {
                spans.extend(chunks[chunk_idx].clone());
            } else if let Some(chunks) = wrapped_plain.as_ref() {
                spans.push(Span::styled(chunks[chunk_idx].to_string(), content_style));
            }
            // Pad to full width so the background spans the entire row
            let visible_width: usize = spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            if visible_width < max_width {
                spans.push(Span::styled(
                    " ".repeat(max_width - visible_width),
                    non_selectable_style(pad_style),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    lines
}

fn diff_gutter_bg(line_type: DiffLineType, diff_bg: Color, base_bg: Color) -> Color {
    match line_type {
        DiffLineType::Add | DiffLineType::Remove => {
            blend_colors(diff_bg, base_bg, GUTTER_DIFF_BG_ALPHA)
        }
        DiffLineType::Context => diff_bg,
    }
}

fn blend_colors(foreground: Color, background: Color, alpha: f32) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(br, bg, bb)) = (foreground, background) else {
        return foreground;
    };

    let mix = |front: u8, back: u8| {
        (front as f32 * alpha + back as f32 * (1.0 - alpha))
            .round()
            .clamp(0.0, 255.0) as u8
    };

    Color::Rgb(mix(fr, br), mix(fg, bg), mix(fb, bb))
}

fn syntax_spans_for_diff_line<'a>(
    diff_line: &DiffLine,
    start_line: usize,
    old_syntax_lines: Option<&'a [Vec<Span<'static>>]>,
    new_syntax_lines: Option<&'a [Vec<Span<'static>>]>,
) -> Option<&'a [Span<'static>]> {
    let line_number = diff_line.line_number?;
    let index = line_number.checked_sub(start_line)?;
    match diff_line.line_type {
        DiffLineType::Remove => old_syntax_lines,
        DiffLineType::Add | DiffLineType::Context => new_syntax_lines,
    }
    .and_then(|lines| lines.get(index))
    .map(Vec::as_slice)
}

fn wrap_styled_spans(spans: &[Span<'static>], max_cols: usize) -> Vec<Vec<Span<'static>>> {
    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut col: usize = 0;

    for span in spans {
        let style = span.style;
        let mut remaining = span.content.as_ref();

        while !remaining.is_empty() {
            let mut byte_end = 0;
            let mut chars_col = 0;

            for ch in remaining.chars() {
                let width = ch.width().unwrap_or(if ch == '\t' { TAB_WIDTH } else { 0 });
                if col + chars_col + width > max_cols {
                    break;
                }
                byte_end += ch.len_utf8();
                chars_col += width;
            }

            if byte_end == 0 {
                if !current_line.is_empty() {
                    result.push(std::mem::take(&mut current_line));
                    col = 0;
                }
                let Some(ch) = remaining.chars().next() else {
                    break;
                };
                let ch_len = ch.len_utf8();
                current_line.push(Span::styled(remaining[..ch_len].to_string(), style));
                col = ch.width().unwrap_or(if ch == '\t' { TAB_WIDTH } else { 1 });
                remaining = &remaining[ch_len..];
                continue;
            }

            let (chunk, rest) = remaining.split_at(byte_end);
            current_line.push(Span::styled(chunk.to_string(), style));
            col += chars_col;
            remaining = rest;

            if col >= max_cols {
                result.push(std::mem::take(&mut current_line));
                col = 0;
            }
        }
    }

    if !current_line.is_empty() || result.is_empty() {
        result.push(current_line);
    }

    result
}

/// Convenience: compute and render a unified diff in one call.
pub fn format_edit_diff(
    old_string: &str,
    new_string: &str,
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    format_edit_diff_with_start(old_string, new_string, 1, max_width, colors, "")
}

pub fn format_edit_diff_with_start(
    old_string: &str,
    new_string: &str,
    start_line: usize,
    max_width: usize,
    colors: &ThemeColors,
    indent: &str,
) -> Vec<Line<'static>> {
    let diff_lines =
        compute_unified_diff_with_start(old_string, new_string, start_line, start_line);
    render_unified_diff_with_indent(&diff_lines, max_width, colors, indent)
}

pub fn format_edit_diff_for_path_with_start(
    old_string: &str,
    new_string: &str,
    start_line: usize,
    max_width: usize,
    colors: &ThemeColors,
    indent: &str,
    path: &str,
) -> Vec<Line<'static>> {
    let diff_lines =
        compute_unified_diff_with_start(old_string, new_string, start_line, start_line);
    let old_syntax_lines = crate::ui::syntax::highlight_code_for_path(old_string, path, colors);
    let new_syntax_lines = crate::ui::syntax::highlight_code_for_path(new_string, path, colors);
    render_unified_diff_with_indent_and_syntax(
        &diff_lines,
        max_width,
        colors,
        indent,
        old_syntax_lines.as_deref(),
        new_syntax_lines.as_deref(),
        start_line,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn test_colors() -> ThemeColors {
        ThemeColors {
            primary: Color::Reset,
            secondary: Color::Reset,
            accent: Color::Reset,
            interactive: Color::Reset,
            background: Color::Reset,
            dialog_background: Color::Reset,
            background_element: Color::Reset,
            text: Color::Reset,
            text_weak: Color::Reset,
            text_strong: Color::Reset,
            border: Color::Reset,
            border_weak_focus: Color::Reset,
            border_focus: Color::Reset,
            border_strong_focus: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            error: Color::Reset,
            info: Color::Reset,
            markdown_text: Color::Reset,
            markdown_heading: Color::Reset,
            markdown_link: Color::Reset,
            markdown_link_text: Color::Reset,
            markdown_code: Color::Reset,
            markdown_block_quote: Color::Reset,
            markdown_emph: Color::Reset,
            markdown_strong: Color::Reset,
            markdown_horizontal_rule: Color::Reset,
            markdown_list_item: Color::Reset,
            markdown_list_enumeration: Color::Reset,
            markdown_image: Color::Reset,
            markdown_image_text: Color::Reset,
            markdown_code_block: Color::Reset,
            diff_add: Color::Rgb(0, 255, 0),
            diff_add_bg: Color::Rgb(0, 60, 0),
            diff_remove: Color::Rgb(255, 0, 0),
            diff_remove_bg: Color::Rgb(60, 0, 0),
            diff_gutter: Color::Rgb(140, 140, 140),
        }
    }

    #[test]
    fn test_compute_unified_diff_simple() {
        let old = "line1\nline2\nline3";
        let new = "line1\nchanged\nline3";
        let diff = compute_unified_diff(old, new);
        assert_eq!(diff.len(), 4);
        assert_eq!(diff[0].line_type, DiffLineType::Context);
        assert_eq!(diff[0].text, "line1");
        assert_eq!(diff[1].line_type, DiffLineType::Remove);
        assert_eq!(diff[1].text, "line2");
        assert_eq!(diff[2].line_type, DiffLineType::Add);
        assert_eq!(diff[2].text, "changed");
        assert_eq!(diff[3].line_type, DiffLineType::Context);
        assert_eq!(diff[3].text, "line3");
    }

    #[test]
    fn test_compute_unified_diff_insertion() {
        let old = "line1";
        let new = "line1\nline2";
        let diff = compute_unified_diff(old, new);
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].line_type, DiffLineType::Context);
        assert_eq!(diff[0].text, "line1");
        assert_eq!(diff[1].line_type, DiffLineType::Add);
        assert_eq!(diff[1].text, "line2");
    }

    #[test]
    fn test_compute_diff_stats_ignores_terminal_newline() {
        let stats = compute_diff_stats("", "line1\n");
        assert_eq!(stats.added, 1);
        assert_eq!(stats.removed, 0);
    }

    #[test]
    fn test_compute_unified_diff_deletion() {
        let old = "line1\nline2";
        let new = "line1";
        let diff = compute_unified_diff(old, new);
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].line_type, DiffLineType::Context);
        assert_eq!(diff[0].text, "line1");
        assert_eq!(diff[1].line_type, DiffLineType::Remove);
        assert_eq!(diff[1].text, "line2");
    }

    #[test]
    fn test_render_unified_diff_produces_lines() {
        let colors = test_colors();
        let old = "a\nb\nc";
        let new = "a\nX\nc";
        let lines = format_edit_diff(old, new, 40, &colors);
        assert!(!lines.is_empty());
        // Each line should have at least 2 spans (gutter + content)
        for line in &lines {
            assert!(line.spans.len() >= 2);
        }
    }

    #[test]
    fn test_render_unified_diff_narrow_width() {
        let colors = test_colors();
        let old = "a\nb\nc";
        let new = "a\nX\nc";
        let lines = format_edit_diff(old, new, 3, &colors);
        // Should still produce lines (width >= 4 is needed)
        // With width 3, returns empty
        assert!(lines.is_empty());
    }

    #[test]
    fn test_render_unified_diff_highlights_known_file_extension() {
        let colors = test_colors();
        let old = "fn value() -> u8 {\n    1\n}";
        let new = "fn value() -> u8 {\n    2\n}";

        let lines =
            format_edit_diff_for_path_with_start(old, new, 1, 80, &colors, "", "src/lib.rs");

        let context_line = lines
            .iter()
            .find(|line| line_text(line).contains("fn value"))
            .expect("expected context line");
        assert!(
            context_line.spans.iter().any(|span| {
                span.content.as_ref().contains("fn")
                    && span.style.fg.is_some()
                    && span.style.fg != Some(colors.text_weak)
            }),
            "expected syntax-colored Rust keyword span"
        );
    }

    #[test]
    fn test_render_unified_diff_keeps_syntax_foreground_after_diff_signs() {
        let colors = test_colors();
        let old = "let value = false;\n";
        let new = "let value = true;\n";

        let lines =
            format_edit_diff_for_path_with_start(old, new, 1, 80, &colors, "", "src/lib.rs");
        let removed_line = lines
            .iter()
            .find(|line| line_text(line).contains("-let value"))
            .expect("expected removed line");
        let added_line = lines
            .iter()
            .find(|line| line_text(line).contains("+let value"))
            .expect("expected added line");

        let removed_identifier = removed_line
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("value"))
            .expect("expected removed identifier span");
        let added_identifier = added_line
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("value"))
            .expect("expected added identifier span");

        assert_ne!(removed_identifier.style.fg, Some(colors.diff_remove));
        assert_eq!(removed_identifier.style.bg, Some(colors.diff_remove_bg));
        assert!(removed_identifier
            .style
            .add_modifier
            .contains(Modifier::DIM));
        assert_ne!(added_identifier.style.fg, Some(colors.diff_add));
        assert_eq!(added_identifier.style.bg, Some(colors.diff_add_bg));
    }

    #[test]
    fn test_render_unified_diff_highlights_typescript_additions() {
        let colors = test_colors();
        let new = "import { argv } from 'node:process'\n\nconsole.log(`hello ${argv[2]}`)\n";

        let lines =
            format_edit_diff_for_path_with_start("", new, 1, 100, &colors, "", "scripts/script.ts");
        let import_line = lines
            .iter()
            .find(|line| line_text(line).contains("+import"))
            .expect("expected TypeScript import line");
        let import_span = import_line
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("import"))
            .expect("expected import content span");

        assert_ne!(import_span.style.fg, Some(colors.diff_add));
        assert_eq!(import_span.style.bg, Some(colors.diff_add_bg));
    }

    #[test]
    fn test_render_unified_diff_gutter_is_not_selection_highlighted_or_copied() {
        let colors = test_colors();
        let lines = format_edit_diff("old", "new", 40, &colors);
        let added_idx = lines
            .iter()
            .position(|line| line_text(line).contains("+new"))
            .expect("expected added line");
        let selection = crate::ui::selection::Selection {
            active: true,
            start_line: added_idx,
            start_col: 0,
            end_line: added_idx,
            end_col: 8,
            is_dragging: false,
            anchor: None,
        };

        let copied = crate::ui::selection::extract_selected_text(&lines, &selection)
            .expect("expected copied content");
        assert_eq!(copied, "new");

        let selected_lines = crate::ui::selection::apply_selection_to_lines(
            lines.clone(),
            &selection,
            Color::Rgb(128, 0, 255),
        );
        let selected_line = &selected_lines[added_idx];
        assert_ne!(
            selected_line.spans[0].style.bg,
            Some(Color::Rgb(128, 0, 255))
        );
        assert_ne!(
            selected_line.spans[1].style.bg,
            Some(Color::Rgb(128, 0, 255))
        );
        assert_ne!(
            selected_line.spans[2].style.bg,
            Some(Color::Rgb(128, 0, 255))
        );
        assert!(selected_line
            .spans
            .iter()
            .any(|span| span.content.as_ref().contains("new")
                && span.style.bg == Some(Color::Rgb(128, 0, 255))));
    }

    #[test]
    fn test_render_unified_diff_uses_softer_gutter_background_for_changes() {
        let mut colors = test_colors();
        colors.background = Color::Rgb(10, 10, 10);
        colors.diff_add_bg = Color::Rgb(10, 70, 20);

        let lines = format_edit_diff_for_path_with_start(
            "",
            "let value = true;\n",
            1,
            80,
            &colors,
            "",
            "src/lib.rs",
        );
        let added_line = lines
            .iter()
            .find(|line| line_text(line).contains("+let value"))
            .expect("expected added line");

        assert_eq!(added_line.spans[1].style.bg, Some(Color::Rgb(10, 43, 16)));
        assert_eq!(added_line.spans[2].style.bg, Some(Color::Rgb(10, 43, 16)));
        assert_eq!(added_line.spans[3].style.bg, Some(colors.diff_add_bg));
    }
}
