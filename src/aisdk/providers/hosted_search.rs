//! Provider-executed (server-side) search tools.
//!
//! These are normal aisdk [`Tool`] values with a provider-native transport.
//! Pass them in the same `tools` list as local tools:
//!
//! ```ignore
//! tools.push(openai::tools::web_search());
//! tools.push(xai::tools::x_search());
//! stream_with_tools(provider, messages, tools, ...)
//! ```
//!
//! Host policy chooses *which* tools to include. This module only defines them.

use schemars::Schema;
use serde_json::{json, Value};

use crate::aisdk::tool::{Tool, ToolExecute, ToolTransport};

/// Providers that currently expose hosted web search via their native APIs.
pub fn supports_hosted_web_search(provider_name: &str) -> bool {
    matches!(
        provider_name.to_ascii_lowercase().as_str(),
        "xai" | "openai" | "anthropic" | "openrouter"
    )
}

/// Which provider-executed search tools a host wants to attach.
///
/// Host config (e.g. `websearch.native`) maps into this. aisdk does not read
/// product config — callers pass an explicit selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostedSearchSelection {
    /// Attach provider web search when supported (`web_search` / Anthropic hosted web / OpenRouter web).
    pub web: bool,
    /// Attach provider X/Twitter search when supported (`x_search`). Currently xAI-only.
    pub x: bool,
}

impl HostedSearchSelection {
    pub const ALL: Self = Self { web: true, x: true };
    pub const NONE: Self = Self { web: false, x: false };
    /// Product default: local websearch + complementary provider X search when available.
    pub const DEFAULT: Self = Self { web: false, x: true };
}

/// Whether the host should also register its local `websearch` tool.
///
/// Local websearch is skipped only when native **web** is requested and the
/// provider can supply it. Native `x` is a complement and does not displace local web.
pub fn should_register_local_websearch(provider_name: &str, native_web: bool) -> bool {
    !(native_web && supports_hosted_web_search(provider_name))
}

fn provider_tool(name: &str, description: &str, transport: ToolTransport) -> Tool {
    Tool::builder()
        .name(name)
        .description(description)
        .input_schema(Schema::from(true))
        .execute(ToolExecute::new(|_input| async move {
            Err::<String, _>(
                "provider-executed tool; the model provider runs this server-side".to_string(),
            )
        }))
        .transport(transport)
        .build()
        .expect("provider tool builder inputs are complete")
}

pub mod openai {
    use super::*;

    pub mod tools {
        use super::*;

        /// OpenAI Responses hosted web search: `{ "type": "web_search" }`.
        pub fn web_search() -> Tool {
            provider_tool(
                "web_search",
                "OpenAI provider-executed web search.",
                ToolTransport::ProviderNative(json!({ "type": "web_search" })),
            )
        }
    }
}

pub mod xai {
    use super::*;

    pub mod tools {
        use super::*;

        /// xAI Responses hosted web search: `{ "type": "web_search" }`.
        pub fn web_search() -> Tool {
            provider_tool(
                "web_search",
                "xAI provider-executed web search.",
                ToolTransport::ProviderNative(json!({ "type": "web_search" })),
            )
        }

        /// xAI Responses hosted X/Twitter search: `{ "type": "x_search" }`.
        pub fn x_search() -> Tool {
            provider_tool(
                "x_search",
                "xAI provider-executed X/Twitter search.",
                ToolTransport::ProviderNative(json!({ "type": "x_search" })),
            )
        }
    }
}

pub mod anthropic {
    use super::*;

    pub mod tools {
        use super::*;

        /// Anthropic hosted web search tool (`web_search_20250305`).
        pub fn web_search() -> Tool {
            provider_tool(
                "web_search",
                "Anthropic provider-executed web search.",
                ToolTransport::ProviderNative(json!({
                    "type": "web_search_20250305",
                    "name": "web_search",
                    "max_uses": 5
                })),
            )
        }
    }
}

