pub mod message;
pub mod tool;
pub mod chunk;
pub mod stop;
pub mod response;
pub mod error;
pub mod provider;
pub mod providers;

pub mod core {
    pub use crate::chunk::ChunkType;
    pub use crate::message::Message;
    pub use crate::response::StreamTextResponse;
    pub use crate::stop::{step_count_is, StopReason};
    pub use crate::tool::Tool;

    pub mod language_model {
        pub use crate::chunk::ChunkType as LanguageModelStreamChunkType;
        pub use crate::response::LanguageModelStream;
        pub use crate::stop::StopReason;
        pub use crate::stop::step_count_is;
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
        pub use crate::chunk::ChunkType;
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
