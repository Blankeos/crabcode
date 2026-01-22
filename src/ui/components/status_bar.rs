use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

pub struct StatusBar {
    pub version: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub agent: String,
    pub model: String,
}

impl StatusBar {
    pub fn new(
        version: String,
        cwd: String,
        branch: Option<String>,
        agent: String,
        model: String,
    ) -> Self {
        Self {
            version,
            cwd,
            branch,
            agent,
            model,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let cwd_display = if self.cwd.len() > 30 {
            format!("...{}", &self.cwd[self.cwd.len() - 27..])
        } else {
            self.cwd.clone()
        };
        let mut left_spans = vec![Span::raw(cwd_display)];

        if let Some(ref branch) = self.branch {
            left_spans.push(Span::raw(" ("));
            left_spans.push(Span::styled(branch, Style::default().fg(Color::Cyan)));
            left_spans.push(Span::raw(")"));
        }

        let left_line = Line::from(left_spans);

        let right_spans = vec![Span::styled(
            &self.version,
            Style::default().add_modifier(Modifier::BOLD),
        )];

        let right_line = Line::from(right_spans);

        let left_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        let right_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };

        f.render_widget(left_line, left_area);
        f.render_widget(right_line, right_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_bar_creation() {
        let status_bar = StatusBar::new(
            "0.1.0".to_string(),
            "/home/user/projects/crabcode".to_string(),
            Some("main".to_string()),
            "PLAN".to_string(),
            "nano-gpt".to_string(),
        );
        assert_eq!(status_bar.version, "0.1.0");
        assert_eq!(status_bar.cwd, "/home/user/projects/crabcode");
        assert_eq!(status_bar.branch, Some("main".to_string()));
        assert_eq!(status_bar.agent, "PLAN");
        assert_eq!(status_bar.model, "nano-gpt");
    }

    #[test]
    fn test_status_bar_no_branch() {
        let status_bar = StatusBar::new(
            "0.1.0".to_string(),
            "/home/user/projects/crabcode".to_string(),
            None,
            "BUILD".to_string(),
            "z-ai".to_string(),
        );
        assert_eq!(status_bar.branch, None);
        assert_eq!(status_bar.agent, "BUILD");
        assert_eq!(status_bar.model, "z-ai");
    }

    #[test]
    fn test_status_bar_empty_branch() {
        let status_bar = StatusBar::new(
            "0.1.0".to_string(),
            "/home/user/projects/crabcode".to_string(),
            None,
            "PLAN".to_string(),
            "nano-gpt".to_string(),
        );
        assert!(status_bar.branch.is_none());
    }
}
