extern crate self as aisdk;

#[path = "../../src/aisdk/mod.rs"]
mod shared;

pub mod chunk {
    pub use crate::shared::chunk::*;
}

pub mod error {
    pub use crate::shared::error::*;
}

pub mod message {
    pub use crate::shared::message::*;
}

pub mod provider {
    pub use crate::shared::provider::*;
}

pub mod providers {
    pub use crate::shared::providers::*;
}

pub mod response {
    pub use crate::shared::response::*;
}

pub mod retry {
    pub use crate::shared::retry::*;
}

pub mod stop {
    pub use crate::shared::stop::*;
}

pub mod tool {
    pub use crate::shared::tool::*;
}

pub mod core {
    pub use crate::shared::core::*;
}

pub use crate::shared::{Anthropic, OpenAI, OpenAICompatible};
