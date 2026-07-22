pub mod configuration;

pub use configuration::{
    ConfigLoader, CustomModelConfig, CustomProviderConfig, ImageOpenCommandConfig, ImageOpenWith, ImagesConfig, McpConfig, McpServerConfig,
    NotificationEventConfig, NotificationsConfig, ProviderTimeout, TerminalNotificationCondition,
    TerminalNotificationMode,
};

#[cfg(test)]
pub use configuration::{McpLocalConfig, McpRemoteConfig};

#[cfg(target_os = "macos")]
pub use configuration::MacosNotificationBackend;

pub use configuration::discover_themes;
