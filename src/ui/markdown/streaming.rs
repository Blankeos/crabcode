use crate::theme::ThemeColors;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// A simple streaming markdown renderer that caches parsed content
/// to avoid re-parsing on every frame during streaming.
///
/// This implements the "Simple Caching Strategy" from the streaming markdown plan.
/// It only re-parses when the content changes, not every render call.
///
/// Note: Due to version incompatibility between tui-markdown (uses ratatui-core)
/// and our ratatui version, we store content and render it directly.
#[derive(Debug, Clone)]
pub struct SimpleStreamingRenderer {
    content: String,
    last_content_hash: u64,
    needs_render: bool,
}

impl SimpleStreamingRenderer {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            last_content_hash: 0,
            needs_render: true,
        }
    }

    /// Reset the renderer for a new message
    pub fn reset(&mut self) {
        self.content.clear();
        self.last_content_hash = 0;
        self.needs_render = true;
    }

    /// Append new content from the stream
    pub fn append(&mut self, chunk: &str) {
        self.content.push_str(chunk);
        self.needs_render = true;
    }

    /// Get the current content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Check if the renderer needs to be re-rendered
    pub fn needs_render(&self) -> bool {
        self.needs_render
    }

    /// Mark the renderer as rendered (reset the needs_render flag)
    pub fn mark_rendered(&mut self) {
        self.needs_render = false;
        self.last_content_hash = compute_hash(&self.content);
    }

    /// Get the content to render
    /// Returns the markdown content that should be rendered
    pub fn get_content(&self) -> &str {
        &self.content
    }
}

impl Default for SimpleStreamingRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute a hash of the content
fn compute_hash(content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy)]
struct MarkdownStyleSheet {
    colors: ThemeColors,
}

impl MarkdownStyleSheet {
    fn new(colors: ThemeColors) -> Self {
        Self { colors }
    }
}

impl tui_markdown::StyleSheet for MarkdownStyleSheet {
    fn heading(&self, _level: u8) -> ratatui_core::style::Style {
        ratatui_core::style::Style::default()
            .fg(convert_color_to_core(self.colors.markdown_heading))
            .add_modifier(ratatui_core::style::Modifier::BOLD)
    }

    fn code(&self) -> ratatui_core::style::Style {
        ratatui_core::style::Style::default()
            .fg(convert_color_to_core(self.colors.markdown_code))
            .bg(convert_color_to_core(self.colors.background_element))
    }

