use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const BUNDLED_THEMES: &[(&str, &str)] = &[
    (
        "crabcode-orange",
        include_str!("themes/crabcode-orange.json"),
    ),
    ("groknight", include_str!("themes/groknight.json")),
    ("grokday", include_str!("themes/grokday.json")),
    ("aura", include_str!("generated_themes/aura.json")),
    ("ayu", include_str!("generated_themes/ayu.json")),
    ("carbonfox", include_str!("generated_themes/carbonfox.json")),
    (
        "catppuccin",
        include_str!("generated_themes/catppuccin.json"),
    ),
    (
        "catppuccin-frappe",
        include_str!("generated_themes/catppuccin-frappe.json"),
    ),
    (
        "catppuccin-macchiato",
        include_str!("generated_themes/catppuccin-macchiato.json"),
    ),
    ("cobalt2", include_str!("generated_themes/cobalt2.json")),
    ("cursor", include_str!("generated_themes/cursor.json")),
    ("dracula", include_str!("generated_themes/dracula.json")),
    (
        "everforest",
        include_str!("generated_themes/everforest.json"),
    ),
    ("flexoki", include_str!("generated_themes/flexoki.json")),
    ("github", include_str!("generated_themes/github.json")),
    ("gruvbox", include_str!("generated_themes/gruvbox.json")),
    ("kanagawa", include_str!("generated_themes/kanagawa.json")),
    (
        "lucent-orng",
        include_str!("generated_themes/lucent-orng.json"),
    ),
    ("material", include_str!("generated_themes/material.json")),
    ("matrix", include_str!("generated_themes/matrix.json")),
    ("mercury", include_str!("generated_themes/mercury.json")),
    ("monokai", include_str!("generated_themes/monokai.json")),
    ("nightowl", include_str!("generated_themes/nightowl.json")),
    ("nord", include_str!("generated_themes/nord.json")),
    ("one-dark", include_str!("generated_themes/one-dark.json")),
    ("opencode", include_str!("generated_themes/opencode.json")),
    ("orng", include_str!("generated_themes/orng.json")),
    (
        "osaka-jade",
        include_str!("generated_themes/osaka-jade.json"),
    ),
    ("palenight", include_str!("generated_themes/palenight.json")),
    ("rosepine", include_str!("generated_themes/rosepine.json")),
    ("solarized", include_str!("generated_themes/solarized.json")),
    (
        "synthwave84",
        include_str!("generated_themes/synthwave84.json"),
    ),
    (
        "tokyonight",
        include_str!("generated_themes/tokyonight.json"),
    ),
    ("vercel", include_str!("generated_themes/vercel.json")),
    ("vesper", include_str!("generated_themes/vesper.json")),
    ("zenburn", include_str!("generated_themes/zenburn.json")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThemeColors {
    pub primary: ratatui::style::Color,
    pub secondary: ratatui::style::Color,
    pub accent: ratatui::style::Color,
    pub interactive: ratatui::style::Color,
    pub background: ratatui::style::Color,
    pub dialog_background: ratatui::style::Color,
    pub background_element: ratatui::style::Color,
    pub text: ratatui::style::Color,
    pub text_weak: ratatui::style::Color,
    pub text_strong: ratatui::style::Color,
    pub border: ratatui::style::Color,
    pub border_weak_focus: ratatui::style::Color,
    pub border_focus: ratatui::style::Color,
    pub border_strong_focus: ratatui::style::Color,
    pub success: ratatui::style::Color,
    pub warning: ratatui::style::Color,
    pub error: ratatui::style::Color,
    pub info: ratatui::style::Color,
    pub markdown_text: ratatui::style::Color,
    pub markdown_heading: ratatui::style::Color,
    pub markdown_link: ratatui::style::Color,
    pub markdown_link_text: ratatui::style::Color,
    pub markdown_code: ratatui::style::Color,
    pub markdown_block_quote: ratatui::style::Color,
    pub markdown_emph: ratatui::style::Color,
    pub markdown_strong: ratatui::style::Color,
    pub markdown_horizontal_rule: ratatui::style::Color,
    pub markdown_list_item: ratatui::style::Color,
    pub markdown_list_enumeration: ratatui::style::Color,
    pub markdown_image: ratatui::style::Color,
    pub markdown_image_text: ratatui::style::Color,
    pub markdown_code_block: ratatui::style::Color,
    // Diff colors
    pub diff_add: ratatui::style::Color,
    pub diff_add_bg: ratatui::style::Color,
    pub diff_remove: ratatui::style::Color,
    pub diff_remove_bg: ratatui::style::Color,
    pub diff_gutter: ratatui::style::Color,
}

pub fn darken_color(color: ratatui::style::Color, factor: f32) -> ratatui::style::Color {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => {
            let r = (r as f32 * factor).max(0.0).min(255.0) as u8;
            let g = (g as f32 * factor).max(0.0).min(255.0) as u8;
            let b = (b as f32 * factor).max(0.0).min(255.0) as u8;
            ratatui::style::Color::Rgb(r, g, b)
        }
        _ => color,
    }
}

pub fn contrast_text(background: ratatui::style::Color) -> ratatui::style::Color {
    match background {
        ratatui::style::Color::Rgb(r, g, b) => {
            // Relative luminance (rough) to choose black/white for readability.
            let lum = 0.2126 * (r as f32) + 0.7152 * (g as f32) + 0.0722 * (b as f32);
            if lum > 140.0 {
                ratatui::style::Color::Black
            } else {
                ratatui::style::Color::White
            }
        }
        _ => ratatui::style::Color::White,
    }
}

pub fn agent_color(agent: &str, colors: &ThemeColors) -> ratatui::style::Color {
    match agent.to_ascii_lowercase().as_str() {
        // Match OpenCode visible-agent palette rotation for builtins:
        // secondary / accent / success / warning / primary / error / info
        "build" => colors.secondary,
        "plan" => colors.accent,
        "general" => colors.success,
        "explore" => colors.warning,
        "executor" => colors.info,
        other => {
            let palette = [
                colors.secondary,
                colors.accent,
                colors.success,
                colors.warning,
                colors.primary,
                colors.error,
                colors.info,
            ];
            let hash = other.bytes().fold(0usize, |acc, b| {
                acc.wrapping_mul(31).wrapping_add(b as usize)
            });
            palette[hash % palette.len()]
        }
    }
}

pub fn agent_mode_color(agent_mode: Option<&str>, colors: &ThemeColors) -> ratatui::style::Color {
    agent_color(agent_mode.unwrap_or("Plan"), colors)
}

/// Whether a theme is designed for light or dark terminal windows.
/// Searchable in /themes via description ("light" / "dark").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeAppearance {
    Dark,
    Light,
}

