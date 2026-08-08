//! Shared `@agent` mention detection for the chat input and rendered history.

use std::ops::Range;

/// Find `@agent` mention ranges in a single content line.
///
/// A match is accepted only when:
/// - the `@` is at the start of the line or preceded by whitespace
/// - the following token matches a configured agent name (case-insensitive)
/// - the token ends at a boundary (whitespace, punctuation, or EOL)
///
/// Emails like `user@explore.com` are rejected because the `@` is not at a
/// token boundary.
pub fn agent_mention_ranges_in_line(
    line: &str,
    agent_names: &[String],
) -> Vec<(Range<usize>, String)> {
    if agent_names.is_empty() || line.is_empty() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(rel) = line[i..].find('@') else {
            break;
        };
        let idx = i + rel;
        let at_boundary = idx == 0
            || line[..idx]
                .chars()
                .next_back()
                .map(char::is_whitespace)
                .unwrap_or(true);
        if !at_boundary {
            i = idx + 1;
            continue;
        }

        let after_at = &line[idx + 1..];
        let name_end = after_at
            .char_indices()
            .find(|(_, ch)| !ch.is_alphanumeric() && *ch != '-' && *ch != '_')
            .map(|(pos, _)| pos)
            .unwrap_or(after_at.len());
        if name_end == 0 {
            i = idx + 1;
            continue;
        }

        let candidate = &after_at[..name_end];
        if let Some(matched) = agent_names
            .iter()
            .find(|name| name.eq_ignore_ascii_case(candidate))
        {
            let end = idx + 1 + name_end;
            ranges.push((idx..end, matched.clone()));
            i = end;
        } else {
            i = idx + 1;
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_configured_names_case_insensitive() {
        let agents = vec![
            "explore".to_string(),
            "general".to_string(),
            "executor".to_string(),
        ];

        let ranges = agent_mention_ranges_in_line(
            "use @explore and @General, ignore @unknown and email@explore.com",
            &agents,
        );
        assert_eq!(
            ranges,
            vec![
                (4..12, "explore".to_string()),
                (17..25, "general".to_string()),
            ]
        );

        assert!(agent_mention_ranges_in_line("no mentions", &agents).is_empty());
    }

    #[test]
    fn rejects_mid_token_at() {
        let agents = vec!["explore".to_string()];
        assert!(agent_mention_ranges_in_line("foo@explore bar", &agents).is_empty());
        assert!(agent_mention_ranges_in_line("a@explore", &agents).is_empty());
    }

    #[test]
    fn accepts_start_of_line_and_after_whitespace() {
        let agents = vec!["executor".to_string()];
        assert_eq!(
            agent_mention_ranges_in_line("@executor please", &agents),
            vec![(0..9, "executor".to_string())]
        );
        assert_eq!(
            agent_mention_ranges_in_line("  @executor", &agents),
            vec![(2..11, "executor".to_string())]
        );
    }
}
