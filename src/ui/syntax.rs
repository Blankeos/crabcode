use crate::theme::ThemeColors;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, Style as SyntectStyle, Theme, ThemeSet,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
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
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, syntax_set()).ok()?;
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
