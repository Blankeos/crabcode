use std::collections::HashMap;
use std::time::{Duration, SystemTime};

pub const RETRY_INITIAL_DELAY: Duration = Duration::from_secs(2);
pub const RETRY_BACKOFF_FACTOR: u32 = 2;
pub const RETRY_MAX_DELAY_NO_HEADERS: Duration = Duration::from_secs(30);
pub const RETRY_MAX_DELAY: Duration = Duration::from_millis(i32::MAX as u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryStatus {
    pub attempt: usize,
    pub message: String,
    pub delay_ms: u64,
    pub next_epoch_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryError {
    pub message: String,
    pub status: Option<u16>,
    pub headers: HashMap<String, String>,
    pub replay_safe: bool,
}

impl RetryError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            headers: HashMap::new(),
            replay_safe: true,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_headers(mut self, headers: &reqwest::header::HeaderMap) -> Self {
        self.headers = normalized_headers(headers);
        self
    }

    pub fn replay_safe(mut self, replay_safe: bool) -> Self {
        self.replay_safe = replay_safe;
        self
    }

    pub fn from_message(message: impl Into<String>) -> Self {
        Self::new(message)
    }
}

impl std::fmt::Display for RetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RetryError {}

pub fn delay(attempt: usize, headers: Option<&HashMap<String, String>>) -> Duration {
    if let Some(headers) = headers {
        if let Some(delay) = retry_after_delay(headers) {
            return cap(delay);
        }
    }

    let exponential = RETRY_INITIAL_DELAY.as_millis().saturating_mul(
        (RETRY_BACKOFF_FACTOR as u128).saturating_pow(attempt.saturating_sub(1) as u32),
    );
    let delay = Duration::from_millis(exponential.min(u64::MAX as u128) as u64);
    if headers.is_some() {
        cap(delay)
    } else {
        delay.min(RETRY_MAX_DELAY_NO_HEADERS)
    }
}

pub fn status_for_attempt(error: &RetryError, attempt: usize) -> RetryStatus {
    let headers = (!error.headers.is_empty()).then_some(&error.headers);
    let delay = delay(attempt, headers);
    RetryStatus {
        attempt,
        message: retry_message(error),
        delay_ms: saturating_millis(delay),
        next_epoch_ms: current_time_ms().saturating_add(saturating_millis(delay)),
    }
}

pub fn retryable(error: &RetryError) -> bool {
    if !error.replay_safe {
        return false;
    }

    if let Some(status) = error.status {
        if status >= 500 || matches!(status, 429 | 503 | 504 | 529) {
            return !is_context_overflow_or_invalid_request(&error.message);
        }
        if matches!(status, 400 | 401 | 403 | 409 | 413 | 422) {
            return false;
        }
    }

    let lower = error.message.to_ascii_lowercase();
    if is_context_overflow_or_invalid_request(&lower) {
        return false;
    }

    lower.contains("rate increased too quickly")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("too many requests")
        || lower.contains("resource_exhausted")
        || lower.contains("temporarily unavailable")
        || lower.contains("overloaded")
        || lower.contains("connection reset")
        || lower.contains("econnreset")
        || lower.contains("websocket")
        || lower.contains("closed before response.completed")
        || lower.contains("ended before response.completed")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("transport")
        || lower.contains("error sending request")
        || lower.contains("server_error")
        || lower.contains("too_many_requests")
        || lower.contains("empty response")
        || lower.contains("reasoning_only")
        || lower.contains("no_visible_content")
}

pub fn retry_message(error: &RetryError) -> String {
    let trimmed = error.message.trim();
    if trimmed.is_empty() {
        return "Provider request failed".to_string();
    }

    let lower = trimmed.to_ascii_lowercase();

    if let Some(parsed) = parse_json_message(trimmed) {
        return parsed;
    }

    if error.status == Some(429) {
        if lower.contains("quota") || lower.contains("insufficient_quota") {
            return "Quota exceeded".to_string();
        }
        return "Too Many Requests".to_string();
    }

    if error
        .status
        .is_some_and(|status| status >= 500 || matches!(status, 503 | 504 | 529))
    {
        return "Provider is overloaded".to_string();
    }

    if lower.contains("overloaded") || lower.contains("resource_exhausted") {
        return "Provider is overloaded".to_string();
    }
    if lower.contains("too many requests") && trimmed.len() > 160 {
        return "Too Many Requests".to_string();
    }
    if lower.contains("rate_limit") && trimmed.len() > 160 {
        return "Rate Limited".to_string();
    }

    trimmed.to_string()
}

pub fn normalized_headers(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

pub fn retry_after_delay(headers: &HashMap<String, String>) -> Option<Duration> {
    if let Some(ms) = headers
        .get("retry-after-ms")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        return Some(Duration::from_millis(ms.ceil().min(u64::MAX as f64) as u64));
    }

    let retry_after = headers.get("retry-after")?;
    if let Ok(seconds) = retry_after.parse::<f64>() {
        if seconds.is_finite() && seconds >= 0.0 {
            return Some(Duration::from_millis(
                (seconds * 1000.0).ceil().min(u64::MAX as f64) as u64,
            ));
        }
    }

    parse_http_date_delta(retry_after)
}

fn parse_http_date_delta(value: &str) -> Option<Duration> {
    let parsed = chrono::DateTime::parse_from_rfc2822(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(value))
        .ok()?;
    let now = chrono::Utc::now();
    let delta = parsed.with_timezone(&chrono::Utc) - now;
    (delta.num_milliseconds() > 0).then(|| Duration::from_millis(delta.num_milliseconds() as u64))
}

fn parse_json_message(value: &str) -> Option<String> {
    let json = serde_json::from_str::<serde_json::Value>(value).ok()?;

    if json.get("type").and_then(|value| value.as_str()) == Some("error") {
        if json
            .get("error")
            .and_then(|error| error.get("type"))
            .and_then(|value| value.as_str())
            == Some("too_many_requests")
        {
            return Some("Too Many Requests".to_string());
        }
        if let Some(message) = json
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        {
            return Some(message.to_string());
        }
        if let Some(code) = json
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(|value| value.as_str())
        {
            if code.contains("rate_limit") {
                return Some("Rate Limited".to_string());
            }
        }
    }

    if let Some(code) = json.get("code").and_then(|value| value.as_str()) {
        if code.contains("exhausted") || code.contains("unavailable") {
            return Some("Provider is overloaded".to_string());
        }
        if code.contains("rate_limit") {
            return Some("Rate Limited".to_string());
        }
    }

    json.get("message")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn is_context_overflow_or_invalid_request(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("input is too long")
        || lower.contains("content_filter")
        || lower.contains("content policy")
        || lower.contains("invalid_request")
}

fn cap(delay: Duration) -> Duration {
    delay.min(RETRY_MAX_DELAY)
}

fn saturating_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_delay_without_retry_headers_at_30_seconds() {
        let delays = (1..=6)
            .map(|attempt| delay(attempt, None).as_millis())
            .collect::<Vec<_>>();

        assert_eq!(delays, vec![2000, 4000, 8000, 16000, 30000, 30000]);
    }

    #[test]
    fn retry_after_ms_takes_precedence() {
        let headers = HashMap::from([
            ("retry-after-ms".to_string(), "1500".to_string()),
            ("retry-after".to_string(), "30".to_string()),
        ]);

        assert_eq!(delay(4, Some(&headers)), Duration::from_millis(1500));
    }

    #[test]
    fn retry_after_seconds_are_supported() {
        let headers = HashMap::from([("retry-after".to_string(), "2.5".to_string())]);

        assert_eq!(delay(1, Some(&headers)), Duration::from_millis(2500));
    }

    #[test]
    fn retry_status_caps_empty_headers_like_missing_headers() {
        let error = RetryError::new("rate limit");

        assert_eq!(status_for_attempt(&error, 6).delay_ms, 30_000);
    }

    #[test]
    fn plain_text_rate_limit_is_retryable() {
        let error = RetryError::new("Rate limit exceeded, please try again later");

        assert!(retryable(&error));
        assert_eq!(
            retry_message(&error),
            "Rate limit exceeded, please try again later"
        );
    }

    #[test]
    fn status_429_is_retryable() {
        let error = RetryError::new("Too Many Requests").with_status(429);

        assert!(retryable(&error));
        assert_eq!(retry_message(&error), "Too Many Requests");
    }

    #[test]
    fn websocket_failures_are_retryable() {
        let error = RetryError::new("WebSocket closed before response.completed");

        assert!(retryable(&error));
    }

    #[test]
    fn context_overflow_is_not_retryable() {
        let error = RetryError::new("context_length_exceeded").with_status(400);

        assert!(!retryable(&error));
    }
}