impl ThemeAppearance {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeAppearance::Dark => "dark",
            ThemeAppearance::Light => "light",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(ThemeAppearance::Dark),
            "light" => Some(ThemeAppearance::Light),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub id: String,
    pub appearance: ThemeAppearance,
    data: ThemeData,
}

#[derive(Debug, Clone)]
enum ThemeData {
    Desktop(DesktopTheme),
    Tui(TuiTheme),
}

// OpenCode desktop themes ("https://opencode.ai/desktop-theme.json")
#[derive(Debug, Clone, Deserialize)]
struct DesktopTheme {
    pub name: String,
    pub id: String,
    pub light: DesktopThemeMode,
    pub dark: DesktopThemeMode,
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopThemeMode {
    pub seeds: DesktopThemeSeeds,
    pub overrides: DesktopThemeOverrides,
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopThemeSeeds {
    pub neutral: String,
    pub primary: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub info: String,
    pub interactive: String,
    #[serde(rename = "diffAdd", default)]
    pub diff_add: Option<String>,
    #[serde(rename = "diffDelete", default)]
    pub diff_delete: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopThemeOverrides {
    #[serde(rename = "background-base")]
    pub background_base: String,

    #[serde(rename = "background-weak")]
    #[serde(default)]
    pub background_weak: Option<String>,

    #[serde(rename = "background-stronger")]
    #[serde(default)]
    pub background_stronger: Option<String>,

    #[serde(rename = "surface-raised-stronger-non-alpha")]
    #[serde(default)]
    pub surface_raised_stronger_non_alpha: Option<String>,

    #[serde(rename = "text-base")]
    pub text_base: String,

    #[serde(rename = "text-weak")]
    pub text_weak: String,

    #[serde(rename = "text-strong")]
    pub text_strong: String,

    #[serde(rename = "border-base")]
    pub border_base: String,

    #[serde(rename = "border-weak-focus")]
    pub border_weak_focus: String,

    #[serde(rename = "border-focus")]
    pub border_focus: String,

    #[serde(rename = "border-strong-focus")]
    pub border_strong_focus: String,

    #[serde(rename = "syntax-string")]
    pub syntax_string: String,

    #[serde(rename = "markdown-text")]
    #[serde(default)]
    pub markdown_text: Option<String>,

    #[serde(rename = "markdown-heading")]
    #[serde(default)]
    pub markdown_heading: Option<String>,

    #[serde(rename = "markdown-link")]
    #[serde(default)]
    pub markdown_link: Option<String>,

    #[serde(rename = "markdown-link-text")]
    #[serde(default)]
    pub markdown_link_text: Option<String>,

    #[serde(rename = "markdown-code")]
    #[serde(default)]
    pub markdown_code: Option<String>,

    #[serde(rename = "markdown-block-quote")]
    #[serde(default)]
    pub markdown_block_quote: Option<String>,

    #[serde(rename = "markdown-emph")]
    #[serde(default)]
    pub markdown_emph: Option<String>,

    #[serde(rename = "markdown-strong")]
    #[serde(default)]
    pub markdown_strong: Option<String>,

    #[serde(rename = "markdown-horizontal-rule")]
    #[serde(default)]
    pub markdown_horizontal_rule: Option<String>,

    #[serde(rename = "markdown-list-item")]
    #[serde(default)]
    pub markdown_list_item: Option<String>,

    #[serde(rename = "markdown-list-enumeration")]
    #[serde(default)]
    pub markdown_list_enumeration: Option<String>,

    #[serde(rename = "markdown-image")]
    #[serde(default)]
    pub markdown_image: Option<String>,

    #[serde(rename = "markdown-image-text")]
    #[serde(default)]
    pub markdown_image_text: Option<String>,

    #[serde(rename = "markdown-code-block")]
    #[serde(default)]
    pub markdown_code_block: Option<String>,

    #[serde(rename = "surface-diff-add-base", default)]
    pub surface_diff_add_base: Option<String>,

    #[serde(rename = "surface-diff-delete-base", default)]
    pub surface_diff_delete_base: Option<String>,
}

// OpenCode TUI themes ("https://opencode.ai/theme.json")
#[derive(Debug, Clone, Deserialize)]
struct TuiTheme {
    #[serde(default)]
    pub defs: HashMap<String, String>,

    #[serde(default)]
    pub theme: HashMap<String, TuiThemeValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TuiThemeValue {
    Str(String),
    Mode { dark: String, light: String },
}

impl Theme {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)?;

        // Some OpenCode theme JSONs don't include name/id; derive from filename.
        let derived_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("theme")
            .to_string();
        Self::load_from_str(&content, &derived_id)
    }

    pub fn load_builtin_default() -> Self {
        Self::load_from_str(
            include_str!("themes/crabcode-orange.json"),
            "crabcode-orange",
        )
        .expect("embedded default theme must be valid")
    }

    pub fn bundled_themes() -> Vec<Self> {
        BUNDLED_THEMES
            .iter()
            .filter_map(|(id, content)| Self::load_from_str(content, id).ok())
            .collect()
    }

    fn load_from_str(content: &str, derived_id: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let v: Value = serde_json::from_str(content)?;
        let derived_id = derived_id.to_string();
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| derived_id.clone());
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| id.clone());
        let declared_appearance = v
            .get("appearance")
            .and_then(|x| x.as_str())
            .and_then(ThemeAppearance::parse);

        if v.get("light").is_some() && v.get("dark").is_some() {
            let desktop: DesktopTheme = serde_json::from_value(v)?;
            let appearance = declared_appearance.unwrap_or_else(|| {
                appearance_from_color(parse_hex(&desktop.dark.overrides.background_base))
            });
            return Ok(Self {
                name: desktop.name.clone(),
                id: desktop.id.clone(),
                appearance,
                data: ThemeData::Desktop(desktop),
            });
        }

        if v.get("defs").is_some() && v.get("theme").is_some() {
            let tui: TuiTheme = serde_json::from_value(v)?;
            let appearance = declared_appearance.unwrap_or_else(|| {
                let bg = resolve_tui_color(&tui, "background", true);
                let bg = if bg == ratatui::style::Color::Reset {
                    let panel = resolve_tui_color(&tui, "backgroundPanel", true);
                    if panel == ratatui::style::Color::Reset {
                        resolve_tui_color(&tui, "backgroundMenu", true)
                    } else {
                        panel
                    }
                } else {
                    bg
                };
                appearance_from_color(bg)
            });
            return Ok(Self {
                name,
                id,
                appearance,
                data: ThemeData::Tui(tui),
            });
        }

        Err(format!("Unsupported theme schema for {}", derived_id).into())
    }

