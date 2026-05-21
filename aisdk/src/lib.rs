pub mod chunk;
pub mod error;
pub mod message;
pub mod provider;
pub mod providers;
pub mod response;
pub mod stop;
pub mod tool;

pub mod core {
    pub use crate::chunk::{ChunkType, MessagePhase};
    pub use crate::message::Message;
    pub use crate::response::StreamTextResponse;
    pub use crate::stop::{step_count_is, StopReason};
    pub use crate::tool::Tool;

    pub mod language_model {
        pub use crate::chunk::{
            ChunkType as LanguageModelStreamChunkType, MessagePhase as LanguageModelMessagePhase,
        };
        pub use crate::response::LanguageModelStream;
        pub use crate::stop::step_count_is;
        pub use crate::stop::StopReason;
    }

    pub mod utils {
        pub use crate::stop::step_count_is;
    }

    pub mod capabilities {
        pub use crate::provider::DynamicModel;
    }

    pub mod tools {
        pub use crate::tool::ToolExecute;
    }

    pub mod chunk {
        pub use crate::chunk::{ChunkType, MessagePhase};
    }

    pub mod response {
        pub use crate::response::{stream_with_tools, LanguageModelStream, StreamTextResponse};
    }

    pub mod stop {
        pub use crate::stop::{step_count_is, StopReason};
    }
}

pub use crate::core::*;

pub use crate::providers::{Anthropic, OpenAI, OpenAICompatible};
