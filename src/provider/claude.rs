//! Claude Code.
//!
//! Allowed keys come from the validator shipped with the `skill-creator` skill:
//! `ALLOWED_PROPERTIES = {'name', 'description', 'license', 'allowed-tools',
//! 'metadata', 'compatibility'}`. Unknown keys are rejected outright, so anything
//! outside that set has to be dropped rather than passed through.

use super::{Provider, ProviderId, ToolsStyle};
use crate::manifest::TargetScope;

pub(super) struct Claude;

impl Provider for Claude {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn relative_root(&self, _scope: TargetScope) -> &'static str {
        ".claude/skills"
    }

    fn extra_keys(&self) -> &'static [&'static str] {
        &["license", "allowed-tools", "metadata", "compatibility"]
    }

    fn tools_style(&self) -> ToolsStyle {
        // Installed Claude skills use a space-separated string.
        ToolsStyle::SpaceSeparated
    }
}