    /// Resolve theme colors. When `transparent` is true, main `background`
    /// becomes `Color::Reset` so the terminal shows through.
    pub fn get_colors(&self, dark: bool) -> ThemeColors {
        self.get_colors_with(dark, false)
    }

    pub fn get_colors_with(&self, dark: bool, transparent: bool) -> ThemeColors {
        let mut colors = match &self.data {
            ThemeData::Desktop(theme) => {
                let mode = if dark { &theme.dark } else { &theme.light };

                let dialog_background = mode
                    .overrides
                    .surface_raised_stronger_non_alpha
                    .as_deref()
                    .or(mode.overrides.background_stronger.as_deref())
                    .unwrap_or(mode.overrides.background_base.as_str());

                let resolve_override = |value: Option<&str>, fallback: ratatui::style::Color| {
                    value.map(parse_hex).unwrap_or(fallback)
                };

                let primary = parse_hex(&mode.seeds.primary);
                let secondary = primary;
                let interactive = parse_hex(&mode.seeds.interactive);
                let background = parse_hex(&mode.overrides.background_base);
                let dialog_background = parse_hex(dialog_background);
                let background_element = dialog_background;
                let text = parse_hex(&mode.overrides.text_base);
                let text_weak = parse_hex(&mode.overrides.text_weak);
                let text_strong = parse_hex(&mode.overrides.text_strong);
                let border = parse_hex(&mode.overrides.border_base);
                let border_weak_focus = parse_hex(&mode.overrides.border_weak_focus);
                let border_focus = parse_hex(&mode.overrides.border_focus);
                let border_strong_focus = parse_hex(&mode.overrides.border_strong_focus);
                let success = parse_hex(&mode.seeds.success);
                let warning = parse_hex(&mode.seeds.warning);
                let error = parse_hex(&mode.seeds.error);
                let info = parse_hex(&mode.seeds.info);

                let markdown_text = resolve_override(mode.overrides.markdown_text.as_deref(), text);
                let markdown_heading =
                    resolve_override(mode.overrides.markdown_heading.as_deref(), primary);
                let markdown_link = resolve_override(mode.overrides.markdown_link.as_deref(), info);
                let markdown_link_text =
                    resolve_override(mode.overrides.markdown_link_text.as_deref(), info);
                let markdown_code = resolve_override(
                    mode.overrides.markdown_code.as_deref(),
                    parse_hex(&mode.overrides.syntax_string),
                );
                let markdown_block_quote =
                    resolve_override(mode.overrides.markdown_block_quote.as_deref(), text_weak);
                let markdown_emph =
                    resolve_override(mode.overrides.markdown_emph.as_deref(), warning);
                let markdown_strong =
                    resolve_override(mode.overrides.markdown_strong.as_deref(), primary);
                let markdown_horizontal_rule =
                    resolve_override(mode.overrides.markdown_horizontal_rule.as_deref(), border);
                let markdown_list_item =
                    resolve_override(mode.overrides.markdown_list_item.as_deref(), markdown_link);
                let markdown_list_enumeration = resolve_override(
                    mode.overrides.markdown_list_enumeration.as_deref(),
                    markdown_link_text,
                );
                let markdown_image =
                    resolve_override(mode.overrides.markdown_image.as_deref(), markdown_link);
                let markdown_image_text = resolve_override(
                    mode.overrides.markdown_image_text.as_deref(),
                    markdown_link_text,
                );
                let markdown_code_block =
                    resolve_override(mode.overrides.markdown_code_block.as_deref(), markdown_text);

                let diff_add = mode
                    .seeds
                    .diff_add
                    .as_deref()
                    .map(parse_hex)
                    .unwrap_or(success);
                let diff_remove = mode
                    .seeds
                    .diff_delete
                    .as_deref()
                    .map(parse_hex)
                    .unwrap_or(error);
                let diff_add_bg = mode
                    .overrides
                    .surface_diff_add_base
                    .as_deref()
                    .map(parse_hex)
                    .unwrap_or(success);
                let diff_remove_bg = mode
                    .overrides
                    .surface_diff_delete_base
                    .as_deref()
                    .map(parse_hex)
                    .unwrap_or(error);
                let diff_gutter = text_weak;

                ThemeColors {
                    primary,
                    secondary,
                    accent: interactive,
                    interactive,
                    background,
                    dialog_background,
                    background_element,
                    text,
                    text_weak,
                    text_strong,
                    border,
                    border_weak_focus,
                    border_focus,
                    border_strong_focus,
                    success,
                    warning,
                    error,
                    info,
                    markdown_text,
                    markdown_heading,
                    markdown_link,
                    markdown_link_text,
                    markdown_code,
                    markdown_block_quote,
                    markdown_emph,
                    markdown_strong,
                    markdown_horizontal_rule,
                    markdown_list_item,
                    markdown_list_enumeration,
                    markdown_image,
                    markdown_image_text,
                    markdown_code_block,
                    diff_add,
                    diff_add_bg,
                    diff_remove,
                    diff_remove_bg,
                    diff_gutter,
                }
            }
            ThemeData::Tui(theme) => {
                let resolve = |key: &str| resolve_tui_color(theme, key, dark);
                let resolve_or = |key: &str, fallback: ratatui::style::Color| {
                    let v = resolve(key);
                    if v == ratatui::style::Color::Reset {
                        fallback
                    } else {
                        v
                    }
                };

                let primary = resolve("primary");
                let secondary = resolve_or("secondary", primary);
                let accent = resolve_or("accent", secondary);
                let interactive = {
                    // OpenCode theme.json doesn't always include an explicit interactive token.
                    // Map it to primary so we still get a theme-driven value.
                    let v = resolve_tui_color(theme, "interactive", dark);
                    if v == ratatui::style::Color::Reset {
                        primary
                    } else {
                        v
                    }
                };
                // Prefer solid backgrounds: if theme declares transparent, fall back to panel.
                let mut background = resolve("background");
                let panel = resolve("backgroundPanel");
                let menu = resolve("backgroundMenu");
                if background == ratatui::style::Color::Reset {
                    background = if panel != ratatui::style::Color::Reset {
                        panel
                    } else if menu != ratatui::style::Color::Reset {
                        menu
                    } else if dark {
                        ratatui::style::Color::Rgb(0x0d, 0x0d, 0x0d)
                    } else {
                        ratatui::style::Color::Rgb(0xfa, 0xfa, 0xfa)
                    };
                }
                let dialog_background = {
                    if panel != ratatui::style::Color::Reset {
                        panel
                    } else {
                        background
                    }
                };
                let background_element = resolve_or("backgroundElement", dialog_background);
                let text = resolve_or("text", primary);
                let text_weak = resolve_or("textWeak", resolve_or("textMuted", text));
                let border = resolve_or("border", text_weak);
                let border_focus = resolve_or("borderActive", border);
                let border_weak_focus = resolve_or("borderSubtle", border);

                let markdown_text = resolve_or("markdownText", text);
                let markdown_heading = resolve_or("markdownHeading", primary);
                let markdown_link =
                    resolve_or("markdownLink", resolve_or("info", markdown_heading));
                let markdown_link_text = resolve_or("markdownLinkText", markdown_link);
                let markdown_code =
                    resolve_or("markdownCode", resolve_or("success", markdown_text));
                let markdown_block_quote = resolve_or("markdownBlockQuote", text_weak);
                let markdown_emph =
                    resolve_or("markdownEmph", resolve_or("warning", markdown_text));
                let markdown_strong = resolve_or("markdownStrong", markdown_heading);
                let markdown_horizontal_rule = resolve_or("markdownHorizontalRule", border);
                let markdown_list_item = resolve_or("markdownListItem", markdown_link);
                let markdown_list_enumeration =
                    resolve_or("markdownListEnumeration", markdown_link_text);
                let markdown_image = resolve_or("markdownImage", markdown_link);
                let markdown_image_text = resolve_or("markdownImageText", markdown_link_text);
                let markdown_code_block = resolve_or("markdownCodeBlock", markdown_text);

                let success_color = resolve_or("success", primary);
                let error_color = resolve_or("error", primary);
                let diff_add = resolve_or("diffAdd", success_color);
                let diff_remove = resolve_or("diffDelete", error_color);
                let diff_add_bg = resolve_or("diffAddedBg", success_color);
                let diff_remove_bg = resolve_or("diffRemovedBg", error_color);
                let diff_gutter = text_weak;

                ThemeColors {
                    primary,
                    secondary,
                    accent,
                    interactive,
                    background,
                    dialog_background,
                    background_element,
                    text,
                    text_weak,
                    text_strong: text,
                    border,
                    border_weak_focus,
                    border_focus,
                    border_strong_focus: border_focus,
                    success: success_color,
                    warning: resolve_or("warning", primary),
                    error: error_color,
                    info: resolve_or("info", primary),
                    markdown_text,
                    markdown_heading,
                    markdown_link,
                    markdown_link_text,
                    markdown_code,
                    markdown_block_quote,
                    markdown_emph,
                    markdown_strong,
                    markdown_horizontal_rule,
                    markdown_list_item,
                    markdown_list_enumeration,
                    markdown_image,
                    markdown_image_text,
                    markdown_code_block,
                    diff_add,
                    diff_add_bg,
                    diff_remove,
                    diff_remove_bg,
                    diff_gutter,
                }
            }
        };

        if transparent {
            colors.background = ratatui::style::Color::Reset;
        }

        colors
    }
}

