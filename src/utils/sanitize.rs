pub fn strip_legacy_image_descriptions(content: &str) -> String {
    const OPEN_TAG: &str = "<image_description";
    const CLOSE_TAG: &str = "</image_description>";

    let lower = content.to_ascii_lowercase();
    let mut cursor = 0usize;
    let mut stripped = String::new();
    let mut removed_any = false;

    while let Some(relative_start) = lower[cursor..].find(OPEN_TAG) {
        let start = cursor + relative_start;
        let Some(relative_end) = lower[start..].find(CLOSE_TAG) else {
            break;
        };
        let end = start + relative_end + CLOSE_TAG.len();

        stripped.push_str(&content[cursor..start]);
        cursor = end;
        removed_any = true;
    }

    if !removed_any {
        return content.to_string();
    }

    stripped.push_str(&content[cursor..]);
    collapse_excess_blank_lines(stripped.trim()).to_string()
}

fn collapse_excess_blank_lines(content: &str) -> String {
    let mut output = String::new();
    let mut blank_count = 0usize;

    for line in content.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count > 1 {
                continue;
            }
        } else {
            blank_count = 0;
        }

        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::strip_legacy_image_descriptions;

    #[test]
    fn strips_legacy_image_description_blocks() {
        assert_eq!(
            strip_legacy_image_descriptions(
                "before\n<image_description source=\"vlm-agent\">\nstale\n</image_description>\nafter",
            ),
            "before\n\nafter"
        );
    }

    #[test]
    fn leaves_text_without_legacy_blocks_unchanged() {
        assert_eq!(strip_legacy_image_descriptions("plain text"), "plain text");
    }
}
