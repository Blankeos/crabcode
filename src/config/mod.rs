pub mod configuration;
pub mod runtime;

pub use configuration::{
    ConfigLoader, CustomProviderConfig, EditorConfig, ImageOpenCommandConfig, ImageOpenWith,
    ImagesConfig, McpConfig, McpServerConfig, NotificationEventConfig, NotificationsConfig,
    ProviderTimeout, TerminalNotificationCondition, TerminalNotificationMode,
};
pub use runtime::{ConfigRuntime, ConfigRuntimeOptions};

#[cfg(test)]
pub use configuration::McpLocalConfig;

#[cfg(target_os = "macos")]
pub use configuration::MacosNotificationBackend;

pub use configuration::resolve_startup_theme;
