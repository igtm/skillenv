//! Codex CLI.
//!
//! Two things make Codex different from the others.
//!
//! **The directory.** Codex reads `$CODEX_HOME/skills`, defaulting to
//! `~/.codex/skills`. v0 mapped its "codex" concept onto `.agents/skills`, which
//! is the open standard's shared directory — so a codex target wrote where many
//! tools read and never where Codex itself looks.
//!
//! **The sidecar.** Codex's own authoring guidance is explicit that the
//! frontmatter carries `name` and `description` and nothing else, and that
//! product-specific configuration belongs in `agents/openai.yaml`, described as
//! "an extended, product-specific config intended for the machine/harness to
//! read, not the agent". So instead of dropping presentation metadata, we move it
//! there.

use std::env;
use std::path::PathBuf;

use serde_yaml::{Mapping, Value};

use super::{CanonicalSkill, Provider, ProviderId, Sidecar, TargetContext, ToolsStyle};
use crate::manifest::TargetScope;
use crate::{Result, SkillenvError};

pub(super) struct Codex;

impl Provider for Codex {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn relative_root(&self, _scope: TargetScope) -> &'static str {
        ".codex/skills"
    }

    /// Deliberately shorter than the others: Codex's validator rejects
    /// `compatibility`, and its authoring guidance asks for nothing beyond the
    /// two required keys plus these.
    fn extra_keys(&self) -> &'static [&'static str] {
        &["license", "allowed-tools", "metadata"]
    }

    fn tools_style(&self) -> ToolsStyle {
        ToolsStyle::SpaceSeparated
    }

    /// Honour `$CODEX_HOME` when it is set, since that is what Codex itself does.
    fn write_root(&self, scope: TargetScope, ctx: &TargetContext) -> Result<PathBuf> {
        match scope {
            TargetScope::Home => Ok(match env::var_os("CODEX_HOME") {
                Some(codex_home) => PathBuf::from(codex_home).join("skills"),
                None => ctx.home.join(self.relative_root(scope)),
            }),
            TargetScope::Repo => {
                let repo_root = ctx.repo_root.clone().ok_or(SkillenvError::RepoRequired)?;
                Ok(repo_root.join(self.relative_root(scope)))
            }
        }
    }

    fn sidecars(&self, skill: &CanonicalSkill) -> Vec<Sidecar> {
        match openai_yaml(skill) {
            Some(contents) => vec![Sidecar {
                relative_path: PathBuf::from("agents/openai.yaml"),
                contents,
            }],
            None => Vec::new(),
        }
    }
}

/// Build `agents/openai.yaml` from the canonical skill's presentation metadata.
///
/// Returns `None` when there is nothing to say, so a plain skill does not gain an
/// empty configuration file.
fn openai_yaml(skill: &CanonicalSkill) -> Option<String> {
    let metadata = skill.extra.get("metadata")?.as_mapping()?;

    let mut interface = Mapping::new();
    // `short-description` in frontmatter metadata is the same idea as the
    // sidecar's `short_description`, so carry it across rather than losing it.
    for (from, to) in [
        ("short-description", "short_description"),
        ("display-name", "display_name"),
        ("brand-color", "brand_color"),
    ] {
        if let Some(value) = metadata.get(Value::String(from.to_string())) {
            interface.insert(Value::String(to.to_string()), value.clone());
        }
    }

    if interface.is_empty() {
        return None;
    }

    let mut root = Mapping::new();
    root.insert(
        Value::String("interface".to_string()),
        Value::Mapping(interface),
    );
    serde_yaml::to_string(&Value::Mapping(root)).ok()
}
