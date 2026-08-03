//! opencode.
//!
//! opencode also reads `.claude/skills` and `.agents/skills`, so deploying to it
//! directly is only needed when a skill should be visible to opencode and not to
//! the tools sharing those directories.

use super::{Provider, ProviderId, ToolsStyle};
use crate::manifest::TargetScope;

pub(super) struct Opencode;

impl Provider for Opencode {
    fn id(&self) -> ProviderId {
        ProviderId::Opencode
    }

    fn relative_root(&self, _scope: TargetScope) -> &'static str {
        ".opencode/skills"
    }

    fn extra_keys(&self) -> &'static [&'static str] {
        &["license", "allowed-tools", "metadata", "compatibility"]
    }

    fn tools_style(&self) -> ToolsStyle {
        // Emitted as a sequence, matching what rulesync writes for opencode.
        ToolsStyle::Sequence
    }
}
