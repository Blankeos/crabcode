use crate::theme::{contrast_text, ThemeColors};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeScrollDirection {
    Up,
    Down,
}

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

/// How a rendered row joins to the previous row when copied.
///
/// Soft wraps are a display artifact: the clipboard should keep the source
/// spacing (`joiner`) and drop the terminal width break. `indent_cols` is the
/// visual continuation indent, which is not part of the copied text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyJoin {
    Hard,
    Soft { joiner: String },
    Blank,
}

const SOFT_HINT_PREFIX: &str = "\u{200B}\u{200D}";
const BLANK_HINT: &str = "\u{200B}\u{200C}";
const LINK_HINT_PREFIX: &str = "\u{200B}\u{2060}";

pub fn soft_wrap_hint_span(joiner: &str) -> Span<'static> {
    let mut content = String::from(SOFT_HINT_PREFIX);
    for byte in joiner.bytes() {
        content.push(hint_nibble(byte >> 4));
        content.push(hint_nibble(byte & 0x0f));
    }
    Span::styled(content, non_selectable_style(Style::default()))
}

pub fn blank_line_hint_span() -> Span<'static> {
    Span::styled(
        BLANK_HINT.to_string(),
        non_selectable_style(Style::default()),
    )
}

pub fn is_copy_hint_span(span: &Span<'_>) -> bool {
    let content = span.content.as_ref();
    content.starts_with(SOFT_HINT_PREFIX)
        || content == BLANK_HINT
        || content.starts_with(LINK_HINT_PREFIX)
}

/// Zero-width marker so a table link can be copied as `[label](url)` without
/// drawing the destination in the cell.
pub fn link_dest_hint_span(url: &str) -> Span<'static> {
    let mut content = String::from(LINK_HINT_PREFIX);
    for byte in url.bytes() {
        content.push(hint_nibble(byte >> 4));
        content.push(hint_nibble(byte & 0x0f));
    }
    Span::styled(content, non_selectable_style(Style::default()))
}

pub fn copy_join_of_line(line: &ratatui::text::Line<'_>) -> CopyJoin {
    line.spans
        .iter()
        .find_map(decode_copy_hint)
        .unwrap_or(CopyJoin::Hard)
}

fn hint_nibble(nibble: u8) -> char {
    char::from_u32(0xE0030 + u32::from(nibble)).unwrap_or('\u{2060}')
}

fn decode_nibble(ch: char) -> Option<u8> {
    let raw = ch as u32;
    let nibble = raw.checked_sub(0xE0030)?;
    (nibble < 16).then_some(nibble as u8)
}

fn decode_link_hint(span: &Span<'_>) -> Option<String> {
    let encoded = span.content.as_ref().strip_prefix(LINK_HINT_PREFIX)?;
    decode_hint_bytes(encoded)
}

fn decode_hint_bytes(encoded: &str) -> Option<String> {
    let nibbles = encoded.chars().collect::<Vec<_>>();
    if nibbles.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(nibbles.len() / 2);
    for pair in nibbles.chunks(2) {
        let high = decode_nibble(pair[0])?;
        let low = decode_nibble(pair[1])?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes).ok()
}

