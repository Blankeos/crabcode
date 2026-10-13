use crate::theme::ThemeColors;
use crate::ui::wrapping::wrap_styled_line;
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

/// A rendered table and the source range it replaces. Keeping styled lines out
/// of the markdown parser preserves literal code and cell padding verbatim.
pub(super) struct RenderedTable {
    pub source: Range<usize>,
    pub lines: Vec<Line<'static>>,
}

pub(super) fn render_tables(
    content: &str,
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<RenderedTable> {
    if !contains_markdown_table(content) {
        return Vec::new();
    }

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(content, options).into_offset_iter();
    let mut tables = Vec::new();
    let mut table_start = None;
    let mut alignments = Vec::new();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = Line::default();
    let mut inline_styles = vec![Style::default()];

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Table(table_alignments)) => {
                table_start = Some(range.start);
                alignments = table_alignments;
                rows.clear();
                row.clear();
            }
            Event::End(TagEnd::Table) => {
                if let Some(start) = table_start.take() {
                    tables.push(RenderedTable {
                        source: start..range.end,
                        lines: render_table(&rows, &alignments, max_width, colors),
                    });
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => row.clear(),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                rows.push(std::mem::take(&mut row));
            }
            Event::Start(Tag::TableCell) => {
                cell = Line::default();
                inline_styles.truncate(1);
            }
            Event::End(TagEnd::TableCell) => {
                row.push(std::mem::take(&mut cell));
            }
            Event::Start(
                tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. }),
            ) if table_start.is_some() => {
                let current = *inline_styles.last().unwrap();
                let style = match tag {
                    Tag::Strong => current.add_modifier(Modifier::BOLD),
                    Tag::Emphasis => current.add_modifier(Modifier::ITALIC),
                    Tag::Strikethrough => current.add_modifier(Modifier::CROSSED_OUT),
                    _ => current
                        .fg(colors.markdown_link)
                        .add_modifier(Modifier::UNDERLINED),
                };
                inline_styles.push(style);
            }
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) if table_start.is_some() => {
                inline_styles.pop();
            }
            Event::Text(text) if table_start.is_some() => {
                let style = cell_text_style(*inline_styles.last().unwrap(), colors);
                cell.spans.push(Span::styled(text.into_string(), style));
            }
            Event::Code(code) if table_start.is_some() => {
                let style = inline_styles
                    .last()
                    .copied()
                    .unwrap_or_default()
                    .fg(colors.markdown_code)
                    .bg(colors.background_element);
                cell.spans.push(Span::styled(code.into_string(), style));
            }
            Event::SoftBreak if table_start.is_some() => {
                cell.spans.push(Span::styled(
                    " ",
                    cell_text_style(*inline_styles.last().unwrap(), colors),
                ));
            }
            Event::HardBreak if table_start.is_some() => {
                cell.spans.push(Span::styled(
                    "\n",
                    cell_text_style(*inline_styles.last().unwrap(), colors),
                ));
            }
            _ => {}
        }
    }
    tables
}

fn cell_text_style(style: Style, colors: &ThemeColors) -> Style {
    if style.fg.is_some() {
        return style;
    }
    let fg = if style.add_modifier.contains(Modifier::BOLD) {
        colors.markdown_strong
    } else if style.add_modifier.contains(Modifier::ITALIC) {
        colors.markdown_emph
    } else {
        colors.markdown_text
    };
    style.fg(fg)
}

pub(crate) fn contains_markdown_table(content: &str) -> bool {
    let lines = content.lines().collect::<Vec<_>>();
    lines.iter().enumerate().any(|(index, line)| {
        let line = line.trim().trim_matches('|');
        let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
        if cells.is_empty() {
            return false;
        }
        let mut cells = cells.into_iter();
        let Some(first) = cells.next() else {
            return false;
        };
        let is_delimiter = |cell: &str| {
            let cell = cell.trim_matches(':').trim();
            cell.len() >= 3 && cell.bytes().all(|byte| byte == b'-')
        };
        let delimiter_row = is_delimiter(first) && cells.all(is_delimiter);
        let has_adjacent_table_row = index
            .checked_sub(1)
            .and_then(|index| lines.get(index))
            .or_else(|| lines.get(index + 1))
            .is_some_and(|line| line.contains('|'));
        delimiter_row && has_adjacent_table_row
    })
}

