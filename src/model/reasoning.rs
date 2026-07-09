use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningOption {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, deserialize_with = "deserialize_string_values")]
    pub values: Vec<String>,
}

pub fn efforts_from_options(options: &[ReasoningOption]) -> Option<Vec<ReasoningEffort>> {
    let mut efforts = Vec::new();
    for option in options {
        if !option.kind.trim().eq_ignore_ascii_case("effort") {
            continue;
        }

        for value in &option.values {
            let Ok(effort) = value.parse() else {
                continue;
            };
            if !efforts.contains(&effort) {
                efforts.push(effort);
            }
        }
    }

    if efforts.is_empty() {
        None
    } else {
        Some(efforts)
    }
}

pub fn capability_from_options(options: &[ReasoningOption]) -> Option<ReasoningCapability> {
    let efforts = efforts_from_options(options)?;
    catalog_effort_capability(&efforts)
}

fn deserialize_string_values<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(values
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

fn catalog_effort_capability(efforts: &[ReasoningEffort]) -> Option<ReasoningCapability> {
    let values = efforts.to_vec();
    if values.is_empty() {
        return None;
    }

    let default = [
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Low,
        ReasoningEffort::Minimal,
        ReasoningEffort::Max,
        ReasoningEffort::XHigh,
        ReasoningEffort::None,
    ]
    .into_iter()
    .find(|effort| values.contains(effort))
    .unwrap_or(values[0]);

    Some(ReasoningCapability::effort(values, default))
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReasoningEffort {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match normalize_effort_token(value).as_str() {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            _ => Err(()),
        }
    }
}

fn normalize_effort_token(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('_', "")
        .replace('-', "")
        .replace("extrahigh", "xhigh")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningCapability {
    Unsupported,
    Effort {
        values: Vec<ReasoningEffort>,
        default: ReasoningEffort,
    },
}

impl ReasoningCapability {
    pub fn effort(values: Vec<ReasoningEffort>, default: ReasoningEffort) -> Self {
        let default = if values.contains(&default) {
            default
        } else {
            values.first().copied().unwrap_or(default)
        };
        Self::Effort { values, default }
    }

    pub fn values(&self) -> &[ReasoningEffort] {
        match self {
            Self::Unsupported => &[],
            Self::Effort { values, .. } => values,
        }
    }

    pub fn default_effort(&self) -> Option<ReasoningEffort> {
        match self {
            Self::Unsupported => None,
            Self::Effort { default, .. } => Some(*default),
        }
    }

    pub fn resolve(&self, requested: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
        let Some(requested) = requested else {
            return self.default_effort();
        };

        if self.values().contains(&requested) {
            return Some(requested);
        }

        downgrade_candidates(requested)
            .into_iter()
            .find(|effort| self.values().contains(effort))
    }

    pub fn cycle_next(&self, current: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
        self.cycle(current, 1)
    }

    pub fn cycle(
        &self,
        current: Option<ReasoningEffort>,
        direction: i8,
    ) -> Option<ReasoningEffort> {
        let values = self.values();
        if values.is_empty() {
            return None;
        }

        let current = self.resolve(current).or_else(|| self.default_effort())?;
        let idx = values
            .iter()
            .position(|effort| *effort == current)
            .unwrap_or(0);
        if direction < 0 {
            Some(values[(idx + values.len() - 1) % values.len()])
        } else {
            Some(values[(idx + 1) % values.len()])
        }
    }

    pub fn cycle_override(
        &self,
        current: Option<ReasoningEffort>,
        direction: i8,
    ) -> Option<Option<ReasoningEffort>> {
        let values: Vec<_> = self
            .values()
            .iter()
            .copied()
            .filter(|effort| *effort != ReasoningEffort::None)
            .collect();
        if values.is_empty() {
            return None;
        }

        let mut entries = Vec::with_capacity(values.len() + 1);
        entries.push(None);
        entries.extend(values.into_iter().map(Some));

        let current = current
            .and_then(|effort| self.resolve(Some(effort)))
            .filter(|effort| *effort != ReasoningEffort::None);
        let idx = entries
            .iter()
            .position(|effort| *effort == current)
            .unwrap_or(0);

        if direction < 0 {
            Some(entries[(idx + entries.len() - 1) % entries.len()])
        } else {
            Some(entries[(idx + 1) % entries.len()])
        }
    }
}

fn downgrade_candidates(requested: ReasoningEffort) -> Vec<ReasoningEffort> {
    match requested {
        ReasoningEffort::Max => vec![
            ReasoningEffort::Max,
            ReasoningEffort::XHigh,
            ReasoningEffort::High,
            ReasoningEffort::Medium,
            ReasoningEffort::Low,
        ],
        ReasoningEffort::XHigh => vec![
            ReasoningEffort::XHigh,
            ReasoningEffort::High,
            ReasoningEffort::Medium,
            ReasoningEffort::Low,
        ],
        ReasoningEffort::High => vec![
            ReasoningEffort::High,
            ReasoningEffort::Medium,
            ReasoningEffort::Low,
        ],
        ReasoningEffort::Medium => vec![
            ReasoningEffort::Medium,
            ReasoningEffort::Low,
            ReasoningEffort::High,
        ],
        ReasoningEffort::Low => vec![
            ReasoningEffort::Low,
            ReasoningEffort::Minimal,
            ReasoningEffort::Medium,
        ],
        ReasoningEffort::Minimal => vec![ReasoningEffort::Minimal, ReasoningEffort::Low],
        ReasoningEffort::None => vec![ReasoningEffort::None, ReasoningEffort::Minimal],
    }
}

pub fn capability_for_model(
    provider_id: &str,
    provider_npm: &str,
    model_id: &str,
    api_id: &str,
    model_name: &str,
    family: &str,
    release_date: &str,
    models_dev_reasoning: bool,
) -> ReasoningCapability {
    capability_for_model_with_options(
        provider_id,
        provider_npm,
        model_id,
        api_id,
        model_name,
        family,
        release_date,
        models_dev_reasoning,
        &[],
    )
}

pub fn capability_for_model_with_options(
    provider_id: &str,
    provider_npm: &str,
    model_id: &str,
    api_id: &str,
    model_name: &str,
    family: &str,
    release_date: &str,
    models_dev_reasoning: bool,
    reasoning_options: &[ReasoningOption],
) -> ReasoningCapability {
    if !models_dev_reasoning {
        return ReasoningCapability::Unsupported;
    }

    if let Some(capability) = capability_from_options(reasoning_options) {
        return capability;
    }

    let provider = provider_id.to_ascii_lowercase();
    let npm = provider_npm.to_ascii_lowercase();
    let model = model_id.to_ascii_lowercase();
    let api_id = if api_id.trim().is_empty() {
        model.as_str()
    } else {
        api_id
    };
    let api = api_id.to_ascii_lowercase();
    let name = model_name.to_ascii_lowercase();
    let family = family.to_ascii_lowercase();
    let haystack = format!("{provider} {npm} {model} {api} {name} {family}");

    if provider == crate::model::extensions::commandcode::PROVIDER_ID {
        return commandcode_capability(&haystack);
    }

    // models.dev `reasoning: true` means the model can emit thinking/reasoning
    // tokens. OpenCode still requires provider/model-specific variants before
    // exposing selectable effort controls.
    if has_reasoning_without_selectable_effort(&haystack) {
        return ReasoningCapability::Unsupported;
    }

    if haystack.contains("grok") {
        if haystack.contains("grok-3-mini") {
            return ReasoningCapability::effort(
                vec![ReasoningEffort::Low, ReasoningEffort::High],
                ReasoningEffort::High,
            );
        }
        return ReasoningCapability::Unsupported;
    }

    if provider == "openai" {
        return openai_capability(&api, release_date);
    }

    match npm.as_str() {
        "@ai-sdk/openai" => openai_capability(&api, release_date),
        "@ai-sdk/azure" => azure_capability(&api),
        "@ai-sdk/openai-compatible"
        | "@ai-sdk/cerebras"
        | "@ai-sdk/togetherai"
        | "@ai-sdk/xai"
        | "@ai-sdk/deepinfra"
        | "venice-ai-sdk-provider" => openai_compatible_capability(&api),
        "@ai-sdk/anthropic" | "@ai-sdk/google-vertex/anthropic" => anthropic_capability(&api),
        "ai-gateway-provider" => {
            if api.starts_with("openai/") {
                openai_capability(&api, release_date)
            } else {
                widely_supported_capability()
            }
        }
        "@ai-sdk/gateway" => {
            if haystack.contains("anthropic") || haystack.contains("claude") {
                anthropic_capability(&api)
            } else if haystack.contains("google") || haystack.contains("gemini") {
                google_capability(&api)
            } else {
                openai_compatible_capability(&api)
            }
        }
        "@ai-sdk/google" | "@ai-sdk/google-vertex" => google_capability(&api),
        "@ai-sdk/groq" => ReasoningCapability::effort(
            vec![
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            ReasoningEffort::Medium,
        ),
        "@ai-sdk/mistral" => mistral_capability(&api),
        _ if provider == "anthropic" || haystack.contains("claude") => anthropic_capability(&api),
        _ if provider == "google" || haystack.contains("gemini") => google_capability(&api),
        _ => ReasoningCapability::Unsupported,
    }
}

fn has_reasoning_without_selectable_effort(haystack: &str) -> bool {
    [
        "deepseek-chat",
        "deepseek-reasoner",
        "deepseek-r1",
        "deepseek-v3",
        "minimax",
        "glm",
        "kimi",
        "k2p",
        "mimo",
        "qwen",
        "big-pickle",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn widely_supported_capability() -> ReasoningCapability {
    ReasoningCapability::effort(
        vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        ReasoningEffort::Medium,
    )
}

fn commandcode_capability(haystack: &str) -> ReasoningCapability {
    if haystack.contains("deepseek-v4-pro")
        || haystack.contains("deepseek v4 pro")
        || haystack.contains("deepseek-v4-flash")
        || haystack.contains("deepseek v4 flash")
    {
        return ReasoningCapability::effort(
            vec![ReasoningEffort::High, ReasoningEffort::Max],
            ReasoningEffort::High,
        );
    }

    if haystack.contains("claude-sonnet-4-6")
        || haystack.contains("claude-sonnet-4.6")
        || haystack.contains("claude sonnet 4.6")
        || haystack.contains("claude-fable-5")
        || haystack.contains("claude fable 5")
        || haystack.contains("opus-4-6")
        || haystack.contains("opus-4.6")
        || haystack.contains("opus 4.6")
        || haystack.contains("opus-4-7")
        || haystack.contains("opus-4.7")
        || haystack.contains("opus 4.7")
    {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ],
            ReasoningEffort::High,
        );
    }

    if haystack.contains("gpt-5.4-mini") || haystack.contains("gpt 5.4 mini") {
        return widely_supported_capability();
    }

    if haystack.contains("gpt-5.5")
        || haystack.contains("gpt 5.5")
        || haystack.contains("gpt-5.4")
        || haystack.contains("gpt 5.4")
        || haystack.contains("gpt-5.3-codex")
        || haystack.contains("gpt 5.3 codex")
    {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
            ],
            ReasoningEffort::Medium,
        );
    }

    if haystack.contains("gemini-3.5-flash")
        || haystack.contains("gemini 3.5 flash")
        || haystack.contains("gemini-3.1-flash-lite")
        || haystack.contains("gemini 3.1 flash lite")
    {
        return widely_supported_capability();
    }

    ReasoningCapability::Unsupported
}

fn openai_efforts(api_id: &str, release_date: &str) -> Vec<ReasoningEffort> {
    if api_id.contains("deep-research") {
        return vec![ReasoningEffort::Medium];
    }

    if is_gpt5_chat(api_id) {
        return if gpt5_version(api_id).is_some() {
            vec![ReasoningEffort::Medium]
        } else {
            Vec::new()
        };
    }

    if is_gpt5_versioned_pro(api_id) {
        return vec![
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ];
    }

    if is_gpt5_pro(api_id) {
        return vec![ReasoningEffort::High];
    }

    if let Some(codex_efforts) = gpt5_codex_efforts(api_id) {
        return codex_efforts;
    }

    if let Some(versioned_efforts) = versioned_gpt5_efforts(api_id) {
        return versioned_efforts;
    }

    let mut efforts = vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];
    if is_gpt5_family(api_id) {
        efforts.insert(0, ReasoningEffort::Minimal);
    }
    if release_date >= "2025-11-13" {
        efforts.insert(0, ReasoningEffort::None);
    }
    if release_date >= "2025-12-04" {
        efforts.push(ReasoningEffort::XHigh);
    }
    efforts
}

