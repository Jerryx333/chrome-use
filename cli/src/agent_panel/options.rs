//! Conversation overrides are persisted locally and never edit CLI defaults.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ModelOptions {
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub tool_access: Option<ToolAccess>,
}

impl ModelOptions {
    /// Reject unsupported adapter options rather than silently dropping them.
    pub fn validate(&self, agent: &str) -> Result<(), String> {
        if self.tool_access == Some(ToolAccess::BrowserOnly) && agent == "codex" {
            return Err("Codex cannot remove its file-reading shell; choose readFiles".into());
        }
        if agent == "opencode"
            && self
                .tool_access
                .is_some_and(|mode| mode != ToolAccess::RunCommands)
        {
            return Err(
                "OpenCode ACP does not enforce builtin tool restrictions; use its CLI permissions"
                    .into(),
            );
        }
        if let Some(effort) = self.effort.as_deref() {
            let supported: &[&str] = match agent {
                "codex" => &["minimal", "low", "medium", "high", "xhigh", "max"],
                "claude" => &["low", "medium", "high", "max"],
                _ => return Err("This adapter has no conversation effort override".into()),
            };
            if !supported.contains(&effort) {
                return Err(
                    "Unsupported reasoning effort; use the CLI default or a supported level".into(),
                );
            }
        }
        if let Some(tier) = self.service_tier.as_deref() {
            if agent != "codex" || !["default", "priority"].contains(&tier) {
                return Err(
                    "Conversation service tier is supported only for Codex (default or priority)"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_overrides_are_not_silently_accepted() {
        let options = ModelOptions {
            effort: Some("high".into()),
            service_tier: Some("priority".into()),
            tool_access: None,
        };
        assert!(options.validate("codex").is_ok());
        assert!(options.validate("claude").is_err());
        assert!(options.validate("pi").is_err());
        assert!(ModelOptions::default().validate("opencode").is_ok());
        assert!(serde_json::from_str::<ModelOptions>(r#"{"unknown":true}"#).is_err());
    }
}

/// Ordered capabilities permit comparisons when a chat creates a schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ToolAccess {
    BrowserOnly,
    ReadFiles,
    WriteFiles,
    RunCommands,
}
impl ModelOptions {
    pub fn effective_access(&self, agent: &str, full_access: bool) -> ToolAccess {
        self.tool_access
            .unwrap_or(if full_access || agent == "opencode" {
                ToolAccess::RunCommands
            } else if agent == "codex" {
                ToolAccess::ReadFiles
            } else {
                ToolAccess::BrowserOnly
            })
    }
    pub fn sandbox(&self, agent: &str, full_access: bool) -> &'static str {
        match self.effective_access(agent, full_access) {
            ToolAccess::RunCommands => "danger-full-access",
            ToolAccess::WriteFiles => "workspace-write",
            _ => "read-only",
        }
    }
}