pub mod openrouter {
    use super::*;

    pub mod tools {
        use super::*;

        /// OpenRouter chat-completions web plugin (`plugins: [{ "id": "web" }]`).
        pub fn web() -> Tool {
            provider_tool(
                "web",
                "OpenRouter provider-executed web search plugin.",
                ToolTransport::OpenRouterPlugin(json!({ "id": "web" })),
            )
        }
    }
}

/// Hosted search tools for a provider filtered by [`HostedSearchSelection`].
///
/// Unknown / unsupported providers return an empty list. Unsupported selection
/// flags are ignored (e.g. `x` on OpenAI).
pub fn tools_for(provider_name: &str, selection: HostedSearchSelection) -> Vec<Tool> {
    match provider_name.to_ascii_lowercase().as_str() {
        "xai" => {
            let mut tools = Vec::new();
            if selection.web {
                tools.push(xai::tools::web_search());
            }
            if selection.x {
                tools.push(xai::tools::x_search());
            }
            tools
        }
        "openai" if selection.web => vec![openai::tools::web_search()],
        "anthropic" if selection.web => vec![anthropic::tools::web_search()],
        "openrouter" if selection.web => vec![openrouter::tools::web()],
        _ => Vec::new(),
    }
}

/// All hosted search tools for a provider (`web` + `x` when available).
pub fn default_tools_for(provider_name: &str) -> Vec<Tool> {
    tools_for(provider_name, HostedSearchSelection::ALL)
}

/// Native `tools` array fragments for provider-native transports.
pub fn provider_native_tool_values(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| match &tool.transport {
            ToolTransport::ProviderNative(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

/// OpenRouter `plugins` fragments from provider-executed tools.
pub fn openrouter_plugin_values(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| match &tool.transport {
            ToolTransport::OpenRouterPlugin(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

/// True when any tool in the list is provider-executed hosted search.
pub fn has_provider_executed_tools(tools: &[Tool]) -> bool {
    tools.iter().any(|tool| tool.is_provider_executed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_matches_supported_providers() {
        assert!(supports_hosted_web_search("xai"));
        assert!(supports_hosted_web_search("OpenAI"));
        assert!(supports_hosted_web_search("anthropic"));
        assert!(supports_hosted_web_search("openrouter"));
        assert!(!supports_hosted_web_search("google"));
        assert!(!supports_hosted_web_search("groq"));
    }

    #[test]
    fn xai_defaults_include_web_and_x_search() {
        let tools = default_tools_for("xai");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "web_search");
        assert_eq!(tools[1].name, "x_search");
        let native = provider_native_tool_values(&tools);
        assert_eq!(native[0]["type"], "web_search");
        assert_eq!(native[1]["type"], "x_search");
    }

    #[test]
    fn xai_selection_can_keep_x_without_web() {
        let tools = tools_for(
            "xai",
            HostedSearchSelection {
                web: false,
                x: true,
            },
        );
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "x_search");
    }

    #[test]
    fn openai_default_is_web_only() {
        let tools = default_tools_for("openai");
        assert_eq!(
            provider_native_tool_values(&tools),
            vec![json!({ "type": "web_search" })]
        );
        assert!(tools_for(
            "openai",
            HostedSearchSelection {
                web: false,
                x: true,
            },
        )
        .is_empty());
    }

    #[test]
    fn openrouter_uses_plugin_transport() {
        let tools = default_tools_for("openrouter");
        assert!(provider_native_tool_values(&tools).is_empty());
        assert_eq!(
            openrouter_plugin_values(&tools),
            vec![json!({ "id": "web" })]
        );
    }

    #[test]
    fn native_web_skips_local_for_supported() {
        assert!(!should_register_local_websearch("xai", true));
        assert!(should_register_local_websearch("xai", false));
        assert!(should_register_local_websearch("groq", true));
    }
}
