use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, text::Line};
use std::path::PathBuf;
use std::sync::LazyLock;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use url::Url;

static LOCATION_SUFFIX_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r":\d+(?::\d+)?(?:-\d+(?::\d+)?)?$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperlinkTarget {
    Url(String),
    File(FileHyperlinkTarget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHyperlinkTarget {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperlinkRange {
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
    pub target: HyperlinkTarget,
}

/// A hyperlink span mapped onto one wrapped display line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperlinkLineRange {
    pub line_idx: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// Mark URL-like and local-path-like text in rendered buffer cells.
pub fn mark_detected_hyperlinks(buf: &mut Buffer, area: Rect, lines: &[Line<'_>]) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    for (line_idx, line) in lines.iter().take(area.height as usize).enumerate() {
        let text = line_to_string(line);
        let ranges = detect_hyperlinks(&text);
        let y = area.y.saturating_add(line_idx as u16);

        for range in ranges {
            mark_range(buf, area, y, &range);
        }
    }
}

pub fn mark_hyperlink_range(buf: &mut Buffer, area: Rect, line_idx: usize, range: &HyperlinkRange) {
    if line_idx >= area.height as usize {
        return;
    }

    let y = area.y.saturating_add(line_idx as u16);
    mark_range(buf, area, y, range);
}

pub fn mark_hyperlink_line_range(
    buf: &mut Buffer,
    area: Rect,
    line_idx: usize,
    start_col: usize,
    end_col: usize,
) {
    mark_hyperlink_range(
        buf,
        area,
        line_idx,
        &HyperlinkRange {
            start_col,
            end_col,
            text: String::new(),
            target: HyperlinkTarget::Url(String::new()),
        },
    );
}

pub fn hyperlink_at_line_col(line: &Line<'_>, col: usize) -> Option<HyperlinkTarget> {
    hyperlink_range_at_line_col(line, col).map(|range| range.target)
}

pub fn hyperlink_range_at_line_col(line: &Line<'_>, col: usize) -> Option<HyperlinkRange> {
    let text = line_to_string(line);
    detect_hyperlinks(&text)
        .into_iter()
        .find(|range| col >= range.start_col && col < range.end_col)
}

/// Resolve a hyperlink at `(line_idx, col)` while reconstructing paths/URLs that
/// were hard-wrapped across adjacent lines.
///
/// Terminal wrapping can split `/Users/.../file.md` mid-token. Per-line detection
/// then sees incomplete fragments that are not valid targets. This joins the wrap
/// group, detects on the reconstructed text, and maps the match back onto the
/// clicked line for underlining.
pub fn hyperlink_range_at_wrapped_lines(
    lines: &[Line<'_>],
    line_idx: usize,
    col: usize,
    wrap_width: usize,
) -> Option<HyperlinkRange> {
    if wrap_width == 0 || line_idx >= lines.len() {
        return hyperlink_range_at_line_col(lines.get(line_idx)?, col);
    }

    let (group_start, joined, line_spans) = stitch_wrap_group(lines, line_idx, wrap_width);
    let local = line_spans.get(line_idx - group_start)?;
    let prefix_cols = UnicodeWidthStr::width(&joined[..local.start]);
    let line_text = line_to_string(&lines[line_idx]);
    let stripped = local.stripped_prefix_cols;
    if col < stripped {
        return None;
    }
    let content_col = col - stripped;
    let content_start = byte_index_at_display_col(&line_text, stripped);
    let line_content_width = UnicodeWidthStr::width(&line_text[content_start..]);
    if content_col >= line_content_width {
        return None;
    }
    let joined_col = prefix_cols + content_col;

    let Some(full_range) = detect_hyperlinks(&joined)
        .into_iter()
        .find(|range| joined_col >= range.start_col && joined_col < range.end_col)
    else {
        return hyperlink_range_at_line_col(&lines[line_idx], col);
    };

    let link_start = byte_index_at_display_col(&joined, full_range.start_col);
    let link_end = byte_index_at_display_col(&joined, full_range.end_col);
    let overlap_start = link_start.max(local.start);
    let overlap_end = link_end.min(local.end);
    if overlap_start >= overlap_end {
        return Some(full_range);
    }

    let start_col = stripped + UnicodeWidthStr::width(&joined[local.start..overlap_start]);
    let end_col = stripped + UnicodeWidthStr::width(&joined[local.start..overlap_end]);

    Some(HyperlinkRange {
        start_col,
        end_col,
        text: full_range.text,
        target: full_range.target,
    })
}

pub fn hyperlink_at_wrapped_lines(
    lines: &[Line<'_>],
    line_idx: usize,
    col: usize,
    wrap_width: usize,
) -> Option<HyperlinkTarget> {
    hyperlink_range_at_wrapped_lines(lines, line_idx, col, wrap_width).map(|range| range.target)
}

/// Like [`hyperlink_range_at_wrapped_lines`], but returns underline ranges for
/// every wrap segment that participates in the matched link.
pub fn hyperlink_segments_at_wrapped_lines(
    lines: &[Line<'_>],
    line_idx: usize,
    col: usize,
    wrap_width: usize,
) -> Option<(HyperlinkRange, Vec<HyperlinkLineRange>)> {
    if wrap_width == 0 || line_idx >= lines.len() {
        let range = hyperlink_range_at_line_col(lines.get(line_idx)?, col)?;
        let segments = vec![HyperlinkLineRange {
            line_idx,
            start_col: range.start_col,
            end_col: range.end_col,
        }];
        return Some((range, segments));
    }

    let (group_start, joined, line_spans) = stitch_wrap_group(lines, line_idx, wrap_width);
    let local = line_spans.get(line_idx - group_start)?;
    let prefix_cols = UnicodeWidthStr::width(&joined[..local.start]);
    let line_text = line_to_string(&lines[line_idx]);
    let stripped = local.stripped_prefix_cols;
    if col < stripped {
        return None;
    }
    let content_col = col - stripped;
    let content_start = byte_index_at_display_col(&line_text, stripped);
    let line_content_width = UnicodeWidthStr::width(&line_text[content_start..]);
    if content_col >= line_content_width {
        return None;
    }
    let joined_col = prefix_cols + content_col;

    let Some(full_range) = detect_hyperlinks(&joined)
        .into_iter()
        .find(|range| joined_col >= range.start_col && joined_col < range.end_col)
    else {
        let range = hyperlink_range_at_line_col(&lines[line_idx], col)?;
        let segments = vec![HyperlinkLineRange {
            line_idx,
            start_col: range.start_col,
            end_col: range.end_col,
        }];
        return Some((range, segments));
    };

    let link_start = byte_index_at_display_col(&joined, full_range.start_col);
    let link_end = byte_index_at_display_col(&joined, full_range.end_col);

    let mut segments = Vec::new();
    for (offset, span) in line_spans.iter().enumerate() {
        let overlap_start = link_start.max(span.start);
        let overlap_end = link_end.min(span.end);
        if overlap_start >= overlap_end {
            continue;
        }
        let start_col =
            span.stripped_prefix_cols + UnicodeWidthStr::width(&joined[span.start..overlap_start]);
        let end_col =
            span.stripped_prefix_cols + UnicodeWidthStr::width(&joined[span.start..overlap_end]);
        if start_col < end_col {
            segments.push(HyperlinkLineRange {
                line_idx: group_start + offset,
                start_col,
                end_col,
            });
        }
    }

    let clicked = segments
        .iter()
        .find(|seg| seg.line_idx == line_idx)
        .cloned()
        .or_else(|| segments.first().cloned())?;

    Some((
        HyperlinkRange {
            start_col: clicked.start_col,
            end_col: clicked.end_col,
            text: full_range.text,
            target: full_range.target,
        },
        segments,
    ))
}

fn mark_range(buf: &mut Buffer, area: Rect, y: u16, range: &HyperlinkRange) {
    if range.start_col >= range.end_col {
        return;
    }

    let start = range.start_col.min(area.width as usize);
    let end = range.end_col.min(area.width as usize);

    for col in start..end {
        let x = area.x.saturating_add(col as u16);
        let cell = &mut buf[(x, y)];
        let symbol = cell.symbol().to_string();
        if symbol.trim().is_empty() {
            continue;
        }

        cell.modifier.insert(Modifier::UNDERLINED);
    }
}

#[derive(Debug, Clone, Copy)]
struct WrapLineSpan {
    start: usize,
    end: usize,
    /// Display columns stripped from the start of this line when joining
    /// (leading indent on continuation wrap segments).
    stripped_prefix_cols: usize,
}

fn stitch_wrap_group(
    lines: &[Line<'_>],
    line_idx: usize,
    wrap_width: usize,
) -> (usize, String, Vec<WrapLineSpan>) {
    let group_start = find_wrap_group_start(lines, line_idx, wrap_width);
    let group_end = find_wrap_group_end(lines, line_idx, wrap_width);

    let mut joined = String::new();
    let mut spans = Vec::with_capacity(group_end - group_start);
    for idx in group_start..group_end {
        let text = line_to_string(&lines[idx]);
        let is_continuation = idx > group_start;
        let (content, stripped_prefix_cols) = if is_continuation {
            strip_leading_indent_for_join(&text)
        } else {
            (text.as_str(), 0)
        };
        let start = joined.len();
        joined.push_str(content);
        spans.push(WrapLineSpan {
            start,
            end: joined.len(),
            stripped_prefix_cols,
        });
    }
    (group_start, joined, spans)
}

fn find_wrap_group_start(lines: &[Line<'_>], line_idx: usize, wrap_width: usize) -> usize {
    let mut start = line_idx;
    while start > 0 && should_join_wrapped_lines(&lines[start - 1], &lines[start], wrap_width) {
        start -= 1;
    }
    start
}

fn find_wrap_group_end(lines: &[Line<'_>], line_idx: usize, wrap_width: usize) -> usize {
    let mut end = line_idx + 1;
    while end < lines.len() && should_join_wrapped_lines(&lines[end - 1], &lines[end], wrap_width) {
        end += 1;
    }
    end
}

fn should_join_wrapped_lines(prev: &Line<'_>, next: &Line<'_>, wrap_width: usize) -> bool {
    let prev_text = line_to_string(prev);
    if line_fills_wrap_width(&prev_text, wrap_width) {
        return true;
    }

    // Soft wraps after `/` leave the previous line under width.
    let prev_trim = prev_text.trim_end();
    if !prev_trim.ends_with('/') {
        return false;
    }

    let next_text = line_to_string(next);
    let (next_content, _) = strip_leading_indent_for_join(&next_text);
    next_content
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '~'))
}

fn line_fills_wrap_width(text: &str, wrap_width: usize) -> bool {
    UnicodeWidthStr::width(text) >= wrap_width
}

fn strip_leading_indent_for_join(text: &str) -> (&str, usize) {
    let trimmed = text.trim_start_matches(' ');
    let stripped = &text[..text.len() - trimmed.len()];
    (trimmed, UnicodeWidthStr::width(stripped))
}

fn byte_index_at_display_col(text: &str, col: usize) -> usize {
    if col == 0 {
        return 0;
    }
    let mut width = 0usize;
    for (idx, ch) in text.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > col {
            return idx;
        }
        width += ch_width;
        if width == col {
            return idx + ch.len_utf8();
        }
    }
    text.len()
}

fn detect_hyperlinks(text: &str) -> Vec<HyperlinkRange> {
    candidate_tokens(text)
        .filter_map(|(start, end, token)| {
            let target = hyperlink_target_for_token(token)?;
            Some(HyperlinkRange {
                start_col: UnicodeWidthStr::width(&text[..start]),
                end_col: UnicodeWidthStr::width(&text[..end]),
                text: token.to_string(),
                target,
            })
        })
        .collect()
}

fn candidate_tokens(text: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut raw_tokens = Vec::new();
    let mut start = None;

    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(token_start) = start.take() {
                raw_tokens.push((token_start, idx));
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }

    if let Some(token_start) = start {
        raw_tokens.push((token_start, text.len()));
    }

    raw_tokens.into_iter().filter_map(move |(start, end)| {
        let (start, end) = trim_token_bounds(text, start, end);
        (start < end).then(|| (start, end, &text[start..end]))
    })
}

fn trim_token_bounds(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end {
        let Some(ch) = text[start..end].chars().next() else {
            break;
        };
        if is_token_prefix_delimiter(ch) {
            start += ch.len_utf8();
        } else {
            break;
        }
    }

    while start < end {
        let Some(ch) = text[start..end].chars().next_back() else {
            break;
        };
        if is_token_suffix_delimiter(ch) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }

    (start, end)
}

fn is_token_prefix_delimiter(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '`' | '(' | '[' | '{' | '<')
}

fn is_token_suffix_delimiter(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '`' | ')' | ']' | '}' | '>' | ',' | ';' | ':' | '.' | '!' | '?'
    )
}

fn hyperlink_target_for_token(token: &str) -> Option<HyperlinkTarget> {
    if token.starts_with("http://") || token.starts_with("https://") {
        return Some(HyperlinkTarget::Url(token.to_string()));
    }

    if token.starts_with("file://") {
        return file_target_for_file_url_token(token).map(HyperlinkTarget::File);
    }

    file_target_for_local_path_token(token).map(HyperlinkTarget::File)
}

fn file_target_for_file_url_token(token: &str) -> Option<FileHyperlinkTarget> {
    let url = Url::parse(token).ok()?;
    let path = url.to_file_path().ok()?;
    let path_text = path.to_string_lossy();
    let location = split_location_suffix(&path_text);
    Some(FileHyperlinkTarget {
        path: PathBuf::from(location.path),
        line: location.line,
        column: location.column,
    })
}

fn file_target_for_local_path_token(token: &str) -> Option<FileHyperlinkTarget> {
    let location = split_location_suffix(token);
    if !is_local_path_like(location.path) {
        return None;
    }

    expand_local_path(location.path).map(|path| FileHyperlinkTarget {
        path,
        line: location.line,
        column: location.column,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathLocationSuffix<'a> {
    path: &'a str,
    line: Option<usize>,
    column: Option<usize>,
}

fn split_location_suffix(token: &str) -> PathLocationSuffix<'_> {
    let without_hash = token
        .rsplit_once('#')
        .filter(|(_, fragment)| is_hash_location_suffix(fragment))
        .map(|(path, fragment)| {
            let (line, column) = parse_hash_location_suffix(fragment);
            (path, line, column)
        });

    if let Some((path, line, column)) = without_hash {
        return PathLocationSuffix { path, line, column };
    }

    let Some(matched) = LOCATION_SUFFIX_RE
        .find(token)
        .filter(|matched| matched.end() == token.len())
    else {
        return PathLocationSuffix {
            path: token,
            line: None,
            column: None,
        };
    };

    let suffix = &token[matched.start() + 1..matched.end()];
    let (line, column) = parse_colon_location_suffix(suffix);
    PathLocationSuffix {
        path: &token[..matched.start()],
        line,
        column,
    }
}

fn parse_colon_location_suffix(suffix: &str) -> (Option<usize>, Option<usize>) {
    let mut parts = suffix.split(':');
    let line = parts.next().and_then(parse_location_number);
    let column = parts.next().and_then(parse_location_number);
    (line, column)
}

fn parse_hash_location_suffix(fragment: &str) -> (Option<usize>, Option<usize>) {
    let Some(after_l) = fragment.strip_prefix('L') else {
        return (None, None);
    };
    let (line_text, column_text) = after_l
        .split_once("C")
        .map(|(line, column)| (line, Some(column)))
        .unwrap_or((after_l, None));
    let line_text = line_text
        .split_once('-')
        .map(|(line, _)| line)
        .unwrap_or(line_text);
    let line = parse_location_number(line_text);
    let column = column_text.and_then(parse_location_number);
    (line, column)
}

fn parse_location_number(value: &str) -> Option<usize> {
    value.parse::<usize>().ok().filter(|number| *number > 0)
}

fn is_hash_location_suffix(fragment: &str) -> bool {
    let mut chars = fragment.chars();
    matches!(chars.next(), Some('L'))
        && chars.any(|ch| ch.is_ascii_digit())
        && fragment
            .chars()
            .all(|ch| ch == 'L' || ch == 'C' || ch == '-' || ch.is_ascii_digit())
}

fn expand_local_path(path_text: &str) -> Option<PathBuf> {
    if let Some(rest) = path_text.strip_prefix("~/") {
        return dirs::home_dir().map(|home| home.join(rest));
    }

    let path = PathBuf::from(path_text);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(path))
    }
}

fn is_local_path_like(path_text: &str) -> bool {
    if path_text.is_empty()
        || path_text.contains("://")
        || path_text.chars().any(is_forbidden_path_char)
    {
        return false;
    }

    if path_text.starts_with('/') {
        return is_absolute_path_like(path_text);
    }

    if path_text.starts_with("~/") || path_text.starts_with("./") || path_text.starts_with("../") {
        return path_text.len() > 1;
    }

    if path_text.contains('/') {
        return is_relative_slash_path(path_text);
    }

    is_known_local_filename(path_text) || has_known_extension(path_text)
}

fn is_forbidden_path_char(ch: char) -> bool {
    matches!(ch, '=' | '|' | '*' | '?' | '<' | '>' | '@')
}

fn is_absolute_path_like(path_text: &str) -> bool {
    let segments = path_text
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.len() < 2 && !std::path::Path::new(path_text).exists() {
        return false;
    }

    // Don't mark directories as clickable
    let path = std::path::Path::new(path_text);
    if path.exists() {
        return path.is_file();
    }

    true
}

fn is_relative_slash_path(path_text: &str) -> bool {
    let segments = path_text
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    let syntactically_path_like = segments.len() >= 2
        && segments
            .first()
            .and_then(|segment| segment.chars().next())
            .is_some_and(|ch| ch.is_ascii_alphabetic() || matches!(ch, '.' | '_'))
        && segments.iter().all(|segment| {
            *segment != "." && *segment != ".." && segment.chars().all(is_path_segment_char)
        });

    if !syntactically_path_like {
        return false;
    }

    if has_known_extension(path_text)
        || segments
            .last()
            .is_some_and(|segment| is_known_local_filename(segment))
    {
        return true;
    }

    expand_local_path(path_text).is_some_and(|path| path.is_file())
}

fn is_path_segment_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+')
}

fn is_known_local_filename(path_text: &str) -> bool {
    matches!(
        path_text.to_ascii_lowercase().as_str(),
        ".env"
            | ".gitignore"
            | ".gitattributes"
            | "agents.md"
            | "cargo.lock"
            | "cargo.toml"
            | "dockerfile"
            | "justfile"
            | "license"
            | "makefile"
            | "package.json"
            | "pnpm-lock.yaml"
            | "readme.md"
    )
}

fn has_known_extension(path_text: &str) -> bool {
    let Some(ext) = path_text.rsplit('.').next() else {
        return false;
    };
    if ext == path_text || ext.is_empty() || ext.len() > 8 {
        return false;
    }

    matches!(
        ext.to_ascii_lowercase().as_str(),
        "c" | "cc"
            | "cpp"
            | "css"
            | "go"
            | "h"
            | "hpp"
            | "html"
            | "java"
            | "js"
            | "json"
            | "jsonc"
            | "jsx"
            | "kt"
            | "lock"
            | "lua"
            | "m"
            | "md"
            | "mdx"
            | "mm"
            | "gif"
            | "jpeg"
            | "jpg"
            | "pdf"
            | "png"
            | "py"
            | "rb"
            | "rs"
            | "sh"
            | "sql"
            | "svg"
            | "swift"
            | "toml"
            | "ts"
            | "tsx"
            | "txt"
            | "vue"
            | "webp"
            | "xml"
            | "yaml"
            | "yml"
            | "zig"
    )
}

fn line_to_string(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        layout::Rect,
        text::Line,
        widgets::{Paragraph, Widget},
    };

    fn detected_texts(text: &str) -> Vec<String> {
        detect_hyperlinks(text)
            .into_iter()
            .map(|range| range.text)
            .collect()
    }

    #[test]
    fn detects_relative_and_basename_paths() {
        let text = "Read README.md, AGENTS.md and src/ui/components/chat.rs:12.";
        assert_eq!(
            detected_texts(text),
            vec!["README.md", "AGENTS.md", "src/ui/components/chat.rs:12"]
        );
    }

    #[test]
    fn avoids_common_non_path_slash_tokens() {
        assert!(detect_hyperlinks(
            "streaming at 42t/s, ratio=1/2, non-selectable/unhighlighted, /connect"
        )
        .is_empty());
    }

    #[test]
    fn extracts_location_suffix_from_file_target() {
        let text = "See src/main.rs:42";
        let links = detect_hyperlinks(text);
        assert_eq!(links.len(), 1);
        match &links[0].target {
            HyperlinkTarget::File(target) => {
                assert!(target.path.ends_with("src/main.rs"));
                assert!(!target.path.to_string_lossy().ends_with(":42"));
                assert_eq!(target.line, Some(42));
                assert_eq!(target.column, None);
            }
            HyperlinkTarget::Url(url) => panic!("expected file target, got {url}"),
        }
    }

    #[test]
    fn strips_location_suffix_from_file_scheme_target() {
        let file_url = Url::from_file_path(std::env::current_dir().unwrap().join("src/main.rs"))
            .unwrap()
            .to_string();
        let target = file_target_for_file_url_token(&format!("{file_url}:42")).unwrap();

        assert!(target.path.ends_with("src/main.rs"));
        assert!(!target.path.to_string_lossy().ends_with(":42"));
        assert_eq!(target.line, Some(42));
    }

    #[test]
    fn marks_rendered_cells_without_changing_symbols() {
        let area = Rect::new(0, 0, 80, 1);
        let line = Line::from("Added src/new.rs (+1 -0)");
        let mut buf = Buffer::empty(area);

        Paragraph::new(line.clone()).render(area, &mut buf);
        mark_detected_hyperlinks(&mut buf, area, &[line]);

        let linked = (0..area.width)
            .filter_map(|x| {
                buf[(x, 0)]
                    .modifier
                    .contains(Modifier::UNDERLINED)
                    .then_some(buf[(x, 0)].symbol().to_string())
            })
            .collect::<Vec<_>>();

        assert_eq!(linked.len(), "src/new.rs".len());
        assert!(linked[0].contains('s'));
        assert!(!linked.iter().any(|symbol| symbol.contains("\x1B]8;;")));
        assert!(buf[("Added ".len() as u16, 0)]
            .modifier
            .contains(Modifier::UNDERLINED));
    }

    #[test]
    fn returns_target_at_line_column() {
        let line = Line::from("Open src/ui/hyperlink.rs:12");
        let target = hyperlink_at_line_col(&line, "Open src".len()).unwrap();

        match target {
            HyperlinkTarget::File(target) => {
                assert!(target.path.ends_with("src/ui/hyperlink.rs"));
                assert_eq!(target.line, Some(12));
            }
            HyperlinkTarget::Url(url) => panic!("expected file target, got {url}"),
        }
    }

    #[test]
    fn resolves_file_link_split_across_wrapped_lines() {
        let path = "/Users/carlo/work/some-project/PR_REVIEW_20260821_112404.md";
        let prefix = "⬢ Added ";
        let full = format!("{prefix}{path}");

        // Simulate hard wrap by splitting mid-path (no whitespace break).
        let first = "⬢ Added /Users/carlo/work/some-project/PR";
        let second = "_REVIEW_20260821_112404.md";
        assert_eq!(format!("{first}{second}"), full);

        let width = UnicodeWidthStr::width(first);
        assert!(width > 0);

        let lines = vec![Line::from(first), Line::from(second)];

        // Click on first half.
        let target = hyperlink_at_wrapped_lines(&lines, 0, prefix.len() + 2, width).unwrap();
        match target {
            HyperlinkTarget::File(file) => {
                assert_eq!(file.path, std::path::Path::new(path));
            }
            HyperlinkTarget::Url(url) => panic!("expected file, got {url}"),
        }

        // Click on second half.
        let target = hyperlink_at_wrapped_lines(&lines, 1, 2, width).unwrap();
        match target {
            HyperlinkTarget::File(file) => {
                assert_eq!(file.path, std::path::Path::new(path));
            }
            HyperlinkTarget::Url(url) => panic!("expected file, got {url}"),
        }

        // Underline range on the second line should cover the fragment.
        let range = hyperlink_range_at_wrapped_lines(&lines, 1, 2, width).unwrap();
        assert_eq!(range.text, path);
        assert_eq!(range.start_col, 0);
        assert_eq!(range.end_col, UnicodeWidthStr::width(second));
    }

    #[test]
    fn returns_underline_segments_for_all_wrap_parts() {
        let path = "/Users/carlo/work/some-project/PR_REVIEW_20260821_112404.md";
        let first = "⬢ Added /Users/carlo/work/some-project/PR";
        let second = "_REVIEW_20260821_112404.md";
        let width = UnicodeWidthStr::width(first);
        let lines = vec![Line::from(first), Line::from(second)];

        let (range, segments) = hyperlink_segments_at_wrapped_lines(&lines, 1, 2, width).unwrap();
        assert_eq!(range.text, path);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].line_idx, 0);
        assert_eq!(segments[0].start_col, UnicodeWidthStr::width("⬢ Added "));
        assert_eq!(segments[0].end_col, UnicodeWidthStr::width(first));
        assert_eq!(segments[1].line_idx, 1);
        assert_eq!(segments[1].start_col, 0);
        assert_eq!(segments[1].end_col, UnicodeWidthStr::width(second));
    }

    #[test]
    fn stitches_soft_wrap_after_path_separator() {
        let path = "/Users/carlo/work/some-project/PR_REVIEW_20260821_112404.md";
        let first = "⬢ Added /Users/carlo/work/some-project/";
        let second = "PR_REVIEW_20260821_112404.md";
        assert_eq!(format!("{first}{second}"), format!("⬢ Added {path}"));

        let lines = vec![Line::from(first), Line::from(second)];
        let width = 80; // previous line does not fill width

        let target = hyperlink_at_wrapped_lines(&lines, 1, 0, width).unwrap();
        match target {
            HyperlinkTarget::File(file) => {
                assert_eq!(file.path, std::path::Path::new(path));
            }
            HyperlinkTarget::Url(url) => panic!("expected file, got {url}"),
        }
    }

    #[test]
    fn does_not_stitch_unrelated_adjacent_lines() {
        let lines = vec![
            Line::from("short line"),
            Line::from("/Users/carlo/work/file.md"),
        ];
        // First line does not fill wrap width, so no stitch.
        let target = hyperlink_at_wrapped_lines(&lines, 1, 1, 80).unwrap();
        match target {
            HyperlinkTarget::File(file) => {
                assert_eq!(file.path, std::path::Path::new("/Users/carlo/work/file.md"));
            }
            HyperlinkTarget::Url(url) => panic!("expected file, got {url}"),
        }

        assert!(hyperlink_at_wrapped_lines(&lines, 0, 0, 80).is_none());
    }
}