fn openai_capability(api_id: &str, release_date: &str) -> ReasoningCapability {
    let efforts = openai_efforts(api_id, release_date);
    if efforts.is_empty() {
        ReasoningCapability::Unsupported
    } else {
        ReasoningCapability::effort(efforts, ReasoningEffort::Medium)
    }
}

fn openai_compatible_capability(api_id: &str) -> ReasoningCapability {
    if is_gpt5_chat(api_id) {
        return if gpt5_version(api_id).is_some() {
            ReasoningCapability::effort(vec![ReasoningEffort::Medium], ReasoningEffort::Medium)
        } else {
            ReasoningCapability::Unsupported
        };
    }

    if is_gpt5_versioned_pro(api_id) {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
            ],
            ReasoningEffort::Medium,
        );
    }

    if is_gpt5_pro(api_id) {
        return ReasoningCapability::effort(vec![ReasoningEffort::High], ReasoningEffort::High);
    }

    if let Some(codex_efforts) = gpt5_codex_efforts(api_id) {
        return ReasoningCapability::effort(codex_efforts, ReasoningEffort::Medium);
    }

    if let Some(versioned_efforts) = versioned_gpt5_efforts(api_id) {
        return ReasoningCapability::effort(versioned_efforts, ReasoningEffort::Medium);
    }

    if api_id.contains("deepseek-v4") {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ],
            ReasoningEffort::Medium,
        );
    }

    ReasoningCapability::effort(
        vec![
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ],
        ReasoningEffort::Medium,
    )
}