fn wrap_cell(cell: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }
    wrap_styled_line(cell, width)
}

fn render_table(
    rows: &[Vec<Line<'static>>],
    alignments: &[Alignment],
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let num_cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if num_cols == 0 {
        return Vec::new();
    }

    // Measure visible text, not markdown delimiters or style boundaries.
    let plain_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    cell.spans
                        .iter()
                        .flat_map(|span| span.content.chars())
                        .map(|ch| {
                            if ch.is_control() && !matches!(ch, '\n' | '\r') {
                                '�'
                            } else {
                                ch
                            }
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let mut natural = vec![0; num_cols];
    for row in &plain_rows {
        for (index, cell) in row.iter().enumerate() {
            natural[index] =
                natural[index].max(cell.lines().map(UnicodeWidthStr::width).max().unwrap_or(0));
        }
    }
    let available = max_width.saturating_sub(3 * num_cols + 1);
    let minimum = minimum_column_widths(&plain_rows, num_cols);
    let widths = allocate_column_widths(&natural, &minimum, available);
    let base_style = Style::default().fg(colors.markdown_text);
    let border = |left, join, right| {
        let segments = widths
            .iter()
            .map(|width| "─".repeat(width + 2))
            .collect::<Vec<_>>();
        Line::styled(format!("{left}{}{right}", segments.join(join)), base_style)
    };
    let mut result = vec![border('┌', "┬", '┐')];

    for (row_index, row) in rows.iter().enumerate() {
        let wrapped: Vec<_> = widths
            .iter()
            .enumerate()
            .map(|(index, &width)| {
                row.get(index)
                    .map(|cell| wrap_cell(cell, width))
                    .unwrap_or_else(|| vec![Line::default()])
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for line_index in 0..height {
            let mut spans = vec![Span::styled("│", base_style)];
            for (column, &width) in widths.iter().enumerate() {
                let cell_line = wrapped[column].get(line_index);
                let padding = width.saturating_sub(cell_line.map_or(0, Line::width));
                let (left, right) = match alignments.get(column) {
                    Some(Alignment::Right) => (padding, 0),
                    Some(Alignment::Center) => (padding / 2, padding - padding / 2),
                    _ => (0, padding),
                };
                // Padding and borders use the base style, never a cell's code
                // background or emphasis modifiers.
                spans.push(Span::styled(" ".repeat(left + 1), base_style));
                if let Some(cell_line) = cell_line {
                    spans.extend(cell_line.spans.iter().cloned());
                }
                spans.push(Span::styled(
                    format!("{}│", " ".repeat(right + 1)),
                    base_style,
                ));
            }
            result.push(Line::from(spans));
        }
        if row_index == 0 && rows.len() > 1 {
            result.push(border('├', "┼", '┤'));
        }
    }
    result.push(border('└', "┴", '┘'));
    result
}

fn minimum_column_widths(rows: &[Vec<String>], num_cols: usize) -> Vec<usize> {
    let mut widths = vec![1; num_cols];
    for row in rows {
        for (index, cell) in row.iter().enumerate().take(num_cols) {
            let word_width = cell
                .split_whitespace()
                .map(UnicodeWidthStr::width)
                .max()
                .unwrap_or(1);
            widths[index] = widths[index].max(word_width);
        }
    }
    widths
}

fn allocate_column_widths(natural: &[usize], minimum: &[usize], available: usize) -> Vec<usize> {
    if natural.is_empty() {
        return Vec::new();
    }

    let total_natural: usize = natural.iter().sum();
    if total_natural <= available {
        let mut widths = natural.to_vec();
        if let Some(last) = widths.last_mut() {
            *last += available - total_natural;
        }
        return widths;
    }

    let column_count = natural.len();
    // A pathological unbroken word (URL/hash) may exceed the viewport.
    // Cap its minimum at an equal share only when the word minimums cannot fit.
    let mut min_widths: Vec<usize> = natural
        .iter()
        .enumerate()
        .map(|(index, &width)| {
            minimum
                .get(index)
                .copied()
                .unwrap_or(1)
                .clamp(1, width.max(1))
        })
        .collect();
    if min_widths.iter().sum::<usize>() > available {
        let fair_share = (available / column_count).max(1);
        for width in &mut min_widths {
            *width = (*width).min(fair_share);
        }
    }
    let min_total: usize = min_widths.iter().sum();
    if available < min_total {
        return allocate_tiny_widths(natural, available);
    }

    // Square-root weighting gives compact labels/IDs enough room before long
    // prose claims the remainder. Compare squared ratios to avoid floats.
    let mut widths = min_widths;
    let mut remaining = available - min_total;
    while remaining > 0 {
        let Some(index) = (0..column_count)
            .filter(|&index| widths[index] < natural[index])
            .max_by(|&left, &right| {
                let left_score = natural[left] as u128 * (widths[right] as u128).pow(2);
                let right_score = natural[right] as u128 * (widths[left] as u128).pow(2);
                left_score
                    .cmp(&right_score)
                    .then_with(|| natural[left].cmp(&natural[right]))
                    .then_with(|| right.cmp(&left))
            })
        else {
            break;
        };

        widths[index] += 1;
        remaining -= 1;
    }

    if remaining > 0 {
        if let Some(last) = widths.last_mut() {
            *last += remaining;
        }
    }

    widths
}

fn allocate_tiny_widths(natural: &[usize], available: usize) -> Vec<usize> {
    let mut widths = vec![0; natural.len()];
    if available == 0 {
        return widths;
    }

    let mut used = 0usize;
    for width in &mut widths {
        if used < available {
            *width = 1;
            used += 1;
        }
    }

    while used < available {
        let Some(index) = (0..natural.len())
            .filter(|&index| widths[index] < natural[index].max(1))
            .max_by(|&left, &right| {
                let left_width = widths[left].max(1);
                let right_width = widths[right].max(1);
                let left_score = natural[left] * right_width;
                let right_score = natural[right] * left_width;
                left_score
                    .cmp(&right_score)
                    .then_with(|| natural[left].cmp(&natural[right]))
                    .then_with(|| right.cmp(&left))
            })
        else {
            break;
        };
        widths[index] += 1;
        used += 1;
    }

    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_colors() -> ThemeColors {
        crate::theme::Theme::load_builtin_default().get_colors(true)
    }

    fn plain_lines(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn preprocess_tables(content: &str, width: usize) -> String {
        let mut result = String::new();
        let mut end = 0;
        for table in render_tables(content, width, &test_colors()) {
            result.push_str(&content[end..table.source.start]);
            result.push_str(&plain_lines(&table.lines));
            end = table.source.end;
        }
        result.push_str(&content[end..]);
        result
    }

    const TICKET_TABLE: &str = "| Order | Ticket / current status | Problem and next action |\n\
        | --- | --- | --- |\n\
        | 1 | WEB-1281 – In Progress | Reconnect fails with 404 Stream not found, leaving generation stuck. Latest comment: patch didn't work. Next: reproduce an approved in-flight reload in a fresh conversation, then trace stream IDs, lifecycle and resume handling. |\n\
        | 2 | WEB-1286 – In Progress | Large search/aggregate outputs overwhelm model context. Aggregations lack a size bound. Next: establish a repeatable large-payload case, then implement and verify predictable per-call budgets. |\n\
        | 3 | WEB-1287 – In Progress | Compaction drops earlier search evidence. Next: create a long-thread regression with known evidence, then retain useful structured summaries rather than opaque stubs. |\n\
        | 4 | WEB-1282 – Later | Too many sequential lookups make answers slow. Next: inspect the reported slow chat, confirm where the time went, and identify independent searches that can run in parallel. |\n\
        | 5 | WEB-1115 – Later | Chat suggestions exceed their model's context limit. Separate from the main answer pipeline. Next: reproduce an oversized suggestion request and bound its input to the actual model limit. |\n";

    #[test]
    fn ticket_status_words_remain_intact_beside_long_descriptions() {
        let expected = "Ticket / current status WEB-1281 – In Progress WEB-1286 – In Progress WEB-1287 – In Progress WEB-1282 – Later WEB-1115 – Later";

        for width in [60, 80, 100, 120] {
            let result = preprocess_tables(TICKET_TABLE, width);
            let status_words = result
                .lines()
                .filter(|line| line.starts_with('│'))
                .flat_map(|line| line.split('│').nth(2).unwrap().split_whitespace())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(status_words, expected, "width {width}:\n{result}");
            for line in result.lines() {
                assert_eq!(UnicodeWidthStr::width(line.trim_end()), width);
            }
        }
    }

    #[test]
    fn compact_columns_get_room_before_prose_takes_the_remaining_width() {
        let widths = allocate_column_widths(&[5, 23, 240], &[5, 8, 11], 110);
        assert_eq!(widths, [5, 23, 82]);
    }

    #[test]
    fn minimum_widths_consider_words_in_body_cells() {
        let rows = vec![
            vec!["ID".into(), "Status".into()],
            vec!["WEB-1281".into(), "In Progress".into()],
        ];
        assert_eq!(minimum_column_widths(&rows, 2), [8, 8]);
    }

    #[test]
    fn oversized_words_do_not_steal_other_columns_minimum_widths() {
        let widths = allocate_column_widths(&[20, 1000], &[20, 1000], 40);
        assert_eq!(widths, [20, 20]);
    }

    #[test]
    fn equal_prose_columns_receive_balanced_widths() {
        let widths = allocate_column_widths(&[200, 200, 200], &[8, 8, 8], 80);
        assert_eq!(widths.iter().sum::<usize>(), 80);
        assert!(widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1);
    }

    #[test]
    fn wrapping_prefers_spaces_and_splits_only_oversized_words() {
        assert_eq!(
            plain_lines(&wrap_cell(&Line::from("WEB-1281 – In Progress"), 11)),
            "WEB-1281 –\nIn Progress"
        );
        assert_eq!(
            plain_lines(&wrap_cell(&Line::from("abcdefghijk next"), 5)),
            "abcde\nfghij\nk\nnext"
        );
        assert_eq!(
            plain_lines(&wrap_cell(&Line::from(Span::raw("first\n\nsecond")), 10)),
            "first\n\nsecond"
        );
    }

    #[test]
    fn unicode_words_use_display_width_for_column_minimums() {
        let rows = vec![
            vec!["ID".into(), "Notes".into()],
            vec![
                "東京大阪".into(),
                "A long description that needs to wrap over several lines.".into(),
            ],
        ];
        assert_eq!(minimum_column_widths(&rows, 2), [8, 11]);
        let styled_rows = rows
            .iter()
            .map(|row| row.iter().cloned().map(Line::from).collect())
            .collect::<Vec<_>>();
        let result = plain_lines(&render_table(&styled_rows, &[], 40, &test_colors()));
        assert!(result.contains("東京大阪"), "{result}");
        for line in result.lines() {
            assert_eq!(UnicodeWidthStr::width(line), 40, "{result}");
        }
    }

    #[test]
    fn test_simple_table() {
        let input = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let result = preprocess_tables(input, 80);
        assert!(result.contains('┌'));
        assert!(result.contains('┐'));
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(result.contains("1"));
        assert!(result.contains("2"));
        // Should NOT contain markdown table syntax
        assert!(!result.contains('|'));
    }

    #[test]
    fn test_table_cells_wrap_instead_of_truncating() {
        let input = "| Rank | Approach | Notes |\n|---:|---|---|\n| 1 | Native PDF text extraction | Seconds or less per PDF. |\n| 2 | pdfplumber / Camelot / Docling without OCR-heavy mode | Mostly CPU. More expensive than raw text extraction. |\n";
        let result = preprocess_tables(input, 72);

        assert!(!result.contains("..."));

        let table_lines: Vec<&str> = result.lines().filter(|line| line.contains('│')).collect();
        let rank_two_lines = table_lines
            .iter()
            .filter(|line| line.contains(" 2 ") || line.contains("OCR-heavy"))
            .count();
        assert!(
            rank_two_lines > 1,
            "expected the second row to span multiple visual lines:\n{}",
            result
        );

        let first_width = UnicodeWidthStr::width(result.lines().next().unwrap_or("").trim_end());
        for line in result.lines() {
            let width = UnicodeWidthStr::width(line.trim_end());
            assert_eq!(
                width, first_width,
                "all table lines should be padded to the same width:\n{}",
                result
            );
        }
    }

    #[test]
    fn test_short_header_words_do_not_wrap_when_space_is_available() {
        let input = "| Rank | Approach | Why |\n|---:|---|---|\n| 1 | Download PDF → PyMuPDF/pdfplumber text extraction → deterministic parser | Smallest reliable first step. Lets us quickly parse names and validate counts. |\n| 2 | Add pdfplumber/Camelot/Docling for tables | Good next layer for topnotchers/schools. |\n";
        let result = preprocess_tables(input, 120);

        assert!(
            result.lines().any(|line| line.contains("│ Rank │")),
            "Rank header should not split across visual lines:\n{}",
            result
        );
        assert!(
            !result.lines().any(|line| line.contains("│ Ran │")),
            "Rank header should keep its full word when there is enough space:\n{}",
            result
        );
    }

    #[test]
    fn test_table_after_heading_is_separated_for_markdown_renderer() {
        let input = "## Fastest runtime per PDF\n\n| Rank | Approach | Runtime notes |\n|---:|---|---|\n| 1 | Native text extraction | Seconds or less per PDF. |\n";
        let result = preprocess_tables(input, 80);

        assert!(result.contains("## Fastest runtime per PDF\n\n┌"));
        assert!(!result.contains("## ┌"));
    }

    #[test]
    fn test_table_with_alignment() {
        let input = "| Left | Center | Right |\n| :--- | :---: | ---: |\n| a | b | c |\n";
        let result = preprocess_tables(input, 80);
        assert!(result.contains('┌'));
        assert!(result.contains("Left"));
        assert!(result.contains("Center"));
        assert!(result.contains("Right"));
    }

    #[test]
    fn test_empty_table() {
        let input = "No table here";
        let result = preprocess_tables(input, 80);
        assert_eq!(result, "No table here");
    }

    #[test]
    fn horizontal_rule_is_not_detected_as_a_table() {
        assert!(!contains_markdown_table("Before\n\n---\n\nAfter"));
    }

    #[test]
    fn test_mixed_content_with_table() {
        let input = "Some text\n\n| Col1 | Col2 |\n| --- | --- |\n| A | B |\n\nMore text";
        let result = preprocess_tables(input, 80);
        assert!(result.contains("Some text"));
        assert!(result.contains('┌'));
        assert!(result.contains("More text"));
        assert!(!result.contains('|'));
    }

    #[test]
    fn test_table_cell_with_code() {
        let input = "| Tool | Desc |\n| --- | --- |\n| `read` | Read files |\n";
        let result = preprocess_tables(input, 80);
        // The visible text excludes code delimiters; styles are carried by spans.
        assert!(result.contains("read"));
        assert!(!result.contains("`read`"));
    }

    #[test]
    fn test_table_narrow_width() {
        let input = "| Category | Tool | Description |\n| --- | --- | --- |\n| File Ops | `read` | Read files |\n";
        let result = preprocess_tables(input, 40);
        // Should still render despite narrow width
        assert!(result.contains('┌'));
        assert!(!result.contains('|'));
    }

    #[test]
    fn test_multiple_tables() {
        let input = "| A |\n| --- |\n| 1 |\n\nMiddle text\n\n| X |\n| --- |\n| 9 |\n";
        let result = preprocess_tables(input, 80);
        // Count table borders — should have 2 tables
        let top_border_count = result.matches("┌").count();
        assert_eq!(top_border_count, 2);
        assert!(result.contains("Middle text"));
    }

    #[test]
    fn table_source_ranges_leave_surrounding_markdown_untouched() {
        let content = "**Before**\n\n| A |\n| --- |\n| *one* |\n\nMiddle\n\n| B |\n| --- |\n| `two` |\n\n*After*";
        let tables = render_tables(content, 40, &test_colors());
        assert_eq!(tables.len(), 2);
        assert_eq!(&content[..tables[0].source.start], "**Before**\n\n");
        assert_eq!(
            &content[tables[0].source.end..tables[1].source.start],
            "\nMiddle\n\n"
        );
        assert_eq!(&content[tables[1].source.end..], "\n*After*");
    }

    #[test]
    fn fenced_tables_are_left_as_code() {
        let content = "```md\n| A | B |\n| --- | --- |\n| **bold** | `code` |\n```";
        assert!(render_tables(content, 40, &test_colors()).is_empty());
    }

    #[test]
    fn alignment_uses_visible_width_of_styled_cells() {
        let input = "| Left | Center | Right |\n| :--- | :---: | ---: |\n| **x** | *y* | `z` |\n";
        let tables = render_tables(input, 30, &test_colors());
        let output = plain_lines(&tables[0].lines);
        let body = output.lines().find(|line| line.contains('x')).unwrap();
        let cells = body.split('│').skip(1).take(3).collect::<Vec<_>>();
        assert!(cells[0].starts_with(" x"), "{body}");
        assert!(cells[2].ends_with("z "), "{body}");
        let left = cells[1].len() - cells[1].trim_start().len();
        let right = cells[1].len() - cells[1].trim_end().len();
        assert!(left.abs_diff(right) <= 1, "{body}");
        for line in &tables[0].lines {
            assert_eq!(line.width(), 30, "{output}");
        }
    }

    #[test]
    fn allocations_use_the_budget_and_preserve_feasible_word_minimums() {
        for natural in [[0, 0, 0], [5, 23, 240], [3, 7, 1000], [200, 200, 200]] {
            let minimum = natural.map(|width| width.clamp(1, 8));
            for available in 0..150 {
                let widths = allocate_column_widths(&natural, &minimum, available);
                assert_eq!(widths.len(), natural.len());
                assert_eq!(
                    widths.iter().sum::<usize>(),
                    available,
                    "{natural:?} at {available}"
                );
                if natural.iter().sum::<usize>() > available
                    && minimum.iter().sum::<usize>() <= available
                {
                    for (width, minimum) in widths.iter().zip(minimum) {
                        assert!(*width >= minimum);
                    }
                }
            }
        }
    }

    #[test]
    fn test_real_world_table() {
        let input = "| Category | Tool | Description |\n|----------|------|-------------|\n| **File Operations** | `read` | Read file or directory contents with pagination |\n| | `write` | Create or overwrite a file |\n| | `edit` | Replace text in files with smart matching |\n| | `list` | List directory contents in tree format |\n| | `glob` | Find files by glob pattern |\n| | `grep` | Search file contents using regex |\n| **Code & Development** | `bash` | Execute shell commands with timeout and output streaming |\n| | `task` | Launch subagents for complex multi-step tasks |\n| | `explore` | Fast agent for exploring codebases (read-only) |\n| | `general` | General-purpose agent for research and complex tasks |\n| **Specialized Skills** | `skill` | Load domain-specific skills (frontend-design, ratatui) |\n| **Data & Search** | `question` | Ask user questions during execution |\n| | `update_plan` | Update the current task plan |\n| | `webfetch` | Fetch content from URLs and convert to markdown |";
        let result = preprocess_tables(input, 80);
        assert!(result.contains("File Operations"));
        assert!(result.contains("Specialized"));
        assert!(result.contains("Skills"));
        assert!(result.contains("update"));
        assert!(result.contains("webfetch"));
        assert!(!result.contains("..."));
        // Each row should have 3 cells — no concatenation
        assert!(!result.contains("File Operations`read`"));
        assert!(!result.contains('|'));
    }
}
