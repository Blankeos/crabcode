use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};

/// Pre-process markdown content to extract and render tables,
/// replacing table markdown with Unicode box-drawing rendered tables.
pub fn preprocess_tables(content: &str, max_width: usize) -> String {
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
                result.push_str(&rendered);
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
                let width = unicode_width::UnicodeWidthStr::width(cell.as_str());
                col_widths[i] = col_widths[i].max(width);
            }
        }
    }

    // Constrain total width to max_width
    // Each column separator uses 1 char ('│') and 1 space padding on each side
    // So per column: 1 (left pad) + width + 1 (right pad), plus 1 for the left border
    let padding_per_col = 2; // one space left, one space right
    let border_chars = num_cols + 1; // left border + separators between cols + right border
    let available_for_content =
        max_width.saturating_sub(border_chars + num_cols * padding_per_col);

    // Distribute available width among columns
    let total_natural: usize = col_widths.iter().sum();
    if total_natural <= available_for_content {
        // All columns fit — expand last column to fill max_width
        col_widths[num_cols - 1] += available_for_content - total_natural;
    } else {
        // Need to shrink. Give each column at least its natural width (capped),
        // then reduce the largest columns.
        let natural = col_widths.clone();

        // Floor: each column gets at least 3 chars or its natural width, whichever is smaller
        let min_widths: Vec<usize> = natural.iter().map(|&w| w.min(3)).collect();
        let min_total: usize = min_widths.iter().sum();

        if min_total >= available_for_content {
            // Even minimums exceed space — distribute proportionally
            let mut remaining = available_for_content;
            for i in 0..num_cols {
                if i == num_cols - 1 {
                    col_widths[i] = remaining.max(3);
                } else {
                    let scaled = (natural[i] * available_for_content) / total_natural;
                    col_widths[i] = scaled.max(3);
                    remaining = remaining.saturating_sub(col_widths[i]);
                }
            }
        } else {
            // Give smaller columns their full natural width first,
            // then give remaining space to wider columns
            let mut indices: Vec<usize> = (0..num_cols).collect();
            // Sort: smallest columns first (they get priority)
            indices.sort_by_key(|&i| natural[i]);

            let mut remaining = available_for_content;
            let cols_left = num_cols;

            for (pos, &i) in indices.iter().enumerate() {
                let still_to_place = cols_left - pos - 1;
                // Reserve minimum for remaining columns
                let reserved = still_to_place * 3;
                let max_possible = remaining.saturating_sub(reserved);
                // Give this column its natural width, but don't exceed available
                col_widths[i] = natural[i].min(max_possible).max(3);
                remaining = remaining.saturating_sub(col_widths[i]);
            }
        }
    }

    let mut result = String::new();

    // Helper to truncate/pad a cell respecting alignment
    let format_cell = |text: &str, width: usize, align: &Alignment| -> String {
        let display_width = unicode_width::UnicodeWidthStr::width(text);
        if display_width > width {
            // Truncate with "..."
            let mut truncated = String::with_capacity(width);
            let mut current_width = 0;
            for ch in text.chars() {
                let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if current_width + ch_width + 3 > width {
                    break;
                }
                truncated.push(ch);
                current_width += ch_width;
            }
            truncated.push_str("...");
            truncated
        } else {
            let padding = width - display_width;
            match align {
                Alignment::Right => {
                    format!("{}{}", " ".repeat(padding), text)
                }
                Alignment::Center => {
                    let left_pad = padding / 2;
                    let right_pad = padding - left_pad;
                    format!("{}{}{}", " ".repeat(left_pad), text, " ".repeat(right_pad))
                }
                _ => {
                    format!("{}{}", text, " ".repeat(padding))
                }
            }
        }
    };

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
        // Row content
        result.push('│');
        for (col_idx, width) in col_widths.iter().enumerate() {
            if col_idx > 0 {
                result.push('│');
            }
            let cell_text = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            let align = alignments.get(col_idx).unwrap_or(&Alignment::None);
            result.push(' ');
            result.push_str(&format_cell(cell_text, *width, align));
            result.push(' ');
        }
        result.push_str("│\n");

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
        let input =
            "| A |\n| --- |\n| 1 |\n\nMiddle text\n\n| X |\n| --- |\n| 9 |\n";
        let result = preprocess_tables(input, 80);
        // Count table borders — should have 2 tables
        let top_border_count = result.matches("┌").count();
        assert_eq!(top_border_count, 2);
        assert!(result.contains("Middle text"));
    }

    #[test]
    fn test_real_world_table() {
        let input = "| Category | Tool | Description |\n|----------|------|-------------|\n| **File Operations** | `read` | Read file or directory contents with pagination |\n| | `write` | Create or overwrite a file |\n| | `edit` | Replace text in files with smart matching |\n| | `list` | List directory contents in tree format |\n| | `glob` | Find files by glob pattern |\n| | `grep` | Search file contents using regex |\n| **Code & Development** | `bash` | Execute shell commands with timeout and output streaming |\n| | `task` | Launch subagents for complex multi-step tasks |\n| | `explore` | Fast agent for exploring codebases (read-only) |\n| | `general` | General-purpose agent for research and complex tasks |\n| **Specialized Skills** | `skill` | Load domain-specific skills (frontend-design, ratatui) |\n| **Data & Search** | `question` | Ask user questions during execution |\n| | `todowrite` | Create and manage structured task lists |\n| | `webfetch` | Fetch content from URLs and convert to markdown |";
        let result = preprocess_tables(input, 80);
        assert!(result.contains("File Operations"));
        assert!(result.contains("Specialized Skills"));
        assert!(result.contains("todowrite"));
        // Each row should have 3 cells — no concatenation
        assert!(!result.contains("File Operations`read`"));
        assert!(!result.contains('|'));
    }
}
