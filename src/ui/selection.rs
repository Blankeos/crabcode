use crate::theme::contrast_text;
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

/// Internal marker for spans that should render normally but be ignored by
/// selection highlighting and clipboard extraction (for example diff gutters).
pub const NON_SELECTABLE_SPAN_MODIFIER: Modifier = Modifier::HIDDEN;

pub fn non_selectable_style(mut style: Style) -> Style {
    style.add_modifier.insert(NON_SELECTABLE_SPAN_MODIFIER);
    style.sub_modifier.remove(NON_SELECTABLE_SPAN_MODIFIER);
    style
}

fn is_selectable_span(span: &Span<'_>) -> bool {
    !span
        .style
        .add_modifier
        .contains(NON_SELECTABLE_SPAN_MODIFIER)
}

fn visible_style(mut style: Style) -> Style {
    style.add_modifier.remove(NON_SELECTABLE_SPAN_MODIFIER);
    style.sub_modifier.remove(NON_SELECTABLE_SPAN_MODIFIER);
    style
}

fn visible_span<'a>(span: Span<'a>) -> Span<'a> {
    Span::styled(span.content, visible_style(span.style))
}

fn strip_non_selectable_markers<'a>(line: ratatui::text::Line<'a>) -> ratatui::text::Line<'a> {
    let spans = line.spans.into_iter().map(visible_span).collect();
    ratatui::text::Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

/// Coordinates are in rendered-content space (line index, column within line).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub active: bool,
    /// Start position (line, column) in rendered content
    pub start_line: usize,
    pub start_col: usize,
    /// End position (line, column) in rendered content
    pub end_line: usize,
    pub end_col: usize,
    /// Whether the user is currently dragging to extend selection
    pub is_dragging: bool,
    /// Last non-shift click position used as the anchor for shift-click selection
    pub anchor: Option<(usize, usize)>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the selection
    pub fn clear(&mut self) {
        self.active = false;
        self.is_dragging = false;
    }

    /// Reset the selection and forget the click anchor
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Start a new selection at the given rendered-content position
    pub fn start(&mut self, line: usize, col: usize) {
        self.active = true;
        self.is_dragging = true;
        self.start_line = line;
        self.start_col = col;
        self.end_line = line;
        self.end_col = col;
        self.anchor = Some((line, col));
    }

    /// Start a new selection from the last non-shift click anchor.
    pub fn start_from_anchor_to(&mut self, line: usize, col: usize) -> bool {
        let Some((anchor_line, anchor_col)) = self.anchor else {
            return false;
        };

        self.active = true;
        self.is_dragging = true;
        self.start_line = anchor_line;
        self.start_col = anchor_col;
        self.end_line = line;
        self.end_col = col;
        true
    }

    /// Extend selection to the given position during drag
    pub fn extend(&mut self, line: usize, col: usize) {
        if !self.is_dragging {
            return;
        }
        self.end_line = line;
        self.end_col = col;
    }

    /// Finalize selection (mouse up)
    pub fn finish(&mut self) {
        self.is_dragging = false;
        // Normalize so start <= end
        self.normalize();
    }

    /// Normalize selection so start <= end
    fn normalize(&mut self) {
        if self.start_line > self.end_line
            || (self.start_line == self.end_line && self.start_col > self.end_col)
        {
            std::mem::swap(&mut self.start_line, &mut self.end_line);
            std::mem::swap(&mut self.start_col, &mut self.end_col);
        }
    }

    /// Get the normalized range (start_line, start_col) to (end_line, end_col)
    pub fn range(&self) -> ((usize, usize), (usize, usize)) {
        let mut start = (self.start_line, self.start_col);
        let mut end = (self.end_line, self.end_col);
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }
        (start, end)
    }

    /// Check if a position (line, col_start..col_end) overlaps with the selection
    pub fn overlaps(&self, line: usize, col_start: usize, col_end: usize) -> bool {
        if !self.active {
            return false;
        }
        let ((s_line, s_col), (e_line, e_col)) = self.range();

        if line < s_line || line > e_line {
            return false;
        }

        if line == s_line && line == e_line {
            // Same line
            col_end > s_col && col_start < e_col
        } else if line == s_line {
            // Start line
            col_end > s_col
        } else if line == e_line {
            // End line
            col_start < e_col
        } else {
            // Fully between start and end lines
            true
        }
    }

    /// Check if a line is fully selected
    pub fn is_line_fully_selected(&self, line: usize, line_width: usize) -> bool {
        if !self.active {
            return false;
        }
        let ((s_line, s_col), (e_line, e_col)) = self.range();
        if line > s_line && line < e_line {
            return true;
        }
        if line == s_line && line == e_line {
            return s_col == 0 && e_col >= line_width;
        }
        if line == s_line {
            return s_col == 0 && e_line > s_line;
        }
        if line == e_line {
            return s_line < e_line && e_col >= line_width;
        }
        false
    }

    /// Return the selection range within a specific line.
    /// Returns None if the line is not in the selection.
    /// Returns (start_col, end_col) if partially or fully selected.
    pub fn selection_range_in_line(
        &self,
        line: usize,
        line_width: usize,
    ) -> Option<(usize, usize)> {
        if !self.active {
            return None;
        }
        let ((s_line, s_col), (e_line, e_col)) = self.range();

        if line < s_line || line > e_line {
            return None;
        }

        let start = if line == s_line { s_col } else { 0 };
        let end = if line == e_line { e_col } else { line_width };

        if start >= end {
            return None;
        }
        Some((start, end))
    }
}

