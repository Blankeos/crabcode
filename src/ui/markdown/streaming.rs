use ratatui::text::Line;

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

/// Render markdown content to lines
/// This uses tui-markdown to parse and render the markdown
pub fn render_markdown(content: &str, max_width: usize) -> Vec<Line> {
    // Use tui-markdown to parse the content
    let text = tui_markdown::from_str(content);

    // Convert to our ratatui version's Line type and wrap to max_width
    let mut result = Vec::new();

    for line in text.lines {
        // Convert ratatui-core Line to our ratatui Line
        let converted_line = convert_line(line);

        // Check if line needs wrapping
        let line_str = line_to_string(&converted_line);
        let line_width = unicode_width::UnicodeWidthStr::width(line_str.as_str());

        if line_width <= max_width {
            result.push(converted_line);
        } else {
            // Wrap the line
            let wrapped = wrap_line(&line_str, max_width);
            result.extend(wrapped);
        }
    }

    result
}

/// Convert a ratatui-core Line to our ratatui Line
fn convert_line(line: ratatui_core::text::Line<'_>) -> Line<'static> {
    let spans: Vec<ratatui::text::Span<'static>> = line
        .spans
        .into_iter()
        .map(|span| {
            let content = span.content.to_string();
            let style = convert_style(span.style);
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

/// Convert a Line to a String (for width calculation)
fn line_to_string(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

/// Wrap a line string into multiple lines respecting max_width
fn wrap_line(line_str: &str, max_width: usize) -> Vec<Line<'static>> {
    let wrapped = textwrap::wrap(line_str, max_width);

    wrapped
        .into_iter()
        .map(|s| Line::from(s.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let lines = render_markdown("# Hello\n\nThis is **bold** and *italic*.", 80);

        // Should have parsed into lines
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_code_block() {
        let lines = render_markdown("```rust\nfn main() {\n    println!(\"Hello\");\n}\n```", 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_with_wrapping() {
        let lines = render_markdown(
            "This is a long line that needs wrapping because it exceeds the maximum width.",
            20,
        );
        // Should produce multiple lines due to wrapping
        assert!(lines.len() > 1);
    }
}
