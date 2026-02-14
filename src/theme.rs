use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub primary: ratatui::style::Color,
    pub secondary: ratatui::style::Color,
    pub accent: ratatui::style::Color,
    pub interactive: ratatui::style::Color,
    pub background: ratatui::style::Color,
    pub dialog_background: ratatui::style::Color,
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
    match agent {
        // Match OpenCode primary agent colors:
        // - Build: secondary
        // - Plan: accent
        "Build" => colors.secondary,
        "Plan" => colors.accent,
        _ => colors.primary,
    }
}

pub fn agent_mode_color(agent_mode: Option<&str>, colors: &ThemeColors) -> ratatui::style::Color {
    agent_color(agent_mode.unwrap_or("Plan"), colors)
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub id: String,
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
}

#[derive(Debug, Clone, Deserialize)]
struct DesktopThemeOverrides {
    #[serde(rename = "background-base")]
    pub background_base: String,

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
        let v: Value = serde_json::from_str(&content)?;

        // Some OpenCode theme JSONs don't include name/id; derive from filename.
        let derived_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("theme")
            .to_string();
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

        if v.get("light").is_some() && v.get("dark").is_some() {
            let desktop: DesktopTheme = serde_json::from_value(v)?;
            return Ok(Self {
                name: desktop.name.clone(),
                id: desktop.id.clone(),
                data: ThemeData::Desktop(desktop),
            });
        }

        if v.get("defs").is_some() && v.get("theme").is_some() {
            let tui: TuiTheme = serde_json::from_value(v)?;
            return Ok(Self {
                name,
                id,
                data: ThemeData::Tui(tui),
            });
        }

        Err(format!("Unsupported theme schema in {}", path.display()).into())
    }

    pub fn get_colors(&self, dark: bool) -> ThemeColors {
        match &self.data {
            ThemeData::Desktop(theme) => {
                let mode = if dark { &theme.dark } else { &theme.light };

                let dialog_background = mode
                    .overrides
                    .surface_raised_stronger_non_alpha
                    .as_deref()
                    .or(mode.overrides.background_stronger.as_deref())
                    .unwrap_or(mode.overrides.background_base.as_str());

                let primary = parse_hex(&mode.seeds.primary);
                let interactive = parse_hex(&mode.seeds.interactive);
                ThemeColors {
                    primary,
                    secondary: primary,
                    accent: interactive,
                    interactive,
                    background: parse_hex(&mode.overrides.background_base),
                    dialog_background: parse_hex(dialog_background),
                    text: parse_hex(&mode.overrides.text_base),
                    text_weak: parse_hex(&mode.overrides.text_weak),
                    text_strong: parse_hex(&mode.overrides.text_strong),
                    border: parse_hex(&mode.overrides.border_base),
                    border_weak_focus: parse_hex(&mode.overrides.border_weak_focus),
                    border_focus: parse_hex(&mode.overrides.border_focus),
                    border_strong_focus: parse_hex(&mode.overrides.border_strong_focus),
                    success: parse_hex(&mode.seeds.success),
                    warning: parse_hex(&mode.seeds.warning),
                    error: parse_hex(&mode.seeds.error),
                    info: parse_hex(&mode.seeds.info),
                }
            }
            ThemeData::Tui(theme) => {
                let resolve = |key: &str| resolve_tui_color(theme, key, dark);

                let primary = resolve("primary");
                let secondary = resolve("secondary");
                let accent = resolve("accent");
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
                let background = resolve("background");
                let dialog_background = {
                    let v = resolve("backgroundPanel");
                    if v == ratatui::style::Color::Reset {
                        background
                    } else {
                        v
                    }
                };
                let text = resolve("text");
                let text_weak = resolve("textMuted");
                let border = resolve("border");
                let border_focus = resolve("borderActive");
                let border_weak_focus = resolve("borderSubtle");

                ThemeColors {
                    primary,
                    secondary,
                    accent,
                    interactive,
                    background,
                    dialog_background,
                    text,
                    text_weak,
                    text_strong: text,
                    border,
                    border_weak_focus,
                    border_focus,
                    border_strong_focus: border_focus,
                    success: resolve("success"),
                    warning: resolve("warning"),
                    error: resolve("error"),
                    info: resolve("info"),
                }
            }
        }
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

    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        ratatui::style::Color::Rgb(r, g, b)
    } else {
        ratatui::style::Color::Reset
    }
}