fn azure_capability(api_id: &str) -> ReasoningCapability {
    let mut efforts = vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];
    if is_gpt5_family(api_id) && gpt5_version(api_id).is_none() {
        efforts.insert(0, ReasoningEffort::Minimal);
    }
    ReasoningCapability::effort(efforts, ReasoningEffort::Medium)
}

fn is_gpt5_family(api_id: &str) -> bool {
    api_id == "gpt-5"
        || api_id.starts_with("gpt-5.")
        || api_id.starts_with("gpt-5-")
        || api_id.ends_with("/gpt-5")
        || api_id.contains("/gpt-5.")
        || api_id.contains("/gpt-5-")
}

fn gpt5_version(api_id: &str) -> Option<u32> {
    let id = api_id.strip_prefix("openai/").unwrap_or(api_id);
    let rest = id
        .strip_prefix("gpt-5.")
        .or_else(|| id.strip_prefix("gpt-5-"))?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn is_gpt5_pro(api_id: &str) -> bool {
    api_id == "gpt-5-pro"
        || api_id.starts_with("gpt-5-pro.")
        || api_id.starts_with("gpt-5-pro-")
        || api_id.ends_with("/gpt-5-pro")
        || api_id.contains("/gpt-5-pro.")
        || api_id.contains("/gpt-5-pro-")
}

fn is_gpt5_versioned_pro(api_id: &str) -> bool {
    is_gpt5_family(api_id) && gpt5_version(api_id).is_some() && api_id.contains("pro")
}

fn is_gpt5_chat(api_id: &str) -> bool {
    is_gpt5_family(api_id) && api_id.contains("-chat")
}

fn versioned_gpt5_efforts(api_id: &str) -> Option<Vec<ReasoningEffort>> {
    let version = gpt5_version(api_id)?;
    if version == 1 {
        Some(vec![
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ])
    } else {
        Some(vec![
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ])
    }
}

fn gpt5_codex_efforts(api_id: &str) -> Option<Vec<ReasoningEffort>> {
    if !is_gpt5_family(api_id) || !api_id.contains("codex") {
        return None;
    }

    let version = gpt5_version(api_id);
    if version.is_some_and(|version| version >= 3) {
        return Some(vec![
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ]);
    }

    if api_id.contains("codex-max") || version.is_some_and(|version| version >= 2) {
        return Some(vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ]);
    }

    Some(vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ])
}

