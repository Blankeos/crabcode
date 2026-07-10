use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::borrow::Cow;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Pre-process markdown content to extract and render tables,
/// replacing table markdown with Unicode box-drawing rendered tables.
pub fn preprocess_tables(content: &str, max_width: usize) -> Cow<'_, str> {
    if !contains_markdown_table(content) {
        return Cow::Borrowed(content);
    }

    let parser = Parser::new_ext(content, Options::ENABLE_TABLES).into_offset_iter();

    let mut result = String::with_capacity(content.len());
    let mut last_end = 0;

    let mut in_table = false;
    let mut table_alignments: Vec<Alignment> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Table(alignments)) => {
                // Flush text before the table
                result.push_str(&content[last_end..range.start]);
                in_table = true;
                table_alignments = alignments;
                rows.clear();
                current_row.clear();
                current_cell.clear();
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                last_end = range.end;
                let rendered = render_table(&rows, &table_alignments, max_width);
                if !result.is_empty() && !result.ends_with("\n\n") {
                    if !result.ends_with('\n') {
                        result.push('\n');
                    }
                    result.push('\n');
                }
                result.push_str(&preserve_table_line_breaks(&rendered));
                rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                // Reset for header row
                current_row.clear();
                current_cell.clear();
            }
            Event::End(TagEnd::TableHead) => {
                if !current_row.is_empty() {
                    rows.push(std::mem::take(&mut current_row));
                }
            }
            Event::Start(Tag::TableRow) => {
                current_row.clear();
                current_cell.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !current_row.is_empty() {
                    rows.push(std::mem::take(&mut current_row));
                }
                current_row = Vec::new();
            }
            Event::Start(Tag::TableCell) => {}
            Event::End(TagEnd::TableCell) => {
                // Flush cell content — even if empty (preserves column alignment)
                current_row.push(std::mem::take(&mut current_cell));
            }
            Event::Text(text) if in_table => {
                // Flatten inline formatting — just collect the text
                current_cell.push_str(&text);
            }
            Event::Code(code) if in_table => {
                // Don't wrap with backticks — tui-markdown will render inline code
                // styling itself, and including backticks breaks width calculations
                current_cell.push_str(&code);
            }
            Event::SoftBreak if in_table => {
                current_cell.push(' ');
            }
            Event::HardBreak if in_table => {
                current_cell.push('\n');
            }
            _ => {}
        }
    }

    // Flush remaining content after last table
    result.push_str(&content[last_end..]);
    Cow::Owned(result)
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

fn wrap_cell(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for source_line in text.lines() {
        wrap_cell_line(source_line.trim(), width, &mut lines);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn wrap_cell_line(line: &str, width: usize, output: &mut Vec<String>) {
    if line.is_empty() {
        output.push(String::new());
        return;
    }

    let mut current = String::new();
    let mut current_width = 0usize;

    for word in line.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        let separator_width = usize::from(!current.is_empty());

        if !current.is_empty() && current_width + separator_width + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += separator_width + word_width;
            continue;
        }

        if !current.is_empty() {
            output.push(std::mem::take(&mut current));
            current_width = 0;
        }

        if word_width <= width {
            current.push_str(word);
            current_width = word_width;
        } else {
            split_long_word(word, width, output, &mut current, &mut current_width);
        }
    }

    if !current.is_empty() {
        output.push(current);
    }
}

fn split_long_word(
    word: &str,
    width: usize,
    output: &mut Vec<String>,
    current: &mut String,
    current_width: &mut usize,
) {
    for ch in word.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if *current_width > 0 && *current_width + ch_width > width {
            output.push(std::mem::take(current));
            *current_width = 0;
        }

        current.push(ch);
        *current_width += ch_width;

        if *current_width == width {
            output.push(std::mem::take(current));
            *current_width = 0;
        }
    }
}

fn format_cell_line(text: &str, width: usize, align: &Alignment) -> String {
    let display_width = UnicodeWidthStr::width(text);
    let padding = width.saturating_sub(display_width);

    match align {
        Alignment::Right => format!("{}{}", " ".repeat(padding), text),
        Alignment::Center => {
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            format!("{}{}{}", " ".repeat(left_pad), text, " ".repeat(right_pad))
        }
        _ => format!("{}{}", text, " ".repeat(padding)),
    }
}

