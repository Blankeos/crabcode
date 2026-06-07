pub mod configuration;

pub use configuration::{
    ConfigLoader, ImageOpenCommandConfig, ImageOpenWith, ImagesConfig, NotificationEventConfig,
    NotificationsConfig, ProviderTimeout, TerminalNotificationCondition, TerminalNotificationMode,
};

#[cfg(target_os = "macos")]
pub use configuration::MacosNotificationBackend;

pub use configuration::discover_themes;
