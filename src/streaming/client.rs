use anyhow::{Context, Result};
use futures::{stream, Stream, StreamExt};
use reqwest::Client;
use std::pin::Pin;

use super::parser::{StreamEvent, StreamParser};

pub type StreamResponse = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

pub struct StreamClient {
    client: Client,
    parser: StreamParser,
}

impl StreamClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            parser: StreamParser::new(),
        }
    }

    pub async fn stream(
        &mut self,
        url: &str,
        prompt: &str,
        api_key: Option<&str>,
        model_id: &str,
    ) -> Result<StreamResponse> {
        let mut request = self
            .client
            .post(url)
            .header("Content-Type", "application/json");

        if let Some(key) = api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        let body = serde_json::json!({
            "model": model_id,
            "messages": [{"role": "user", "content": prompt}],
            "stream": true
        });

        let response = request
            .json(&body)
            .send()
            .await
            .context("Failed to send request")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Request failed with status: {}",
                response.status()
            ));
        }

        let byte_stream = response.bytes_stream();

        let event_stream = byte_stream
            .map(move |chunk| match chunk {
                Ok(bytes) => {
                    let mut parser = StreamParser::new();
                    let events = parser.parse_chunk(&bytes);
                    stream::iter(events)
                }
                Err(e) => stream::iter(vec![StreamEvent::Error(e.to_string())]),
            })
            .flatten();

        Ok(Box::pin(event_stream))
    }
}

impl Default for StreamClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_client_new() {
        let _client = StreamClient::new();
    }

    #[test]
    fn test_stream_client_default() {
        let _client = StreamClient::default();
    }
}
