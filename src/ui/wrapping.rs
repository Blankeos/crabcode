use std::ops::Range;

use crate::ui::selection::NON_SELECTABLE_SPAN_MODIFIER;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TAB_STOP_WIDTH: usize = 4;

#[derive(Debug, Clone)]
pub struct WrapOptions<'a> {
    pub width: usize,
    pub initial_indent: Line<'a>,
    pub subsequent_indent: Line<'a>,
}

impl WrapOptions<'_> {
    pub fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            initial_indent: Line::default(),
            subsequent_indent: Line::default(),
        }
    }
}

impl<'a> WrapOptions<'a> {
    pub fn initial_indent(mut self, indent: Line<'a>) -> Self {
        self.initial_indent = indent;
        self
    }

    pub fn subsequent_indent(mut self, indent: Line<'a>) -> Self {
        self.subsequent_indent = indent;
        self
    }
}

impl From<usize> for WrapOptions<'_> {
    fn from(width: usize) -> Self {
        Self::new(width)
    }
}

pub fn wrap_styled_line<'a, O>(line: &'a Line<'a>, options: O) -> Vec<Line<'static>>
where
    O: Into<WrapOptions<'a>>,
{
    let options = options.into();
    let line = sanitize_line(line);
    let mut flat = String::new();
    let mut span_bounds = Vec::with_capacity(line.spans.len());

    for span in &line.spans {
        let start = flat.len();
        flat.push_str(span.content.as_ref());
        let end = flat.len();
        span_bounds.push((start..end, span.style));
    }

    if flat.is_empty() {
        return vec![line_with_indent(&options.initial_indent, line.style)];
    }

    let first_width = options
        .width
        .saturating_sub(options.initial_indent.width())
        .max(1);
    let subsequent_width = options
        .width
        .saturating_sub(options.subsequent_indent.width())
        .max(1);

    let mut ranges = wrap_ranges(&flat, first_width, subsequent_width);
    if ranges.is_empty() {
        ranges.push(0..0);
    }

    ranges
        .into_iter()
        .enumerate()
        .map(|(idx, range)| {
            let indent = if idx == 0 {
                &options.initial_indent
            } else {
                &options.subsequent_indent
            };
            line_from_range(&line, &span_bounds, &range, indent)
        })
        .collect()
}