fn decode_copy_hint(span: &Span<'_>) -> Option<CopyJoin> {
    if !is_copy_hint_span(span) || decode_link_hint(span).is_some() {
        return None;
    }
    let content = span.content.as_ref();
    if content == BLANK_HINT {
        return Some(CopyJoin::Blank);
    }
    let joiner_hex = content.strip_prefix(SOFT_HINT_PREFIX)?;
    let joiner = decode_hint_bytes(joiner_hex)?;
    Some(CopyJoin::Soft { joiner })
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
    let spans = line
        .spans
        .into_iter()
        .filter(|span| !is_copy_hint_span(span))
        .map(visible_span)
        .collect();
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

        // A wrapped-away blank row has no cells, but a drag that crosses it
        // still covers that row. Keep it so the blank survives the copy.
        if line > s_line && line < e_line {
            return Some((0, line_width));
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
                .filter(|span| !is_copy_hint_span(span))
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
                if is_copy_hint_span(&span) {
                    continue;
                }
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
///
/// Visual wraps are joined with the spacing the wrapper removed. Typed line
/// breaks, including blank lines, stay in the clipboard.
pub fn extract_selected_text(
    lines: &[ratatui::text::Line<'_>],
    selection: &Selection,
) -> Option<String> {
    extract_selected(lines, selection, None)
}

/// Same selection as [`extract_selected_text`], but inline styles that hide
/// their markdown (`**bold**`, `*italic*`, `` `code` ``, `~~strike~~`,
/// `[label](url)`) are written back out.
pub fn extract_selected_markdown(
    lines: &[ratatui::text::Line<'_>],
    selection: &Selection,
    colors: &ThemeColors,
) -> Option<String> {
    extract_selected(lines, selection, Some(colors))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct InlineFmt {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

impl InlineFmt {
    fn wraps(self) -> bool {
        self.bold || self.italic || self.strike || self.code
    }

    fn is_link_label(self) -> bool {
        self.link || self.code
    }

    fn wrap_key(self) -> (bool, bool, bool, bool) {
        (self.bold, self.italic, self.strike, self.code)
    }
}

struct Piece {
    text: String,
    fmt: InlineFmt,
    link_url: Option<String>,
}

fn inline_fmt(style: Style, colors: &ThemeColors) -> InlineFmt {
    let mods = style.add_modifier;
    if mods.contains(NON_SELECTABLE_SPAN_MODIFIER) {
        return InlineFmt::default();
    }
    let fg = style.fg;
    let link =
        mods.contains(Modifier::UNDERLINED) && (fg == Some(colors.markdown_link) || fg.is_none());
    // Code blocks share the element background, but use `markdown_code_block`.
    // Inline code uses `markdown_code`. When a theme (or a test fixture) makes
    // those colors identical, skip the markers instead of backtick-wrapping
    // fences and user bubbles.
    let code = colors.markdown_code != colors.markdown_code_block
        && colors.markdown_code != colors.text
        && style.bg == Some(colors.background_element)
        && fg == Some(colors.markdown_code);
    let bold = mods.contains(Modifier::BOLD)
        && (fg == Some(colors.markdown_strong)
            || (link && mods.contains(Modifier::BOLD))
            || fg == Some(colors.markdown_block_quote));
    let italic = mods.contains(Modifier::ITALIC)
        && (fg == Some(colors.markdown_emph) || fg == Some(colors.markdown_strong) || link);
    let strike = mods.contains(Modifier::CROSSED_OUT)
        && !mods.contains(Modifier::DIM)
        && matches!(
            fg,
            Some(color)
                if color == colors.markdown_text
                    || color == colors.markdown_strong
                    || color == colors.markdown_emph
                    || color == colors.markdown_link
        );
    InlineFmt {
        bold,
        italic,
        strike,
        code,
        link,
    }
}

fn extract_selected(
    lines: &[ratatui::text::Line<'_>],
    selection: &Selection,
    colors: Option<&ThemeColors>,
) -> Option<String> {
    if !selection.active {
        return None;
    }
    let ((s_line, _), (e_line, _)) = selection.range();
    let mut logical: Vec<Vec<Piece>> = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    let mut started = false;
    let mut previous_line: Option<usize> = None;

    for (line_idx, line) in lines.iter().enumerate() {
        if line_idx < s_line || line_idx > e_line {
            continue;
        }

        let join = copy_join_of_line(line);
        let line_width: usize = line
            .spans
            .iter()
            .filter(|span| !is_copy_hint_span(span))
            .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
            .sum();
        let Some((start, end)) = selection.selection_range_in_line(line_idx, line_width) else {
            continue;
        };

        let mut pieces = Vec::new();
        let mut col = 0usize;
        for span in &line.spans {
            if let Some(url) = decode_link_hint(span) {
                if colors.is_some() && start <= col && end >= col && start < end {
                    pieces.push(Piece {
                        text: String::new(),
                        fmt: InlineFmt::default(),
                        link_url: Some(url),
                    });
                }
                continue;
            }
            if is_copy_hint_span(span) {
                continue;
            }
            let span_width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
            let span_end = col.saturating_add(span_width);
            if is_selectable_span(span) && start < span_end && end > col {
                let overlap_start = start.saturating_sub(col);
                let overlap_end = end.saturating_sub(col).min(span_width);
                let text =
                    slice_by_display_width(span.content.as_ref(), overlap_start, overlap_end)
                        .to_string();
                if !text.is_empty() {
                    pieces.push(Piece {
                        text,
                        fmt: colors
                            .map(|colors| inline_fmt(span.style, colors))
                            .unwrap_or_default(),
                        link_url: None,
                    });
                }
            }
            col = span_end;
        }

        // Blank logical lines have no selectable glyphs. Keep them when the
        // drag covers the row; skip every other empty row (gutters, spacers).
        if pieces
            .iter()
            .all(|piece| piece.text.is_empty() && piece.link_url.is_none())
            && !matches!(join, CopyJoin::Blank)
        {
            continue;
        }

        if started {
            let consecutive = previous_line.is_some_and(|previous| line_idx == previous + 1);
            if consecutive {
                if let CopyJoin::Soft { joiner } = &join {
                    let fmt = if current.last().map(|piece| piece.fmt)
                        == pieces.first().map(|piece| piece.fmt)
                    {
                        current.last().map(|piece| piece.fmt).unwrap_or_default()
                    } else {
                        InlineFmt::default()
                    };
                    if !joiner.is_empty() {
                        current.push(Piece {
                            text: joiner.clone(),
                            fmt,
                            link_url: None,
                        });
                    }
                    current.extend(pieces);
                } else {
                    logical.push(std::mem::take(&mut current));
                    current = pieces;
                }
            } else {
                logical.push(std::mem::take(&mut current));
                current = pieces;
            }
        } else {
            current = pieces;
            started = true;
        }
        previous_line = Some(line_idx);
    }

    if started {
        logical.push(current);
    }

    let markdown = colors.is_some();
    let result = logical
        .into_iter()
        .map(|pieces| format_logical_line(pieces, markdown))
        .collect::<Vec<_>>()
        .join("\n");

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn format_logical_line(pieces: Vec<Piece>, markdown: bool) -> String {
    if !markdown {
        return pieces.into_iter().map(|piece| piece.text).collect();
    }
    apply_visible_links(resolve_link_hints(pieces))
}

fn resolve_link_hints(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out = Vec::new();
    for piece in pieces {
        if let Some(url) = piece.link_url {
            let start = trailing_label_start(&out);
            let label_pieces = out.split_off(start);
            let label = format_inline_pieces(&label_pieces);
            out.push(Piece {
                text: format!("[{label}]({url})"),
                fmt: InlineFmt::default(),
                link_url: None,
            });
            continue;
        }
        if piece.text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.fmt == piece.fmt {
                last.text.push_str(&piece.text);
                continue;
            }
        }
        out.push(piece);
    }
    out
}

fn trailing_label_start(pieces: &[Piece]) -> usize {
    let mut start = pieces.len();
    while start > 0 && pieces[start - 1].fmt.is_link_label() {
        start -= 1;
    }
    start
}

fn format_inline_pieces(pieces: &[Piece]) -> String {
    let (text, runs) = flatten_pieces(pieces);
    format_range(&text, &runs, 0, text.len())
}

fn apply_visible_links(pieces: Vec<Piece>) -> String {
    let (text, runs) = flatten_pieces(&pieces);
    let links = find_visible_links(&text, &runs);
    if links.is_empty() {
        return format_range(&text, &runs, 0, text.len());
    }
    let mut out = String::new();
    let mut cursor = 0usize;
    for link in links {
        out.push_str(&format_range(&text, &runs, cursor, link.label_start));
        out.push('[');
        out.push_str(&format_range(
            &text,
            &runs,
            link.label_start,
            link.label_end,
        ));
        out.push_str("](");
        out.push_str(&text[link.url_start..link.url_end]);
        out.push(')');
        cursor = link.paren + 1;
    }
    out.push_str(&format_range(&text, &runs, cursor, text.len()));
    out
}

struct Run {
    start: usize,
    end: usize,
    fmt: InlineFmt,
}

struct VisibleLink {
    label_start: usize,
    label_end: usize,
    url_start: usize,
    url_end: usize,
    paren: usize,
}

fn flatten_pieces(pieces: &[Piece]) -> (String, Vec<Run>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    for piece in pieces {
        if piece.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&piece.text);
        runs.push(Run {
            start,
            end: text.len(),
            fmt: piece.fmt,
        });
    }
    (text, runs)
}

fn find_visible_links(text: &str, runs: &[Run]) -> Vec<VisibleLink> {
    let mut links = Vec::new();
    let mut from = 0usize;
    while from < text.len() {
        let Some(rel) = text[from..].find(" (") else {
            break;
        };
        let open = from + rel;
        let url_start = open + 2;
        if !range_is_plain(text, runs, open, url_start) {
            from = open + 1;
            continue;
        }
        let mut url_end = url_start;
        while url_end < text.len() && byte_is(runs, url_end, |fmt| fmt.link) {
            url_end = next_char_end(text, url_end);
        }
        if url_end == url_start
            || !text[url_end..].starts_with(')')
            || text[url_start..url_end].chars().any(char::is_whitespace)
        {
            from = open + 1;
            continue;
        }
        let mut label_start = open;
        while label_start > from && byte_is(runs, label_start - 1, |fmt| fmt.is_link_label()) {
            label_start = prev_char_start(text, label_start);
        }
        let label = &text[label_start..open];
        if label.trim().is_empty() || !label_has_link_or_code(runs, label_start, open) {
            from = open + 1;
            continue;
        }
        links.push(VisibleLink {
            label_start,
            label_end: open,
            url_start,
            url_end,
            paren: url_end,
        });
        from = url_end + 1;
    }
    links
}

fn label_has_link_or_code(runs: &[Run], start: usize, end: usize) -> bool {
    runs.iter()
        .any(|run| run.end > start && run.start < end && (run.fmt.link || run.fmt.code))
}

fn range_is_plain(text: &str, runs: &[Run], start: usize, end: usize) -> bool {
    let mut idx = start;
    while idx < end {
        if !byte_is(runs, idx, |fmt| !fmt.link && !fmt.code) {
            return false;
        }
        idx = next_char_end(text, idx);
    }
    true
}

fn byte_is(runs: &[Run], byte_idx: usize, pred: impl Fn(InlineFmt) -> bool) -> bool {
    runs.iter()
        .find(|run| byte_idx >= run.start && byte_idx < run.end)
        .is_some_and(|run| pred(run.fmt))
}

fn prev_char_start(text: &str, idx: usize) -> usize {
    let mut prev = idx.saturating_sub(1);
    while prev > 0 && !text.is_char_boundary(prev) {
        prev -= 1;
    }
    prev
}

fn next_char_end(text: &str, idx: usize) -> usize {
    text[idx..]
        .chars()
        .next()
        .map(|ch| idx + ch.len_utf8())
        .unwrap_or(idx + 1)
}

fn format_range(text: &str, runs: &[Run], start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut idx = start;
    while idx < end {
        let fmt = fmt_at(runs, idx);
        let mut next = next_char_end(text, idx);
        if next <= idx {
            break;
        }
        while next < end && fmt_at(runs, next).wrap_key() == fmt.wrap_key() {
            let stepped = next_char_end(text, next);
            if stepped <= next {
                break;
            }
            next = stepped;
        }
        out.push_str(&wrap_inline(&text[idx..next.min(end)], fmt));
        idx = next;
    }
    out
}

fn fmt_at(runs: &[Run], byte_idx: usize) -> InlineFmt {
    runs.iter()
        .find(|run| byte_idx >= run.start && byte_idx < run.end)
        .map(|run| run.fmt)
        .unwrap_or_default()
}

fn wrap_inline(text: &str, fmt: InlineFmt) -> String {
    if !fmt.wraps() || text.trim().is_empty() {
        return text.to_string();
    }
    let mut wrapped = if fmt.code {
        wrap_code(text)
    } else {
        text.to_string()
    };
    if fmt.bold || fmt.italic {
        let marker = match (fmt.bold, fmt.italic) {
            (true, true) => "***",
            (true, false) => "**",
            _ => "*",
        };
        wrapped = format!("{marker}{wrapped}{marker}");
    }
    if fmt.strike {
        wrapped = format!("~~{wrapped}~~");
    }
    wrapped
}

fn wrap_code(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
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
