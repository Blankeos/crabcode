use crate::theme::ThemeColors;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::path::Path;
use std::sync::OnceLock;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, HighlightIterator, HighlightState, Highlighter,
    Style as SyntectStyle, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

pub fn highlight_code_for_path(
    code: &str,
    path: &str,
    colors: &ThemeColors,
) -> Option<Vec<Vec<Span<'static>>>> {
    let lang = detect_lang_for_path(path)?;
    highlight_code(code, &lang, colors)
}

fn highlight_code(code: &str, lang: &str, colors: &ThemeColors) -> Option<Vec<Vec<Span<'static>>>> {
    if code.is_empty()
        || code.len() > MAX_HIGHLIGHT_BYTES
        || code.lines().count() > MAX_HIGHLIGHT_LINES
    {
        return None;
    }

    let syntax = find_syntax(lang)?;
    let theme = theme_for_colors(colors)?;
    let highlighter = Highlighter::new(theme);
    let mut parse_state = ParseState::new(syntax);
    let mut highlight_state = HighlightState::new(&highlighter, ScopeStack::new());
    let mut blank_is_fixed_point = false;
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        // Sparse patch hunks can have thousands of empty placeholder lines.
        // Do not run every grammar regex on each one once a newline leaves
        // BOTH parser and highlighting state unchanged. Never skip the first
        // newline: it may terminate a comment/string or change indentation.
        let blank = line == "\n";
        if blank && blank_is_fixed_point {
            lines.push(vec![Span::raw("")]);
            continue;
        }
        let before = blank.then(|| (parse_state.clone(), highlight_state.clone()));
        let ops = parse_state.parse_line(line, syntax_set()).ok()?;
        let ranges = HighlightIterator::new(&mut highlight_state, &ops, line, &highlighter);
        let mut spans = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            spans.push(Span::styled(text.to_string(), convert_style(style)));
        }
        if spans.is_empty() {
            spans.push(Span::raw(""));
        }
        blank_is_fixed_point = before.is_some_and(|(parser, highlight)| {
            parser == parse_state && highlight == highlight_state
        });
        lines.push(spans);
    }

    Some(lines)
}

fn detect_lang_for_path(path: &str) -> Option<String> {
    let path = Path::new(path);
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        return Some(ext.to_string());
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let syntaxes = syntax_set();
    let lower = lang.to_ascii_lowercase();
    let normalized = match lower.as_str() {
        "csharp" | "c-sharp" => "cs",
        "golang" => "go",
        "python3" => "python",
        "shell" | "sh" => "bash",
        _ => lower.as_str(),
    };

    syntaxes
        .find_syntax_by_token(normalized)
        .or_else(|| syntaxes.find_syntax_by_extension(normalized))
        .or_else(|| syntaxes.find_syntax_by_name(normalized))
        .or_else(|| {
            syntaxes
                .syntaxes()
                .iter()
                .find(|syntax| syntax.name.eq_ignore_ascii_case(normalized))
        })
}

fn theme_for_colors(colors: &ThemeColors) -> Option<&'static Theme> {
    let themes = &theme_set().themes;
    let theme_name = if is_light(colors.background) {
        "InspiredGitHub"
    } else {
        "base16-ocean.dark"
    };

    themes
        .get(theme_name)
        .or_else(|| themes.get("base16-ocean.dark"))
        .or_else(|| themes.values().next())
}

fn is_light(color: Color) -> bool {
    let Color::Rgb(r, g, b) = color else {
        return false;
    };
    let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    luminance > 160.0
}

fn convert_style(syn_style: SyntectStyle) -> Style {
    let mut style = Style::default();

    if let Some(fg) = convert_color(syn_style.foreground) {
        style = style.fg(fg);
    }

    if syn_style.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }

    style
}

fn convert_color(color: SyntectColor) -> Option<Color> {
    match color.a {
        0x00 => Some(Color::Indexed(color.r)),
        0x01 => None,
        _ => Some(Color::Rgb(color.r, color.g, color.b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::easy::HighlightLines;

    fn reference_highlight(
        code: &str,
        lang: &str,
        colors: &ThemeColors,
    ) -> Vec<Vec<Span<'static>>> {
        let mut highlighter = HighlightLines::new(
            find_syntax(lang).unwrap(),
            theme_for_colors(colors).unwrap(),
        );
        LinesWithEndings::from(code)
            .map(|line| {
                let mut spans = highlighter
                    .highlight_line(line, syntax_set())
                    .unwrap()
                    .into_iter()
                    .filter_map(|(style, text)| {
                        let text = text.trim_end_matches(['\n', '\r']);
                        (!text.is_empty())
                            .then(|| Span::styled(text.to_owned(), convert_style(style)))
                    })
                    .collect::<Vec<_>>();
                if spans.is_empty() {
                    spans.push(Span::raw(""));
                }
                spans
            })
            .collect()
    }

    #[test]
    fn sparse_highlighting_preserves_every_line_and_style() {
        let mut colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        for background in [Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255)] {
            colors.background = background;
            for (lang, before, after) in [
                ("rs", "/* multiline comment", "*/\nfn main() { let x = 1; }"),
                ("rs", "// line comment", "fn main() {}"),
                ("py", "x = \"\"\"multiline", "end\"\"\"\nprint(x)"),
                (
                    "py",
                    "if True:\n    # comment",
                    "    print('nested')\nprint('outer')",
                ),
                ("js", "const x = `template", "end`;\nconst y = /a+/;"),
                (
                    "html",
                    "<script>/* comment",
                    "*/ const x = 1;</script><p>done</p>",
                ),
                ("css", "p { /* comment", "*/ color: red; }"),
                ("yaml", "value: |\n  scalar", "  continuation\nnext: true"),
                ("sh", "cat <<'EOF'", "text\nEOF\necho done"),
                ("md", "```rust\n/* comment", "*/\n```\n# heading"),
                ("json", "{\"key\":", "true}"),
                ("toml", "value = \"\"\"", "end\"\"\"\nother = true"),
            ] {
                for gap in [1, 2, 64] {
                    let code = format!("\n\n{before}\n{}{after}\n\n", "\n".repeat(gap));
                    assert_eq!(
                        highlight_code(&code, lang, &colors).unwrap(),
                        reference_highlight(&code, lang, &colors),
                        "language={lang}, gap={gap}, background={background:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn sparse_highlighting_preserves_long_gaps_and_line_endings() {
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        for ending in ["\n", "\r\n"] {
            let code = format!(
                "// comment{ending}{}fn main() {{}}{ending}{}/* unterminated",
                ending.repeat(1_312),
                ending.repeat(20)
            );
            let actual = highlight_code(&code, "rs", &colors).unwrap();
            assert_eq!(actual.len(), code.lines().count());
            assert_eq!(actual, reference_highlight(&code, "rs", &colors));
        }
    }
}