/// Apply selection styling to a vector of lines. Spans that fall within the
/// selection range get highlighted with the accent color.
pub fn apply_selection_to_lines<'a>(
    lines: Vec<ratatui::text::Line<'a>>,
    selection: &Selection,
    accent: Color,
) -> Vec<ratatui::text::Line<'a>> {
    apply_selection_to_lines_with_offset(lines, selection, accent, 0)
}

/// Apply selection styling to visible lines whose first line starts at
/// `line_offset` in the full rendered transcript.
pub fn apply_selection_to_lines_with_offset<'a>(
    lines: Vec<ratatui::text::Line<'a>>,
    selection: &Selection,
    accent: Color,
    line_offset: usize,
) -> Vec<ratatui::text::Line<'a>> {
    if !selection.active {
        return lines
            .into_iter()
            .map(strip_non_selectable_markers)
            .collect();
    }
    let ((s_line, _s_col), (e_line, _e_col)) = selection.range();

    lines
        .into_iter()
        .enumerate()
        .map(|(visible_idx, line)| {
            let line_idx = line_offset + visible_idx;
            if line_idx < s_line || line_idx > e_line {
                return strip_non_selectable_markers(line);
            }
            let line_width: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let sel_range = selection.selection_range_in_line(line_idx, line_width);

            // If entire line is selected, just style all spans
            if selection.is_line_fully_selected(line_idx, line_width) {
                let styled_spans: Vec<Span> = line
                    .spans
                    .into_iter()
                    .map(|s| {
                        if is_selectable_span(&s) {
                            selection_span_style(&s, accent)
                        } else {
                            visible_span(s)
                        }
                    })
                    .collect();
                return ratatui::text::Line::from(styled_spans);
            }

            // Partial selection: track column position and split spans
            let mut col = 0usize;
            let mut styled_spans = Vec::new();
            for span in line.spans {
                let span_width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
                if is_selectable_span(&span) {
                    let new_spans = split_and_style_span(&span, col, accent, sel_range);
                    styled_spans.extend(new_spans);
                } else {
                    styled_spans.push(visible_span(span));
                }
                col = col.saturating_add(span_width);
            }
            ratatui::text::Line::from(styled_spans)
        })
        .collect()
}

