pub mod anthropic;
pub mod compatible;
pub mod hosted_search;
pub mod openai;

#[allow(unused_imports)]
pub use hosted_search::{
    default_tools_for, should_register_local_websearch, tools_for, HostedSearchSelection,
};

pub use anthropic::Anthropic;
pub use compatible::OpenAICompatible;
pub use openai::OpenAI;