fn sanitize_line(line: &Line<'_>) -> Line<'static> {
    let mut column = 0;
    let spans = line
        .spans
        .iter()
        .map(|span| {
            let mut content = String::with_capacity(span.content.len());
            for ch in span.content.chars() {
                match ch {
                    '\t' => {
                        let spaces = TAB_STOP_WIDTH - column % TAB_STOP_WIDTH;
                        content.extend(std::iter::repeat_n(' ', spaces));
                        column += spaces;
                    }
                    '\n' | '\r' => {
                        content.push(ch);
                        column = 0;
                    }
                    ch if ch.is_control() => {
                        content.push('�');
                        column += 1;
                    }
                    ch => {
                        content.push(ch);
                        column += ch.width().unwrap_or(0);
                    }
                }
            }
            Span::styled(content, span.style)
        })
        .collect();

    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

pub fn wrap_styled_lines<'a, I, O>(lines: I, options: O) -> Vec<Line<'static>>
where
    I: IntoIterator<Item = &'a Line<'a>>,
    O: Into<WrapOptions<'a>>,
{
    let base_options = options.into();
    let mut out = Vec::new();

    for (idx, line) in lines.into_iter().enumerate() {
        let opts = if idx == 0 {
            base_options.clone()
        } else {
            base_options
                .clone()
                .initial_indent(base_options.subsequent_indent.clone())
        };
        out.extend(wrap_styled_line(line, opts));
    }

    out
}

/// Remove terminal control characters from an already wrapped line.
///
/// Ratatui measures a tab as one column, while terminals execute it as a jump
/// to the next tab stop. Letting one reach Crossterm desynchronizes Ratatui's
/// tracked cursor from the real terminal until the next full redraw.
pub fn sanitize_styled_line(line: &Line<'_>) -> Line<'static> {
    let spans = line
        .spans
        .iter()
        .map(|span| {
            let content = span
                .content
                .chars()
                .map(|ch| if ch.is_control() { ' ' } else { ch })
                .collect::<String>();
            Span::styled(content, span.style)
        })
        .collect();

    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

fn wrap_ranges(text: &str, first_width: usize, subsequent_width: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut first_segment = true;

    for logical_line in logical_line_ranges(text) {
        let line = &text[logical_line.clone()];
        if line.is_empty() {
            ranges.push(logical_line.start..logical_line.start);
            first_segment = false;
            continue;
        }

        let line_first_width = if first_segment {
            first_width
        } else {
            subsequent_width
        };
        let line_ranges = wrap_single_line_ranges(line, line_first_width, subsequent_width);
        ranges.extend(
            line_ranges
                .into_iter()
                .map(|range| logical_line.start + range.start..logical_line.start + range.end),
        );
        first_segment = false;
    }

    ranges
}

fn logical_line_ranges(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut idx = 0;

    while idx < bytes.len() {
        if matches!(bytes[idx], b'\n' | b'\r') {
            ranges.push(start..idx);
            if bytes[idx] == b'\r' && bytes.get(idx + 1) == Some(&b'\n') {
                idx += 1;
            }
            start = idx + 1;
        }
        idx += 1;
    }
    ranges.push(start..text.len());
    ranges
}

fn wrap_single_line_ranges(
    text: &str,
    first_width: usize,
    subsequent_width: usize,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut width = first_width.max(1);
    let mut first_segment = true;

    while start < text.len() {
        if !first_segment {
            start = skip_breaking_whitespace(text, start);
        }
        if start >= text.len() {
            break;
        }

        let remaining = &text[start..];
        if UnicodeWidthStr::width(remaining) <= width {
            ranges.push(start..trim_trailing_whitespace(text, text.len(), start));
            break;
        }

        let mut used_width = 0;
        let mut last_break: Option<(usize, usize)> = None;
        let mut forced_break = None;

        for (offset, ch) in remaining.char_indices() {
            let byte_idx = start + offset;
            let next = byte_idx + ch.len_utf8();
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

            if used_width + ch_width > width {
                forced_break = Some(byte_idx);
                break;
            }

            used_width += ch_width;
            if ch.is_whitespace() && byte_idx > start {
                last_break = Some((byte_idx, next));
            }
        }

        if let Some((break_start, break_end)) = last_break {
            if is_leading_list_marker_break(text, start, break_start, break_end) {
                let end = forced_break
                    .map(|end| {
                        if end == start {
                            next_char_boundary(text, start)
                        } else {
                            end
                        }
                    })
                    .unwrap_or_else(|| trim_trailing_whitespace(text, break_start, start));
                ranges.push(start..end);
                start = end;
            } else {
                ranges.push(start..trim_trailing_whitespace(text, break_start, start));
                start = skip_breaking_whitespace(text, break_end);
            }
        } else if let Some(end) = forced_break {
            let end = if end == start {
                next_char_boundary(text, start)
            } else {
                end
            };
            ranges.push(start..end);
            start = end;
        } else {
            ranges.push(start..trim_trailing_whitespace(text, text.len(), start));
            break;
        }

        width = subsequent_width.max(1);
        first_segment = false;
    }

    ranges
}

fn is_leading_list_marker_break(
    text: &str,
    start: usize,
    break_start: usize,
    break_end: usize,
) -> bool {
    if break_end <= start || break_end > text.len() {
        return false;
    }

    let prefix = &text[start..break_end];
    if !prefix.ends_with(' ') {
        return false;
    }

    let marker = prefix.trim_end().trim_start();
    let is_marker = matches!(marker, "-" | "*" | "+")
        || marker.strip_suffix('.').is_some_and(|digits| {
            !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
        });

    is_marker && break_start + 1 == break_end
}

fn skip_breaking_whitespace(text: &str, mut byte_idx: usize) -> usize {
    while byte_idx < text.len() {
        let Some(ch) = text[byte_idx..].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        byte_idx += ch.len_utf8();
    }
    byte_idx
}

fn trim_trailing_whitespace(text: &str, end: usize, floor: usize) -> usize {
    let mut trimmed = end;
    while trimmed > floor {
        let Some((idx, ch)) = text[..trimmed].char_indices().next_back() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        trimmed = idx;
    }
    trimmed
}

fn next_char_boundary(text: &str, byte_idx: usize) -> usize {
    text[byte_idx..]
        .chars()
        .next()
        .map(|ch| byte_idx + ch.len_utf8())
        .unwrap_or(byte_idx)
}

fn line_with_indent(indent: &Line<'_>, style: Style) -> Line<'static> {
    let mut spans = clone_spans(&indent.spans, style);
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    Line {
        spans,
        style,
        alignment: None,
    }
}

fn line_from_range(
    original: &Line<'_>,
    span_bounds: &[(Range<usize>, Style)],
    range: &Range<usize>,
    indent: &Line<'_>,
) -> Line<'static> {
    let mut spans = clone_spans(&indent.spans, original.style);

    for (idx, (span_range, span_style)) in span_bounds.iter().enumerate() {
        if span_range.end <= range.start {
            continue;
        }
        if span_range.start >= range.end {
            break;
        }

        let seg_start = range.start.max(span_range.start);
        let seg_end = range.end.min(span_range.end);
        if seg_end <= seg_start {
            continue;
        }

        let local_start = seg_start - span_range.start;
        let local_end = seg_end - span_range.start;
        let content = original.spans[idx].content.as_ref();
        spans.push(Span::styled(
            content[local_start..local_end].to_string(),
            merge_non_selectable_marker(original.style, *span_style),
        ));
    }

    Line {
        spans,
        style: original.style,
        alignment: original.alignment,
    }
}

fn clone_spans(spans: &[Span<'_>], base_style: Style) -> Vec<Span<'static>> {
    spans
        .iter()
        .map(|span| {
            let style = merge_non_selectable_marker(base_style, span.style);
            Span::styled(span.content.as_ref().to_string(), style)
        })
        .collect()
}

fn merge_non_selectable_marker(base_style: Style, span_style: Style) -> Style {
    let mut style = base_style.patch(span_style);
    if base_style
        .add_modifier
        .contains(NON_SELECTABLE_SPAN_MODIFIER)
        || span_style
            .add_modifier
            .contains(NON_SELECTABLE_SPAN_MODIFIER)
    {
        style.add_modifier.insert(NON_SELECTABLE_SPAN_MODIFIER);
        style.sub_modifier.remove(NON_SELECTABLE_SPAN_MODIFIER);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn wraps_and_preserves_span_styles() {
        let line = Line::from(vec![
            Span::styled("hello ", Style::default().fg(Color::Red)),
            Span::styled(
                "world again",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        let wrapped = wrap_styled_line(&line, 8);

        assert_eq!(wrapped.len(), 3);
        assert_eq!(line_text(&wrapped[0]), "hello");
        assert_eq!(wrapped[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(line_text(&wrapped[1]), "world");
        assert_eq!(wrapped[1].spans[0].style.fg, Some(Color::Blue));
        assert!(wrapped[1].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
    }

    #[test]
    fn uses_subsequent_indent_for_wrapped_segments() {
        let line = Line::from("one two three four");
        let wrapped = wrap_styled_line(
            &line,
            WrapOptions::new(10).subsequent_indent(Line::from("  ")),
        );

        assert_eq!(line_text(&wrapped[0]), "one two");
        assert_eq!(line_text(&wrapped[1]), "  three");
        assert_eq!(line_text(&wrapped[2]), "  four");
    }

    #[test]
    fn keeps_ordered_list_marker_with_first_word_when_wrapping() {
        let wrapped = wrap_styled_line(&Line::from("1. Replaced old indicator"), 10);

        assert_eq!(line_text(&wrapped[0]), "1. Replace");
        assert_ne!(line_text(&wrapped[0]), "1.");
    }

    #[test]
    fn keeps_unordered_list_marker_with_first_word_when_wrapping() {
        let wrapped = wrap_styled_line(&Line::from("- Replaced old indicator"), 8);

        assert_eq!(line_text(&wrapped[0]), "- Replac");
        assert_ne!(line_text(&wrapped[0]), "-");
    }

    #[test]
    fn wraps_unicode_on_char_boundaries() {
        let line = Line::from("cool 😄 emoji wraps");
        let wrapped = wrap_styled_line(&line, 8);

        assert_eq!(line_text(&wrapped[0]), "cool 😄");
        assert_eq!(line_text(&wrapped[1]), "emoji");
        assert_eq!(line_text(&wrapped[2]), "wraps");
    }

    #[test]
    fn turns_hard_breaks_into_separate_lines() {
        let command = "python3 <<'PY'\n\tfrom pathlib import Path\r\n\t\tprint('ok')\rPY";
        let line = Line::from(vec![
            Span::styled("⬢ Ran ", Style::default().fg(Color::Green)),
            Span::styled(command, Style::default().fg(Color::Blue)),
        ]);

        let wrapped = wrap_styled_line(
            &line,
            WrapOptions::new(200).subsequent_indent(Line::from("  ")),
        );

        assert_eq!(
            wrapped.iter().map(line_text).collect::<Vec<_>>(),
            vec![
                "⬢ Ran python3 <<'PY'",
                "      from pathlib import Path",
                "          print('ok')",
                "  PY",
            ]
        );
        assert!(wrapped.iter().all(|line| line
            .spans
            .iter()
            .all(|span| !span.content.contains(['\n', '\r']))));
        assert!(wrapped
            .iter()
            .all(|line| line.spans.iter().all(|span| !span.content.contains('\t'))));
    }

    #[test]
    fn expands_tabs_using_display_columns_across_spans() {
        let line = Line::from(vec![Span::raw("ab"), Span::raw("\tcd\u{1b}x")]);

        let wrapped = wrap_styled_line(&line, 80);

        assert_eq!(line_text(&wrapped[0]), "ab  cd�x");
        assert!(wrapped[0]
            .spans
            .iter()
            .all(|span| span.content.chars().all(|ch| !ch.is_control())));
    }

    #[test]
    fn final_line_sanitization_preserves_measured_width() {
        let line = Line::from(Span::raw("ab\tcd\u{1b}x"));

        let sanitized = sanitize_styled_line(&line);

        assert_eq!(line_text(&sanitized), "ab cd x");
        assert_eq!(line.width(), sanitized.width());
        assert!(sanitized
            .spans
            .iter()
            .all(|span| span.content.chars().all(|ch| !ch.is_control())));
    }

    #[test]
    fn preserves_blank_lines_without_embedding_control_characters() {
        let line = Line::from(Span::styled(
            "first\n\nlast\n".to_string(),
            Style::default(),
        ));
        let wrapped = wrap_styled_line(&line, 80);

        assert_eq!(
            wrapped.iter().map(line_text).collect::<Vec<_>>(),
            vec!["first", "", "last", ""]
        );
        assert!(wrapped.iter().all(|line| line
            .spans
            .iter()
            .all(|span| !span.content.contains(['\n', '\r']))));
    }
}
