pub mod chunk;
pub mod error;
pub mod message;
pub mod provider;
pub mod providers;
pub mod response;
pub mod retry;
pub mod stop;
pub mod tool;

pub mod core {
    pub use crate::aisdk::message::Message;
    pub use crate::aisdk::tool::{HostedTool, Tool};

    pub mod tools {
        pub use crate::aisdk::tool::{ToolExecute, ToolOutput};
    }

    pub mod chunk {
        pub use crate::aisdk::chunk::{ChunkType, MessagePhase};
    }

    pub mod response {
        pub use crate::aisdk::response::{
            stream_with_hosted_tools, LanguageModelStream, StreamTextResponse,
        };
    }

    pub mod stop {
        pub use crate::aisdk::stop::StopReason;
    }
}

pub use crate::aisdk::providers::{Anthropic, OpenAI, OpenAICompatible};
