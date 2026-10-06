//! Runtime policy supplied by account-backed capabilities, not parsed from user config.
//! MCP owns transport and lifecycle; capability adapters own recovery and authorization.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKind {
    Consent,
    Authorization,
    Unavailable,
}

impl RecoveryKind {
    pub fn status(self) -> &'static str {
        match self {
            Self::Consent => "needs_consent",
            Self::Authorization => "needs_auth",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub kind: RecoveryKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRule {
    /// Case-insensitive error markers. First matching rule wins.
    pub markers: Vec<String>,
    pub recovery: Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationAction {
    pub url: String,
    pub instructions: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpCapabilityPolicy {
    pub authorization: Option<AuthorizationAction>,
    pub recovery_rules: Vec<RecoveryRule>,
}

impl McpCapabilityPolicy {
    pub fn recovery(&self, message: &str) -> Option<Recovery> {
        let message = message.to_ascii_lowercase();
        self.recovery_rules
            .iter()
            .find(|rule| {
                rule.markers
                    .iter()
                    .any(|marker| message.contains(&marker.to_ascii_lowercase()))
            })
            .map(|rule| rule.recovery.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_case_insensitive_recovery_rules_are_provider_agnostic() {
        let policy = McpCapabilityPolicy {
            authorization: None,
            recovery_rules: vec![
                RecoveryRule {
                    markers: vec!["consent_required".into()],
                    recovery: Recovery {
                        kind: RecoveryKind::Consent,
                        message: "Grant access in account settings".into(),
                    },
                },
                RecoveryRule {
                    markers: vec!["401".into()],
                    recovery: Recovery {
                        kind: RecoveryKind::Authorization,
                        message: "Sign in".into(),
                    },
                },
            ],
        };
        assert_eq!(
            policy.recovery("CONSENT_REQUIRED 401").unwrap().kind,
            RecoveryKind::Consent
        );
        assert_eq!(
            policy.recovery("HTTP 401").unwrap().kind.status(),
            "needs_auth"
        );
        assert!(policy.recovery("socket closed").is_none());
        assert!(McpCapabilityPolicy::default().recovery("401").is_none());
    }
}
