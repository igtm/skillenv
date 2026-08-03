//! The Agent Skills open standard — `.agents/skills`.
//!
//! This directory is not Codex-specific, which is how v0 treated it. The
//! `.agents/.skill-lock.json` on this machine lists fourteen tools that read it,
//! so it is its own provider and the lowest common denominator: it carries only
//! what the published specification defines.

use super::{Provider, ToolsStyle};
use crate::manifest::TargetScope;

pub(super) struct Agents;

impl Provider for Agents {
    fn relative_root(&self, _scope: TargetScope) -> &'static str {
        ".agents/skills"
    }

    fn extra_keys(&self) -> &'static [&'static str] {
        &["license", "allowed-tools", "metadata", "compatibility"]
    }

    fn tools_style(&self) -> ToolsStyle {
        ToolsStyle::SpaceSeparated
    }
}
