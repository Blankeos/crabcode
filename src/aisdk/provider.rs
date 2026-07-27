use crate::chunk::ChunkType;
use crate::error::Result;
use crate::message::Message;
use crate::tool::{HostedTool, Tool};
use async_trait::async_trait;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;

#[derive(Debug, Clone)]
pub struct DynamicModel;

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub model_name: String,
    pub provider_name: String,
}

#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug + Clone + 'static {
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    async fn stream_text(
        &self,
        messages: &[Message],
        tools: &[Tool],
        hosted_tools: &[HostedTool],
        headers: &HashMap<String, String>,
    ) -> Result<ProviderStream>;
}

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ChunkType>> + Send>>;
