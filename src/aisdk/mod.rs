// AI SDK — multi-provider streaming LLM client (vendored; extractable).
//
// Paths under `core` use `super::` so this tree is valid as a crate root.
// Host binary re-exports `chunk`/`error`/… at crate root for internal
// `crate::chunk` paths used throughout providers/response.

pub mod chunk;
pub mod error;
pub mod log;
pub mod message;
pub mod provider;
pub mod providers;
pub mod response;
pub mod retry;
pub mod stop;
pub mod tool;

pub mod core {
    pub use super::message::Message;
    pub use super::tool::Tool;

    pub mod tools {
        pub use super::super::tool::{ToolExecute, ToolOutput};
    }

    pub mod chunk {
        pub use super::super::chunk::{ChunkType, MessagePhase};
    }

    pub mod response {
        pub use super::super::response::{
            stream_with_tools, LanguageModelStream, StreamTextResponse,
        };
    }

    pub mod stop {
        pub use super::super::stop::StopReason;
    }
}

pub use providers::{Anthropic, OpenAI, OpenAICompatible};