fn appearance_from_color(color: ratatui::style::Color) -> ThemeAppearance {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => {
            let lum = 0.2126 * (r as f32) + 0.7152 * (g as f32) + 0.0722 * (b as f32);
            if lum > 140.0 {
                ThemeAppearance::Light
            } else {
                ThemeAppearance::Dark
            }
        }
        // Reset / unknown → treat as dark (most terminals default dark).
        _ => ThemeAppearance::Dark,
    }
}

fn resolve_tui_color(theme: &TuiTheme, key: &str, dark: bool) -> ratatui::style::Color {
    let Some(v) = theme.theme.get(key) else {
        return ratatui::style::Color::Reset;
    };

    let raw = match v {
        TuiThemeValue::Str(s) => s.as_str(),
        TuiThemeValue::Mode { dark: d, light: l } => {
            if dark {
                d.as_str()
            } else {
                l.as_str()
            }
        }
    };

    if raw.trim_start().starts_with('#') {
        return parse_hex(raw);
    }

    let Some(def) = theme.defs.get(raw) else {
        return ratatui::style::Color::Reset;
    };
    parse_hex(def)
}

fn parse_hex(hex: &str) -> ratatui::style::Color {
    let hex = hex.trim_start_matches('#');

    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).unwrap_or(0);
            ratatui::style::Color::Rgb(r, g, b)
        }
        6 | 8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            ratatui::style::Color::Rgb(r, g, b)
        }
        _ => ratatui::style::Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_hex, Theme};

    #[test]
    fn parse_hex_supports_short_rgb() {
        let color = parse_hex("#fff");
        assert_eq!(color, ratatui::style::Color::Rgb(255, 255, 255));
    }

    #[test]
    fn parse_hex_supports_rrggbbaa() {
        let color = parse_hex("#112233ff");
        assert_eq!(color, ratatui::style::Color::Rgb(17, 34, 51));
    }

    #[test]
    fn bundled_themes_include_default_theme() {
        let themes = Theme::bundled_themes();
        assert!(themes.iter().any(|theme| theme.id == "crabcode-orange"));
        assert!(themes.iter().any(|theme| theme.id == "ayu"));
    }

    #[test]
    fn bundled_themes_have_appearance() {
        let themes = Theme::bundled_themes();
        for theme in &themes {
            // Every theme must classify as light or dark.
            assert!(
                matches!(
                    theme.appearance,
                    super::ThemeAppearance::Dark | super::ThemeAppearance::Light
                ),
                "{} missing appearance",
                theme.id
            );
        }
        let grokday = themes.iter().find(|t| t.id == "grokday").unwrap();
        assert_eq!(grokday.appearance, super::ThemeAppearance::Light);
        let groknight = themes.iter().find(|t| t.id == "groknight").unwrap();
        assert_eq!(groknight.appearance, super::ThemeAppearance::Dark);
    }

    #[test]
    fn transparent_override_resets_background_only() {
        let theme = Theme::load_builtin_default();
        let solid = theme.get_colors_with(true, false);
        let clear = theme.get_colors_with(true, true);
        assert_ne!(solid.background, ratatui::style::Color::Reset);
        assert_eq!(clear.background, ratatui::style::Color::Reset);
        assert_eq!(clear.dialog_background, solid.dialog_background);
        assert_eq!(clear.primary, solid.primary);
    }

    #[test]
    fn lucent_theme_defaults_to_solid_background() {
        let themes = Theme::bundled_themes();
        let lucent = themes.iter().find(|t| t.id == "lucent-orng").unwrap();
        let colors = lucent.get_colors(true);
        assert_ne!(
            colors.background,
            ratatui::style::Color::Reset,
            "lucent-orng should paint a solid bg by default"
        );
    }

    #[test]
    fn bundled_themes_include_grok_mono() {
        let themes = Theme::bundled_themes();
        for id in ["groknight", "grokday"] {
            let theme = themes
                .iter()
                .find(|theme| theme.id == id)
                .unwrap_or_else(|| panic!("{id} theme should be bundled"));
            for dark in [true, false] {
                let colors = theme.get_colors(dark);
                assert_ne!(colors.background, ratatui::style::Color::Reset, "{id} bg");
                assert_ne!(colors.primary, ratatui::style::Color::Reset, "{id} primary");
                assert_ne!(colors.text, ratatui::style::Color::Reset, "{id} text");
                // Chrome is monochrome — primary must not be TokyoNight/GrokDay blue.
                assert_ne!(
                    colors.primary,
                    ratatui::style::Color::Rgb(0x7a, 0xa2, 0xf7),
                    "{id} primary should not be TokyoNight blue"
                );
                assert_ne!(
                    colors.primary,
                    ratatui::style::Color::Rgb(0x2f, 0x64, 0xd2),
                    "{id} primary should not be GrokDay blue"
                );
            }
        }
    }
}