    fn link(&self) -> ratatui_core::style::Style {
        ratatui_core::style::Style::default()
            .fg(convert_color_to_core(self.colors.markdown_link))
            .add_modifier(ratatui_core::style::Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> ratatui_core::style::Style {
        ratatui_core::style::Style::default()
            .fg(convert_color_to_core(self.colors.markdown_block_quote))
            .add_modifier(ratatui_core::style::Modifier::ITALIC)
    }

    fn heading_meta(&self) -> ratatui_core::style::Style {
        ratatui_core::style::Style::default()
            .fg(convert_color_to_core(self.colors.text_weak))
            .add_modifier(ratatui_core::style::Modifier::DIM)
    }

    fn metadata_block(&self) -> ratatui_core::style::Style {
        ratatui_core::style::Style::default()
            .fg(convert_color_to_core(self.colors.text_weak))
            .add_modifier(ratatui_core::style::Modifier::DIM)
    }
}

/// Render markdown content to lines
/// This uses tui-markdown to parse and render the markdown
pub fn render_markdown(
    content: &str,
    max_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let options = tui_markdown::Options::new(MarkdownStyleSheet::new(*colors));
    let text = tui_markdown::from_str_with_options(content, &options);

    // Convert to our ratatui version's Line type and wrap to max_width
    let mut result = Vec::new();
    let mut in_code_block = false;

    for line in text.lines {
        // Convert ratatui-core Line to our ratatui Line
        let mut converted_line = convert_line(line);
        apply_markdown_theme(&mut converted_line, &mut in_code_block, colors);

        // Check if line needs wrapping
        let line_str = line_to_string(&converted_line);
        let line_width = unicode_width::UnicodeWidthStr::width(line_str.as_str());

        if line_width <= max_width {
            result.push(converted_line);
        } else {
            // Wrap the line
            let wrap_style = converted_line
                .spans
                .first()
                .map(|span| span.style)
                .unwrap_or_else(|| Style::default().fg(colors.markdown_text));
            let wrapped = wrap_line(&line_str, max_width, wrap_style);
            result.extend(wrapped);
        }
    }

    result
}

fn apply_markdown_theme(line: &mut Line<'_>, in_code_block: &mut bool, colors: &ThemeColors) {
    let line_text = line_to_string(line);
    let trimmed = line_text.trim_start();

    if trimmed.starts_with("```") {
        style_line(line, Style::default().fg(colors.markdown_code));
        *in_code_block = !*in_code_block;
        return;
    }

    if *in_code_block {
        style_line(
            line,
            Style::default()
                .fg(colors.markdown_code_block)
                .bg(colors.background_element),
        );
        return;
    }

    if trimmed == "---" {
        style_line(line, Style::default().fg(colors.markdown_horizontal_rule));
        return;
    }

    if is_ordered_list_marker(trimmed) {
        if let Some(span) = line.spans.first_mut() {
            span.style = span.style.fg(colors.markdown_list_enumeration);
        }
    } else if is_unordered_list_marker(trimmed) {
        if let Some(span) = line.spans.first_mut() {
            span.style = span.style.fg(colors.markdown_list_item);
        }
    }

    for span in &mut line.spans {
        if span.style.fg.is_some() {
            continue;
        }

        let modifiers = span.style.add_modifier;
        let fg = if modifiers.contains(Modifier::BOLD) {
            colors.markdown_strong
        } else if modifiers.contains(Modifier::ITALIC) {
            colors.markdown_emph
        } else {
            colors.markdown_text
        };

        span.style = span.style.fg(fg);
    }
}

fn style_line(line: &mut Line<'_>, style: Style) {
    for span in &mut line.spans {
        span.style = span.style.patch(style);
    }
}

fn is_unordered_list_marker(trimmed: &str) -> bool {
    trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ")
}

fn is_ordered_list_marker(trimmed: &str) -> bool {
    let mut chars = trimmed.chars().peekable();
    let mut saw_digit = false;

    while let Some(ch) = chars.peek() {
        if ch.is_ascii_digit() {
            saw_digit = true;
            chars.next();
        } else {
            break;
        }
    }

    saw_digit && chars.next() == Some('.') && chars.next() == Some(' ')
}

/// Convert a ratatui-core Line to our ratatui Line
fn convert_line(line: ratatui_core::text::Line<'_>) -> Line<'static> {
    let line_style = convert_style(line.style);
    let spans: Vec<ratatui::text::Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| {
            let content = span.content.to_string();
            let style = line_style.patch(convert_style(span.style));
            ratatui::text::Span::styled(content, style)
        })
        .collect();

    Line::from(spans)
}

/// Convert ratatui-core Style to our ratatui Style
fn convert_style(style: ratatui_core::style::Style) -> ratatui::style::Style {
    let mut new_style = ratatui::style::Style::default();

    // Copy foreground color if present
    if let Some(fg) = style.fg {
        new_style = new_style.fg(convert_color(fg));
    }

    // Copy background color if present
    if let Some(bg) = style.bg {
        new_style = new_style.bg(convert_color(bg));
    }

    // Copy modifiers
    let modifiers = style.add_modifier;
    if modifiers.contains(ratatui_core::style::Modifier::BOLD) {
        new_style = new_style.add_modifier(ratatui::style::Modifier::BOLD);
    }
    if modifiers.contains(ratatui_core::style::Modifier::ITALIC) {
        new_style = new_style.add_modifier(ratatui::style::Modifier::ITALIC);
    }
    if modifiers.contains(ratatui_core::style::Modifier::DIM) {
        new_style = new_style.add_modifier(ratatui::style::Modifier::DIM);
    }
    if modifiers.contains(ratatui_core::style::Modifier::UNDERLINED) {
        new_style = new_style.add_modifier(ratatui::style::Modifier::UNDERLINED);
    }
    if modifiers.contains(ratatui_core::style::Modifier::CROSSED_OUT) {
        new_style = new_style.add_modifier(ratatui::style::Modifier::CROSSED_OUT);
    }
    if modifiers.contains(ratatui_core::style::Modifier::SLOW_BLINK)
        || modifiers.contains(ratatui_core::style::Modifier::RAPID_BLINK)
    {
        new_style = new_style.add_modifier(ratatui::style::Modifier::SLOW_BLINK);
    }
    if modifiers.contains(ratatui_core::style::Modifier::REVERSED) {
        new_style = new_style.add_modifier(ratatui::style::Modifier::REVERSED);
    }

    new_style
}

/// Convert ratatui-core Color to our ratatui Color
fn convert_color(color: ratatui_core::style::Color) -> ratatui::style::Color {
    match color {
        ratatui_core::style::Color::Reset => ratatui::style::Color::Reset,
        ratatui_core::style::Color::Black => ratatui::style::Color::Black,
        ratatui_core::style::Color::Red => ratatui::style::Color::Red,
        ratatui_core::style::Color::Green => ratatui::style::Color::Green,
        ratatui_core::style::Color::Yellow => ratatui::style::Color::Yellow,
        ratatui_core::style::Color::Blue => ratatui::style::Color::Blue,
        ratatui_core::style::Color::Magenta => ratatui::style::Color::Magenta,
        ratatui_core::style::Color::Cyan => ratatui::style::Color::Cyan,
        ratatui_core::style::Color::Gray => ratatui::style::Color::Gray,
        ratatui_core::style::Color::DarkGray => ratatui::style::Color::DarkGray,
        ratatui_core::style::Color::LightRed => ratatui::style::Color::LightRed,
        ratatui_core::style::Color::LightGreen => ratatui::style::Color::LightGreen,
        ratatui_core::style::Color::LightYellow => ratatui::style::Color::LightYellow,
        ratatui_core::style::Color::LightBlue => ratatui::style::Color::LightBlue,
        ratatui_core::style::Color::LightMagenta => ratatui::style::Color::LightMagenta,
        ratatui_core::style::Color::LightCyan => ratatui::style::Color::LightCyan,
        ratatui_core::style::Color::White => ratatui::style::Color::White,
        ratatui_core::style::Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(r, g, b),
        ratatui_core::style::Color::Indexed(i) => ratatui::style::Color::Indexed(i),
    }
}

fn convert_color_to_core(color: ratatui::style::Color) -> ratatui_core::style::Color {
    match color {
        ratatui::style::Color::Reset => ratatui_core::style::Color::Reset,
        ratatui::style::Color::Black => ratatui_core::style::Color::Black,
        ratatui::style::Color::Red => ratatui_core::style::Color::Red,
        ratatui::style::Color::Green => ratatui_core::style::Color::Green,
        ratatui::style::Color::Yellow => ratatui_core::style::Color::Yellow,
        ratatui::style::Color::Blue => ratatui_core::style::Color::Blue,
        ratatui::style::Color::Magenta => ratatui_core::style::Color::Magenta,
        ratatui::style::Color::Cyan => ratatui_core::style::Color::Cyan,
        ratatui::style::Color::Gray => ratatui_core::style::Color::Gray,
        ratatui::style::Color::DarkGray => ratatui_core::style::Color::DarkGray,
        ratatui::style::Color::LightRed => ratatui_core::style::Color::LightRed,
        ratatui::style::Color::LightGreen => ratatui_core::style::Color::LightGreen,
        ratatui::style::Color::LightYellow => ratatui_core::style::Color::LightYellow,
        ratatui::style::Color::LightBlue => ratatui_core::style::Color::LightBlue,
        ratatui::style::Color::LightMagenta => ratatui_core::style::Color::LightMagenta,
        ratatui::style::Color::LightCyan => ratatui_core::style::Color::LightCyan,
        ratatui::style::Color::White => ratatui_core::style::Color::White,
        ratatui::style::Color::Rgb(r, g, b) => ratatui_core::style::Color::Rgb(r, g, b),
        ratatui::style::Color::Indexed(i) => ratatui_core::style::Color::Indexed(i),
    }
}

/// Convert a Line to a String (for width calculation)
fn line_to_string(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

/// Wrap a line string into multiple lines respecting max_width
fn wrap_line(line_str: &str, max_width: usize, style: Style) -> Vec<Line<'static>> {
    let wrapped = textwrap::wrap(line_str, max_width);

    wrapped
        .into_iter()
        .map(|s| Line::from(Span::styled(s.to_string(), style)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn test_colors() -> ThemeColors {
        ThemeColors {
            primary: Color::Rgb(255, 140, 0),
            secondary: Color::Rgb(255, 140, 0),
            accent: Color::Rgb(255, 140, 0),
            interactive: Color::Rgb(255, 140, 0),
            background: Color::Reset,
            dialog_background: Color::Reset,
            background_element: Color::Reset,
            text: Color::Reset,
            text_weak: Color::Rgb(140, 140, 140),
            text_strong: Color::Reset,
            border: Color::Reset,
            border_weak_focus: Color::Reset,
            border_focus: Color::Reset,
            border_strong_focus: Color::Reset,
            success: Color::Rgb(0, 255, 0),
            warning: Color::Rgb(255, 255, 0),
            error: Color::Rgb(255, 0, 0),
            info: Color::Rgb(0, 255, 255),
            markdown_text: Color::Rgb(180, 255, 180),
            markdown_heading: Color::Rgb(0, 255, 255),
            markdown_link: Color::Rgb(0, 200, 255),
            markdown_link_text: Color::Rgb(80, 240, 240),
            markdown_code: Color::Rgb(0, 255, 0),
            markdown_block_quote: Color::Rgb(180, 180, 180),
            markdown_emph: Color::Rgb(255, 210, 120),
            markdown_strong: Color::Rgb(255, 255, 120),
            markdown_horizontal_rule: Color::Rgb(100, 100, 100),
            markdown_list_item: Color::Rgb(0, 255, 255),
            markdown_list_enumeration: Color::Rgb(80, 240, 240),
            markdown_image: Color::Rgb(0, 200, 255),
            markdown_image_text: Color::Rgb(80, 240, 240),
            markdown_code_block: Color::Rgb(180, 255, 180),
        }
    }

    #[test]
    fn test_streaming_renderer_new() {
        let renderer = SimpleStreamingRenderer::new();
        assert!(renderer.content.is_empty());
        assert!(renderer.needs_render);
    }

    #[test]
    fn test_streaming_renderer_append() {
        let mut renderer = SimpleStreamingRenderer::new();
        renderer.append("Hello");
        assert_eq!(renderer.content(), "Hello");
        assert!(renderer.needs_render());
        renderer.mark_rendered();
        assert!(!renderer.needs_render());
    }

    #[test]
    fn test_streaming_renderer_reset() {
        let mut renderer = SimpleStreamingRenderer::new();
        renderer.append("content");
        renderer.mark_rendered();

        renderer.reset();
        assert!(renderer.content.is_empty());
        assert!(renderer.needs_render());
    }

    #[test]
    fn test_render_markdown_basic() {
        let colors = test_colors();
        let lines = render_markdown("# Hello\n\nThis is **bold** and *italic*.", 80, &colors);

        // Should have parsed into lines
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_code_block() {
        let colors = test_colors();
        let lines = render_markdown(
            "```rust\nfn main() {\n    println!(\"Hello\");\n}\n```",
            80,
            &colors,
        );
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_with_wrapping() {
        let colors = test_colors();
        let lines = render_markdown(
            "This is a long line that needs wrapping because it exceeds the maximum width.",
            20,
            &colors,
        );
        // Should produce multiple lines due to wrapping
        assert!(lines.len() > 1);
    }
}
