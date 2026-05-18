use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use crate::theme::ThemeColors;
use unicode_width::UnicodeWidthStr;

const MAX_DIFF_LINES: usize = 40;
const CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineType {
    Remove,
    Add,
    Context,
}

pub struct DiffLine {
    pub line_type: DiffLineType,
    pub text: String,
}

/// Compute a unified line-based diff between old and new text.
/// Returns at most `MAX_DIFF_LINES` with `CONTEXT_LINES` of context around changes.
pub fn compute_unified_diff(old_text: &str, new_text: &str) -> Vec<DiffLine> {
    let raw_diff = diff::lines(old_text, new_text);

    // First pass: collect all lines with their type
    let mut all_lines: Vec<DiffLine> = Vec::new();
    for result in raw_diff {
        match result {
            diff::Result::Left(line) => {
                all_lines.push(DiffLine {
                    line_type: DiffLineType::Remove,
                    text: line.to_string(),
                });
            }
            diff::Result::Both(line, _) => {
                all_lines.push(DiffLine {
                    line_type: DiffLineType::Context,
                    text: line.to_string(),
                });
            }
            diff::Result::Right(line) => {
                all_lines.push(DiffLine {
                    line_type: DiffLineType::Add,
                    text: line.to_string(),
                });
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
                text: "⋯".to_string(),
            });
            in_ellipsis = true;
        }
    }

    result
}

/// Render a unified diff as ratatui Lines with proper colors and gutter.
/// Every line is padded to `max_width` so the background spans the full row.
pub fn render_unified_diff(
    diff_lines: &[DiffLine],
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let gutter_width = 2usize; // "- ", "+ ", "  "
    let content_width = max_width.saturating_sub(gutter_width).max(1);

    let mut lines: Vec<Line<'static>> = Vec::new();

    if max_width < 4 {
        return lines;
    }

    for diff_line in diff_lines {
        let (gutter, fg, bg) = match diff_line.line_type {
            DiffLineType::Remove => ("- ", colors.diff_remove, colors.diff_remove_bg),
            DiffLineType::Add => ("+ ", colors.diff_add, colors.diff_add_bg),
            DiffLineType::Context => ("  ", colors.text_weak, colors.background),
        };

        let gutter_style = Style::default().fg(colors.diff_gutter).bg(bg);
        let content_style = Style::default().fg(fg).bg(bg);
        let pad_style = Style::default().bg(bg);

        // Handle ellipsis specially
        if diff_line.text == "⋯" {
            let full_line = format!("{}⋯", gutter);
            let remaining = max_width.saturating_sub(full_line.len());
            let padding = "─".repeat(remaining);
            let mut spans = vec![
                Span::styled(gutter.to_string(), gutter_style),
                Span::styled(format!("⋯{}", padding), content_style.add_modifier(Modifier::DIM)),
            ];
            // Pad to full width if the ellipsis line is shorter
            let visible_width: usize = spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
            if visible_width < max_width {
                spans.push(Span::styled(" ".repeat(max_width - visible_width), pad_style));
            }
            lines.push(Line::from(spans));
            continue;
        }

        // Wrap content if needed
        let wrapped = textwrap::wrap(&diff_line.text, content_width);
        for (chunk_idx, chunk) in wrapped.iter().enumerate() {
            let gutter_text = if chunk_idx == 0 {
                gutter.to_string()
            } else {
                "  ".to_string()
            };
            let mut spans = vec![
                Span::styled(gutter_text.clone(), gutter_style),
                Span::styled(chunk.to_string(), content_style),
            ];
            // Pad to full width so the background spans the entire row
            let visible_width: usize = spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
            if visible_width < max_width {
                spans.push(Span::styled(" ".repeat(max_width - visible_width), pad_style));
            }
            lines.push(Line::from(spans));
        }
    }

    lines
}

/// Convenience: compute and render a unified diff in one call.
pub fn format_edit_diff(
    old_string: &str,
    new_string: &str,
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let diff_lines = compute_unified_diff(old_string, new_string);
    render_unified_diff(&diff_lines, max_width, colors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

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
}