/// Extract the selected text from the rendered content lines.
pub fn extract_selected_text(
    lines: &[ratatui::text::Line<'_>],
    selection: &Selection,
) -> Option<String> {
    if !selection.active {
        return None;
    }
    let ((s_line, _), (e_line, _)) = selection.range();
    let mut result = String::new();

    for (line_idx, line) in lines.iter().enumerate() {
        if line_idx < s_line || line_idx > e_line {
            continue;
        }

        let line_width: usize = line
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let Some((start, end)) = selection.selection_range_in_line(line_idx, line_width) else {
            continue;
        };

        let mut line_part = String::new();
        let mut col = 0usize;
        for span in &line.spans {
            let span_width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
            let span_end = col.saturating_add(span_width);

            if is_selectable_span(span) && start < span_end && end > col {
                let overlap_start = start.saturating_sub(col);
                let overlap_end = end.saturating_sub(col).min(span_width);
                line_part.push_str(slice_by_display_width(
                    span.content.as_ref(),
                    overlap_start,
                    overlap_end,
                ));
            }

            col = span_end;
        }

        if line_part.is_empty() {
            continue;
        }

        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&line_part);
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn slice_by_display_width(text: &str, start: usize, end: usize) -> &str {
    if start >= end {
        return "";
    }

    let mut byte_start = text.len();
    let mut byte_end = text.len();
    let mut display_pos = 0usize;

    for (byte_idx, ch) in text.char_indices() {
        if display_pos >= start && byte_start == text.len() {
            byte_start = byte_idx;
        }
        if display_pos >= end {
            byte_end = byte_idx;
            break;
        }
        display_pos += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
    }

    if byte_start == text.len() && display_pos >= start {
        byte_start = text.len();
    }
    if display_pos < end {
        byte_end = text.len();
    }

    &text[byte_start.min(byte_end)..byte_end]
}

/// Apply a selection highlight style to a span.
/// Uses the accent color as background with inverted text for visibility.
fn selection_span_style<'a>(span: &Span<'a>, accent: Color) -> Span<'a> {
    Span::styled(
        span.content.clone(),
        Style::default()
            .bg(accent)
            .fg(contrast_text(accent))
            .add_modifier(Modifier::BOLD),
    )
}

/// Split a span at a given column offset and apply selection style to the selected portion.
/// Returns a vector of spans (unselected prefix, selected middle, unselected suffix).
fn split_and_style_span<'a>(
    span: &Span<'a>,
    col_offset: usize,
    accent: Color,
    selection_range: Option<(usize, usize)>,
) -> Vec<Span<'a>> {
    let content = span.content.as_ref();
    let width = unicode_width::UnicodeWidthStr::width(content);
    let span_end = col_offset + width;

    let (sel_start, sel_end) = match selection_range {
        Some((s, e)) => (s, e),
        None => return vec![visible_span(span.clone())],
    };

    // Check if this span overlaps with the selection
    if sel_end <= col_offset || sel_start >= span_end {
        return vec![visible_span(span.clone())];
    }

    // Calculate the overlap boundaries in display-width positions relative to the span
    let overlap_start = sel_start.saturating_sub(col_offset);
    let overlap_end = sel_end.saturating_sub(col_offset).min(width);

    if overlap_start >= overlap_end {
        return vec![visible_span(span.clone())];
    }

    // Convert display-width positions back to character indices
    let chars: Vec<char> = content.chars().collect();
    let total_chars = chars.len();

    let mut char_idx = 0;
    let mut display_pos = 0;
    let char_start;

    while char_idx < total_chars && display_pos < overlap_start {
        let c = chars[char_idx];
        display_pos += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        char_idx += 1;
    }
    char_start = char_idx;

    while char_idx < total_chars && display_pos < overlap_end {
        let c = chars[char_idx];
        display_pos += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        char_idx += 1;
    }
    let char_end = char_idx;

    if char_start >= char_end {
        return vec![visible_span(span.clone())];
    }

    let before: String = chars[..char_start].iter().collect();
    let selected: String = chars[char_start..char_end].iter().collect();
    let after: String = chars[char_end..].iter().collect();

    let mut result = Vec::new();

    if !before.is_empty() {
        result.push(Span::styled(before, visible_style(span.style)));
    }

    result.push(Span::styled(
        selected,
        Style::default()
            .bg(accent)
            .fg(contrast_text(accent))
            .add_modifier(Modifier::BOLD),
    ));

    if !after.is_empty() {
        result.push(Span::styled(after, visible_style(span.style)));
    }

    result
}