fn preserve_table_line_breaks(rendered: &str) -> String {
    let mut result = String::with_capacity(rendered.len());
    let mut lines = rendered.split('\n').peekable();

    while let Some(line) = lines.next() {
        result.push_str(line);
        if lines.peek().is_some() {
            result.push_str("  \n");
        }
    }

    result
}

fn render_table(rows: &[Vec<String>], alignments: &[Alignment], max_width: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return String::new();
    }

    // Calculate natural column widths
    let mut col_widths: Vec<usize> = vec![0; num_cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                let width = cell.lines().map(UnicodeWidthStr::width).max().unwrap_or(0);
                col_widths[i] = col_widths[i].max(width);
            }
        }
    }

    // Constrain total width to max_width
    // Each column separator uses 1 char ('│') and 1 space padding on each side
    // So per column: 1 (left pad) + width + 1 (right pad), plus 1 for the left border
    let padding_per_col = 2; // one space left, one space right
    let border_chars = num_cols + 1; // left border + separators between cols + right border
    let available_for_content = max_width.saturating_sub(border_chars + num_cols * padding_per_col);

    let min_col_widths = minimum_column_widths(rows, num_cols);
    col_widths = allocate_column_widths(&col_widths, &min_col_widths, available_for_content);

    let mut result = String::new();

    // Top border
    result.push('┌');
    for (i, w) in col_widths.iter().enumerate() {
        if i > 0 {
            result.push('┬');
        }
        result.push_str(&"─".repeat(w + padding_per_col));
    }
    result.push_str("┐\n");

    // Rows
    for (row_idx, row) in rows.iter().enumerate() {
        let wrapped_cells: Vec<Vec<String>> = col_widths
            .iter()
            .enumerate()
            .map(|(col_idx, width)| {
                let cell_text = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                wrap_cell(cell_text, *width)
            })
            .collect();
        let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1).max(1);

        // Row content. Cells that are shorter than the tallest cell in the row
        // are padded with empty visual lines so borders remain aligned.
        for line_idx in 0..row_height {
            result.push('│');
            for (col_idx, width) in col_widths.iter().enumerate() {
                if col_idx > 0 {
                    result.push('│');
                }
                let align = alignments.get(col_idx).unwrap_or(&Alignment::None);
                let cell_line = wrapped_cells[col_idx]
                    .get(line_idx)
                    .map(String::as_str)
                    .unwrap_or("");
                result.push(' ');
                result.push_str(&format_cell_line(cell_line, *width, align));
                result.push(' ');
            }
            result.push_str("│\n");
        }

        // Separator after header
        if row_idx == 0 && rows.len() > 1 {
            result.push('├');
            for (i, w) in col_widths.iter().enumerate() {
                if i > 0 {
                    result.push('┼');
                }
                result.push_str(&"─".repeat(w + padding_per_col));
            }
            result.push_str("┤\n");
        }
    }

    // Bottom border
    result.push('└');
    for (i, w) in col_widths.iter().enumerate() {
        if i > 0 {
            result.push('┴');
        }
        result.push_str(&"─".repeat(w + padding_per_col));
    }
    result.push('┘');

    result
}

fn minimum_column_widths(rows: &[Vec<String>], num_cols: usize) -> Vec<usize> {
    let mut widths = vec![1; num_cols];

    if let Some(header) = rows.first() {
        for (col_idx, cell) in header.iter().enumerate().take(num_cols) {
            let header_word_width = cell
                .split_whitespace()
                .map(UnicodeWidthStr::width)
                .max()
                .unwrap_or(1);
            widths[col_idx] = widths[col_idx].max(header_word_width);
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
    let min_widths: Vec<usize> = natural
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
    let min_total: usize = min_widths.iter().sum();

    if available <= min_total {
        return allocate_tiny_widths(natural, available);
    }

    // Start with readable minimums, then add remaining cells one-by-one to the
    // column that is currently most compressed relative to its natural width.
    // This keeps long text columns balanced instead of starving whichever wide
    // column happens to be sorted first.
    let mut widths = min_widths;
    let mut remaining = available - min_total;
    while remaining > 0 {
        let Some(index) = (0..column_count)
            .filter(|&index| widths[index] < natural[index])
            .max_by(|&left, &right| {
                let left_score = natural[left] * widths[right];
                let right_score = natural[right] * widths[left];
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
        // Backticks are stripped — tui-markdown handles inline code styling
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
