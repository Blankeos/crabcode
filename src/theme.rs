use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    pub name: String,
    pub id: String,
    pub light: ThemeMode,
    pub dark: ThemeMode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeMode {
    pub seeds: ThemeSeeds,
    pub overrides: ThemeOverrides,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeSeeds {
    pub neutral: String,
    pub primary: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub info: String,
    pub interactive: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeOverrides {
    #[serde(rename = "background-base")]
    pub background_base: String,

    #[serde(rename = "text-base")]
    pub text_base: String,

    #[serde(rename = "text-weak")]
    pub text_weak: String,

    #[serde(rename = "text-strong")]
    pub text_strong: String,

    #[serde(rename = "border-base")]
    pub border_base: String,

    #[serde(rename = "syntax-string")]
    pub syntax_string: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub primary: ratatui::style::Color,
    pub background: ratatui::style::Color,
    pub text: ratatui::style::Color,
    pub text_weak: ratatui::style::Color,
    pub text_strong: ratatui::style::Color,
    pub border: ratatui::style::Color,
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

impl Theme {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let theme: Theme = serde_json::from_str(&content)?;
        Ok(theme)
    }

    pub fn get_colors(&self, dark: bool) -> ThemeColors {
        let mode = if dark { &self.dark } else { &self.light };

        ThemeColors {
            primary: parse_hex(&mode.seeds.primary),
            background: parse_hex(&mode.overrides.background_base),
            text: parse_hex(&mode.overrides.text_base),
            text_weak: parse_hex(&mode.overrides.text_weak),
            text_strong: parse_hex(&mode.overrides.text_strong),
            border: parse_hex(&mode.overrides.border_base),
            success: parse_hex(&mode.seeds.success),
            warning: parse_hex(&mode.seeds.warning),
            error: parse_hex(&mode.seeds.error),
            info: parse_hex(&mode.seeds.info),
        }
    }
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