fn anthropic_capability(api_id: &str) -> ReasoningCapability {
    if api_id.contains("opus-4-7") || api_id.contains("opus-4.7") {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ],
            ReasoningEffort::High,
        );
    }

    if api_id.contains("opus-4-6")
        || api_id.contains("opus-4.6")
        || api_id.contains("sonnet-4-6")
        || api_id.contains("sonnet-4.6")
    {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ],
            ReasoningEffort::High,
        );
    }

    if api_id.contains("opus-4-5") || api_id.contains("opus-4.5") {
        return widely_supported_capability();
    }

    ReasoningCapability::effort(
        vec![ReasoningEffort::High, ReasoningEffort::Max],
        ReasoningEffort::High,
    )
}

fn google_capability(api_id: &str) -> ReasoningCapability {
    if api_id.contains("2.5") {
        return ReasoningCapability::effort(
            vec![ReasoningEffort::High, ReasoningEffort::Max],
            ReasoningEffort::High,
        );
    }

    if !api_id.contains("gemini-3") {
        return ReasoningCapability::effort(
            vec![ReasoningEffort::Low, ReasoningEffort::High],
            ReasoningEffort::High,
        );
    }

    if api_id.contains("flash-image") {
        return ReasoningCapability::effort(
            vec![ReasoningEffort::Minimal, ReasoningEffort::High],
            ReasoningEffort::High,
        );
    }

    if api_id.contains("pro-image") {
        return ReasoningCapability::effort(vec![ReasoningEffort::High], ReasoningEffort::High);
    }

    if api_id.contains("flash") {
        return ReasoningCapability::effort(
            vec![
                ReasoningEffort::Minimal,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            ReasoningEffort::Medium,
        );
    }

    widely_supported_capability()
}

fn mistral_capability(api_id: &str) -> ReasoningCapability {
    let reasoning_ids = [
        "mistral-small-2603",
        "mistral-small-latest",
        "mistral-medium-3.5",
        "mistral-medium-2604",
    ];
    if reasoning_ids.iter().any(|id| api_id.contains(id)) {
        ReasoningCapability::effort(vec![ReasoningEffort::High], ReasoningEffort::High)
    } else {
        ReasoningCapability::Unsupported
    }
}

fn generic_reasoning_capability() -> ReasoningCapability {
    widely_supported_capability()
}

pub fn parse_effort(value: &serde_json::Value) -> Option<ReasoningEffort> {
    value.as_str()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xhigh_aliases() {
        assert_eq!("xhigh".parse(), Ok(ReasoningEffort::XHigh));
        assert_eq!("extra-high".parse(), Ok(ReasoningEffort::XHigh));
        assert_eq!("extra_high".parse(), Ok(ReasoningEffort::XHigh));
    }

    #[test]
    fn generic_reasoning_cycles_supported_values() {
        let capability = generic_reasoning_capability();
        assert_eq!(capability.resolve(None), Some(ReasoningEffort::Medium));
        assert_eq!(
            capability.cycle_next(Some(ReasoningEffort::Medium)),
            Some(ReasoningEffort::High)
        );
        assert_eq!(
            capability.cycle_next(Some(ReasoningEffort::High)),
            Some(ReasoningEffort::Low)
        );
    }

    #[test]
    fn override_cycle_includes_no_override() {
        let capability = generic_reasoning_capability();
        assert_eq!(
            capability.cycle_override(None, 1),
            Some(Some(ReasoningEffort::Low))
        );
        assert_eq!(
            capability.cycle_override(Some(ReasoningEffort::High), 1),
            Some(None)
        );
        assert_eq!(
            capability.cycle_override(None, -1),
            Some(Some(ReasoningEffort::High))
        );
    }

    #[test]
    fn downgrades_to_nearest_supported_effort() {
        let capability = generic_reasoning_capability();
        assert_eq!(
            capability.resolve(Some(ReasoningEffort::XHigh)),
            Some(ReasoningEffort::High)
        );
        assert_eq!(capability.resolve(Some(ReasoningEffort::None)), None);
        assert_eq!(
            capability.cycle_next(Some(ReasoningEffort::None)),
            Some(ReasoningEffort::High)
        );
    }

    #[test]
    fn unsupported_models_have_no_cycle() {
        let capability = capability_for_model(
            "openai",
            "@ai-sdk/openai",
            "gpt-4o",
            "gpt-4o",
            "GPT-4o",
            "",
            "",
            false,
        );
        assert_eq!(capability.resolve(None), None);
        assert_eq!(capability.cycle_next(None), None);
    }

    #[test]
    fn reasoning_true_is_not_enough_for_selectable_effort() {
        let capability = capability_for_model(
            "opencode-go",
            "@ai-sdk/openai-compatible",
            "kimi-k2.6",
            "kimi-k2.6",
            "Kimi K2.6",
            "kimi-k2.6",
            "",
            true,
        );
        assert_eq!(capability.values(), &[]);
        assert_eq!(capability.cycle_next(None), None);
    }

    #[test]
    fn models_dev_effort_options_take_precedence_over_heuristics() {
        let capability = capability_for_model_with_options(
            "xai",
            "@ai-sdk/xai",
            "grok-4.5",
            "grok-4.5",
            "Grok 4.5",
            "grok",
            "",
            true,
            &[ReasoningOption {
                kind: "effort".to_string(),
                values: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
            }],
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        );
        assert_eq!(capability.resolve(None), Some(ReasoningEffort::Medium));
    }

    #[test]
    fn empty_models_dev_effort_options_fall_back_to_existing_rules() {
        let capability = capability_for_model_with_options(
            "xai",
            "@ai-sdk/xai",
            "grok-4.5",
            "grok-4.5",
            "Grok 4.5",
            "grok",
            "",
            true,
            &[],
        );
        assert_eq!(capability.values(), &[]);
    }

    #[test]
    fn mimo_reasoning_has_no_selectable_effort() {
        let capability = capability_for_model(
            "xiaomi-token-plan-sgp",
            "@ai-sdk/openai-compatible",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "Mimo V2.5 Pro",
            "mimo-v2.5-pro",
            "",
            true,
        );
        assert_eq!(capability.values(), &[]);
        assert_eq!(capability.cycle_next(None), None);
    }

    #[test]
    fn deepseek_v4_reasoning_includes_max() {
        let capability = capability_for_model(
            "deepseek",
            "@ai-sdk/openai-compatible",
            "deepseek-v4-pro",
            "deepseek-v4-pro",
            "DeepSeek V4 Pro",
            "",
            "",
            true,
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Max,
            ]
        );
    }

    #[test]
    fn commandcode_deepseek_v4_uses_observed_efforts() {
        let capability = capability_for_model(
            "commandcode",
            "@ai-sdk/openai-compatible",
            "deepseek/deepseek-v4-pro",
            "deepseek/deepseek-v4-pro",
            "DeepSeek V4 Pro",
            "deepseek",
            "",
            true,
        );
        assert_eq!(
            capability.values(),
            &[ReasoningEffort::High, ReasoningEffort::Max]
        );
    }

    #[test]
    fn commandcode_claude_uses_observed_efforts() {
        let capability = capability_for_model(
            "commandcode",
            "@ai-sdk/anthropic",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6",
            "claude",
            "",
            true,
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ]
        );
    }

    #[test]
    fn commandcode_gpt_5_4_mini_uses_observed_efforts() {
        let capability = capability_for_model(
            "commandcode",
            "@ai-sdk/openai-compatible",
            "openai/gpt-5.4-mini",
            "openai/gpt-5.4-mini",
            "GPT-5.4 Mini",
            "gpt",
            "",
            true,
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        );
    }

    #[test]
    fn commandcode_gpt_5_5_uses_observed_efforts() {
        let capability = capability_for_model(
            "commandcode",
            "@ai-sdk/openai-compatible",
            "openai/gpt-5.5",
            "openai/gpt-5.5",
            "GPT-5.5",
            "gpt",
            "",
            true,
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
            ]
        );
    }

    #[test]
    fn commandcode_gemini_flash_uses_observed_efforts() {
        let capability = capability_for_model(
            "commandcode",
            "@ai-sdk/openai-compatible",
            "google/gemini-3.1-flash-lite",
            "google/gemini-3.1-flash-lite",
            "Gemini 3.1 Flash Lite",
            "gemini",
            "",
            true,
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        );
    }

    #[test]
    fn gpt_5_3_codex_spark_uses_opencode_style_efforts() {
        let capability = capability_for_model(
            "openai",
            "@ai-sdk/openai",
            "gpt-5.3-codex-spark",
            "gpt-5.3-codex-spark",
            "GPT-5.3 Codex Spark",
            "",
            "2026-01-01",
            true,
        );
        assert_eq!(
            capability.values(),
            &[
                ReasoningEffort::None,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
            ]
        );
    }
}
