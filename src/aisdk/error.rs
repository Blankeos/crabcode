use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Retryable provider error: {0}")]
    RetryableProvider(#[from] crate::retry::RetryError),

    #[error("Missing field: {0}")]
    MissingField(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Tool call error: {0}")]
    ToolCall(String),
}

pub type Result<T> = std::result::Result<T, Error>;
