use anyhow::Result;

pub struct App {
    pub running: bool,
    pub version: String,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    #[allow(clippy::while_immutable_condition)]
    pub async fn run(&mut self) -> Result<()> {
        while self.running {}
        Ok(())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_creation() {
        let app = App::new();
        assert_eq!(app.version, "0.1.0");
        assert!(app.running);
    }

    #[test]
    fn test_app_quit() {
        let mut app = App::new();
        app.quit();
        assert!(!app.running);
    }

    #[test]
    fn test_app_default() {
        let app = App::default();
        assert_eq!(app.version, "0.1.0");
        assert!(app.running);
    }
}
