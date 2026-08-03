//! Per-provider translation: one canonical skill, several frontmatter shapes.
//!
//! Only `name` and `description` are common to every agent tool. Everything else
//! differs, and the differences are enforced: two official validators ship on
//! disk with different allowed-key sets — Claude's accepts `compatibility`,
//! Codex's rejects it — and `allowed-tools` appears in four mutually
//! incompatible serializations in the wild (space-separated string, comma
//! string, inline sequence, block sequence).
//!
//! v0 had no provider concept at all. `render_skill_markdown` took no target
//! argument, so `.agents/skills` and `.claude/skills` received byte-identical
//! files and a provider-specific key could only be right for one of them by
//! accident.
//!
//! Nothing calls the write side yet — `deploy` is the first consumer, and this
//! allow goes away with it.
#![allow(dead_code)]

mod agents;
mod claude;
mod codex;
mod opencode;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};

use crate::manifest::{DeployRule, SkillId, TargetRef, TargetScope};
use crate::paths::normalize_path;
use crate::render::mapping_to_yaml;
use crate::{Result, SkillenvError};

/// Longest frontmatter `name` both official validators accept.
pub(crate) const MAX_NAME_CHARS: usize = 64;
/// Longest `description` both official validators accept.
pub(crate) const MAX_DESCRIPTION_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderId {
    /// Claude Code — `.claude/skills`.
    Claude,
    /// The Agent Skills open standard — `.agents/skills`. Read by many tools,
    /// which is why it is its own provider rather than an alias for Codex as v0
    /// treated it.
    Agents,
    /// Codex CLI — `$CODEX_HOME/skills`, plus an `agents/openai.yaml` sidecar for
    /// anything the frontmatter may not carry.
    Codex,
    /// opencode — `.opencode/skills`.
    Opencode,
}

impl ProviderId {
    pub fn all() -> [Self; 4] {
        [Self::Claude, Self::Agents, Self::Codex, Self::Opencode]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Agents => "agents",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::all().into_iter().find(|id| id.as_str() == raw)
    }

    fn provider(self) -> &'static dyn Provider {
        match self {
            Self::Claude => &claude::Claude,
            Self::Agents => &agents::Agents,
            Self::Codex => &codex::Codex,
            Self::Opencode => &opencode::Opencode,
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the two scopes resolve to on this machine.
#[derive(Debug, Clone)]
pub struct TargetContext {
    pub home: PathBuf,
    /// The repository being linked, when there is one.
    pub repo_root: Option<PathBuf>,
}

pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Directory this provider reads, relative to the scope root.
    fn relative_root(&self, scope: TargetScope) -> &'static str;

    /// Frontmatter keys this provider's validator accepts beyond `name` and
    /// `description`.
    fn extra_keys(&self) -> &'static [&'static str];

    /// How this provider expects `allowed-tools` to be serialized.
    fn tools_style(&self) -> ToolsStyle {
        ToolsStyle::SpaceSeparated
    }

    /// Files written alongside `SKILL.md`, for providers that keep their
    /// configuration outside the frontmatter.
    fn sidecars(&self, _skill: &CanonicalSkill) -> Vec<Sidecar> {
        Vec::new()
    }

    /// Absolute directory for a scope, or an explanation of why there is none.
    fn write_root(&self, scope: TargetScope, ctx: &TargetContext) -> Result<PathBuf> {
        let base = match scope {
            TargetScope::Home => ctx.home.clone(),
            TargetScope::Repo => ctx.repo_root.clone().ok_or(SkillenvError::RepoRequired)?,
        };
        Ok(base.join(self.relative_root(scope)))
    }
}

/// How a provider serializes `allowed-tools`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsStyle {
    /// `allowed-tools: Read Write Bash`
    SpaceSeparated,
    /// `allowed-tools: [Read, Write, Bash]`
    Sequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub relative_path: PathBuf,
    pub contents: String,
}

/// A skill in the form skillenv reasons about, before any provider sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalSkill {
    pub id: SkillId,
    pub description: String,
    /// Everything after the frontmatter, passed through untouched.
    pub body: String,
    /// Frontmatter keys other than `name` and `description`, as parsed.
    pub extra: BTreeMap<String, Value>,
}

/// What one provider produced for one skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSkill {
    pub skill_md: String,
    pub sidecars: Vec<Sidecar>,
    /// Keys dropped because this provider's validator would reject them.
    /// Surfaced rather than silently discarded so a user can see why a key they
    /// wrote did not survive.
    pub dropped_keys: Vec<String>,
}

/// A deploy target after provider resolution, with the rules that chose it.
///
/// Rules are grouped by resolved path, not by `provider:scope`, because that
/// mapping is not injective — several providers can name the same directory, and
/// two rules writing to one directory would otherwise fight on every run.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub path: PathBuf,
    /// Every `provider:scope` that resolved here, sorted.
    pub refs: Vec<TargetRef>,
    /// The provider whose rendering is used. When several share a directory the
    /// most restrictive one wins, so the output satisfies all of them.
    pub render_with: ProviderId,
}

/// Everything that resolved to one directory, while grouping is in progress.
#[derive(Debug, Default)]
struct TargetGroup {
    refs: BTreeSet<TargetRef>,
    providers: BTreeSet<ProviderId>,
    /// Indices into the caller's rule list, so selections can be unioned later.
    rule_indices: Vec<usize>,
}

/// Resolve deploy rules into distinct directories.
///
/// A rule carrying `when.repo` that does not match the repository in play is
/// skipped.
pub fn resolve_targets(
    rules: &[DeployRule],
    ctx: &TargetContext,
) -> Result<Vec<(ResolvedTarget, Vec<usize>)>> {
    let mut grouped: BTreeMap<PathBuf, TargetGroup> = BTreeMap::new();

    for (index, rule) in rules.iter().enumerate() {
        if !rule_applies_here(rule, ctx) {
            continue;
        }
        let Some(provider_id) = ProviderId::parse(&rule.target.provider) else {
            return Err(SkillenvError::UnknownProvider {
                name: rule.target.provider.clone(),
                known: ProviderId::all()
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        };
        let root = provider_id.provider().write_root(rule.target.scope, ctx)?;
        let entry = grouped.entry(normalize_path(&root)).or_default();
        entry.refs.insert(rule.target.clone());
        entry.providers.insert(provider_id);
        entry.rule_indices.push(index);
    }

    Ok(grouped
        .into_iter()
        .map(|(path, group)| {
            let render_with = most_restrictive(&group.providers);
            (
                ResolvedTarget {
                    path,
                    refs: group.refs.into_iter().collect(),
                    render_with,
                },
                group.rule_indices,
            )
        })
        .collect())
}

/// Whether a rule's `when.repo` matches the repository in play.
///
/// Absent means every repository. A trailing `/**` matches the subtree; `~` is
/// expanded so a manifest can be written the way a user thinks about paths.
fn rule_applies_here(rule: &DeployRule, ctx: &TargetContext) -> bool {
    let Some(pattern) = &rule.when_repo else {
        return true;
    };
    let Some(repo_root) = &ctx.repo_root else {
        return false;
    };
    let expanded = expand_home(pattern, &ctx.home);
    let repo = normalize_path(repo_root);

    match expanded.strip_suffix("/**") {
        Some(prefix) => repo.starts_with(normalize_path(Path::new(prefix))),
        None => repo == normalize_path(Path::new(&expanded)),
    }
}

fn expand_home(pattern: &str, home: &Path) -> String {
    match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest).to_string_lossy().to_string(),
        None => pattern.to_string(),
    }
}

/// The provider with the smallest allowed-key set, so a shared directory gets
/// output every provider reading it will accept.
fn most_restrictive(providers: &BTreeSet<ProviderId>) -> ProviderId {
    providers
        .iter()
        .copied()
        .min_by_key(|id| (id.provider().extra_keys().len(), *id))
        .unwrap_or(ProviderId::Agents)
}

/// Render one skill for one provider.
pub fn render_for(
    provider_id: ProviderId,
    skill: &CanonicalSkill,
    generated_name: &str,
) -> Result<RenderedSkill> {
    let provider = provider_id.provider();
    let allowed: BTreeSet<&str> = provider.extra_keys().iter().copied().collect();

    let mut frontmatter = Mapping::new();
    frontmatter.insert(
        Value::String("name".to_string()),
        Value::String(generated_name.to_string()),
    );
    frontmatter.insert(
        Value::String("description".to_string()),
        Value::String(skill.description.clone()),
    );

    let mut dropped = Vec::new();
    for (key, value) in &skill.extra {
        if !allowed.contains(key.as_str()) {
            dropped.push(key.clone());
            continue;
        }
        let value = if key == "allowed-tools" {
            serialize_tools(&normalize_allowed_tools(value), provider.tools_style())
        } else {
            value.clone()
        };
        frontmatter.insert(Value::String(key.clone()), value);
    }

    let yaml = mapping_to_yaml(&frontmatter)?;
    let separator = if skill.body.is_empty() || skill.body.starts_with('\n') {
        "\n"
    } else {
        "\n\n"
    };

    Ok(RenderedSkill {
        skill_md: format!("---\n{yaml}---{separator}{}", skill.body),
        sidecars: provider.sidecars(skill),
        dropped_keys: dropped,
    })
}

/// Flatten any of the four real-world `allowed-tools` shapes into a list.
///
/// A space-separated string and a comma-separated string both appear in
/// installed skills, as do inline and block sequences. Normalizing on the way in
/// means each provider can emit whichever form its validator expects.
pub fn normalize_allowed_tools(value: &Value) -> Vec<String> {
    match value {
        Value::String(raw) => raw
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        Value::Sequence(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(raw) => Some(raw.trim().to_string()),
                _ => None,
            })
            .filter(|part| !part.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn serialize_tools(tools: &[String], style: ToolsStyle) -> Value {
    match style {
        ToolsStyle::SpaceSeparated => Value::String(tools.join(" ")),
        ToolsStyle::Sequence => Value::Sequence(
            tools
                .iter()
                .map(|tool| Value::String(tool.clone()))
                .collect(),
        ),
    }
}

/// A problem with a skill that would make a provider reject it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
}

/// Check the constraints both official validators enforce.
///
/// `name` is the *generated* name, since that is what ends up in the file and
/// what the provider will measure.
pub fn validate(generated_name: &str, skill: &CanonicalSkill) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let note = |message: String| Diagnostic { message };

    let len = generated_name.chars().count();
    if len > MAX_NAME_CHARS {
        diagnostics.push(note(format!(
            "generated name is {len} characters; providers reject anything over \
             {MAX_NAME_CHARS}. Shorten the skill id."
        )));
    }
    if !generated_name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        diagnostics.push(note(format!(
            "generated name {generated_name:?} contains characters providers reject; \
             only lowercase ASCII, digits, and '-' are allowed"
        )));
    }

    if skill.description.trim().is_empty() {
        diagnostics.push(note(
            "description is empty; every provider requires one".to_string(),
        ));
    }
    let description_len = skill.description.chars().count();
    if description_len > MAX_DESCRIPTION_CHARS {
        diagnostics.push(note(format!(
            "description is {description_len} characters; providers reject anything over \
             {MAX_DESCRIPTION_CHARS}"
        )));
    }
    // Both validators refuse angle brackets in a description.
    if let Some(bad) = skill
        .description
        .chars()
        .find(|ch| *ch == '<' || *ch == '>')
    {
        diagnostics.push(note(format!(
            "description contains {bad:?}, which providers reject"
        )));
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, Selector};

    fn ctx() -> TargetContext {
        TargetContext {
            home: PathBuf::from("/home/u"),
            repo_root: Some(PathBuf::from("/work/dotfiles")),
        }
    }

    fn skill(description: &str, extra: &[(&str, Value)]) -> CanonicalSkill {
        CanonicalSkill {
            id: SkillId::parse("kinko").unwrap(),
            description: description.to_string(),
            body: "Body text\n".to_string(),
            extra: extra
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    fn rules(toml: &str) -> Vec<DeployRule> {
        Manifest::parse(toml, Path::new("skillenv.toml"))
            .expect("test manifest should parse")
            .deploys
    }

    #[test]
    fn each_provider_resolves_its_own_directories() -> Result<()> {
        let ctx = ctx();
        for (id, home, repo) in [
            (ProviderId::Claude, ".claude/skills", ".claude/skills"),
            (ProviderId::Agents, ".agents/skills", ".agents/skills"),
            (ProviderId::Opencode, ".opencode/skills", ".opencode/skills"),
        ] {
            let provider = id.provider();
            assert_eq!(
                provider.write_root(TargetScope::Home, &ctx)?,
                PathBuf::from("/home/u").join(home),
                "{id} home root"
            );
            assert_eq!(
                provider.write_root(TargetScope::Repo, &ctx)?,
                PathBuf::from("/work/dotfiles").join(repo),
                "{id} repo root"
            );
        }
        Ok(())
    }

    /// Codex keeps its skills under CODEX_HOME, not in `.agents/skills`. v0
    /// conflated the two, so a "codex" target wrote where the open standard lives
    /// and never where Codex actually reads.
    #[test]
    fn codex_resolves_to_its_own_home() -> Result<()> {
        let root = ProviderId::Codex
            .provider()
            .write_root(TargetScope::Home, &ctx())?;
        assert_eq!(root, PathBuf::from("/home/u/.codex/skills"));
        Ok(())
    }

    #[test]
    fn a_repo_scope_without_a_repo_is_an_error() {
        let ctx = TargetContext {
            home: PathBuf::from("/home/u"),
            repo_root: None,
        };
        let error = ProviderId::Claude
            .provider()
            .write_root(TargetScope::Repo, &ctx)
            .unwrap_err();
        assert!(matches!(error, SkillenvError::RepoRequired));
    }

    #[test]
    fn an_unknown_provider_names_the_ones_that_exist() {
        let error = resolve_targets(
            &rules("[[deploy]]\ntarget = \"gemini:home\"\ninclude = [\"*\"]\n"),
            &ctx(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("gemini"), "unexpected error: {error}");
        assert!(error.contains("claude"), "should list known: {error}");
    }

    /// Two rules naming the same directory must become one target, or they would
    /// each remove the other's entries on every run.
    #[test]
    fn rules_sharing_a_directory_collapse_into_one_target() -> Result<()> {
        // Claude and opencode both read `.claude/skills` in the real layout, so
        // the clearest way to force a collision is two rules on one provider.
        let resolved = resolve_targets(
            &rules(
                "[[deploy]]\ntarget = \"claude:home\"\ninclude = [\"writing\"]\n\
                 [[deploy]]\ntarget = \"claude:home\"\ninclude = [\"tools\"]\n",
            ),
            &ctx(),
        )?;
        assert_eq!(resolved.len(), 1, "expected one directory: {resolved:?}");
        // Both rules are attributed to it, so their selections can be unioned.
        assert_eq!(resolved[0].1, vec![0, 1]);
        Ok(())
    }

    #[test]
    fn distinct_providers_stay_distinct_targets() -> Result<()> {
        let resolved = resolve_targets(
            &rules(
                "[[deploy]]\ntarget = \"claude:home\"\ninclude = [\"*\"]\n\
                 [[deploy]]\ntarget = \"agents:home\"\ninclude = [\"*\"]\n",
            ),
            &ctx(),
        )?;
        assert_eq!(resolved.len(), 2);
        Ok(())
    }

    #[test]
    fn when_repo_limits_a_rule_to_one_repository() -> Result<()> {
        let toml = "[[deploy]]\ntarget = \"claude:repo\"\ninclude = [\"*\"]\n\
                    when.repo = \"/work/other\"\n";
        assert!(resolve_targets(&rules(toml), &ctx())?.is_empty());

        let toml = "[[deploy]]\ntarget = \"claude:repo\"\ninclude = [\"*\"]\n\
                    when.repo = \"/work/dotfiles\"\n";
        assert_eq!(resolve_targets(&rules(toml), &ctx())?.len(), 1);
        Ok(())
    }

    #[test]
    fn when_repo_supports_a_subtree_and_tilde() -> Result<()> {
        let toml = "[[deploy]]\ntarget = \"claude:repo\"\ninclude = [\"*\"]\n\
                    when.repo = \"/work/**\"\n";
        assert_eq!(resolve_targets(&rules(toml), &ctx())?.len(), 1);

        let ctx = TargetContext {
            home: PathBuf::from("/home/u"),
            repo_root: Some(PathBuf::from("/home/u/tmp/kaijin-web")),
        };
        let toml = "[[deploy]]\ntarget = \"claude:repo\"\ninclude = [\"*\"]\n\
                    when.repo = \"~/tmp/**\"\n";
        assert_eq!(resolve_targets(&rules(toml), &ctx)?.len(), 1);
        Ok(())
    }

    #[test]
    fn rendering_sets_the_generated_name_and_keeps_the_body() -> Result<()> {
        let rendered = render_for(
            ProviderId::Claude,
            &skill("A skill", &[]),
            "skillenv-dotfiles-kinko",
        )?;
        assert!(rendered.skill_md.starts_with("---\n"));
        assert!(rendered.skill_md.contains("name: skillenv-dotfiles-kinko"));
        assert!(rendered.skill_md.ends_with("Body text\n"));
        Ok(())
    }

    /// Claude accepts `compatibility`; Codex's validator rejects it. The same
    /// canonical skill therefore renders differently, which v0 could not express.
    #[test]
    fn compatibility_survives_for_claude_and_is_dropped_for_codex() -> Result<()> {
        let input = skill(
            "A skill",
            &[("compatibility", Value::String("Requires Node".to_string()))],
        );

        let claude = render_for(ProviderId::Claude, &input, "s")?;
        assert!(claude.skill_md.contains("compatibility"));
        assert!(claude.dropped_keys.is_empty());

        let codex = render_for(ProviderId::Codex, &input, "s")?;
        assert!(!codex.skill_md.contains("compatibility"));
        assert_eq!(codex.dropped_keys, vec!["compatibility".to_string()]);
        Ok(())
    }

    #[test]
    fn allowed_tools_normalizes_from_every_real_world_shape() {
        let expected = vec!["Read".to_string(), "Write".to_string(), "Bash".to_string()];
        for value in [
            Value::String("Read Write Bash".to_string()),
            Value::String("Read, Write, Bash".to_string()),
            Value::Sequence(vec![
                Value::String("Read".to_string()),
                Value::String("Write".to_string()),
                Value::String("Bash".to_string()),
            ]),
        ] {
            assert_eq!(normalize_allowed_tools(&value), expected, "for {value:?}");
        }
    }

    #[test]
    fn allowed_tools_is_re_serialized_in_each_providers_style() -> Result<()> {
        let input = skill(
            "A skill",
            &[("allowed-tools", Value::String("Read, Write".to_string()))],
        );
        let claude = render_for(ProviderId::Claude, &input, "s")?;
        assert!(
            claude.skill_md.contains("allowed-tools: Read Write"),
            "got: {}",
            claude.skill_md
        );

        let opencode = render_for(ProviderId::Opencode, &input, "s")?;
        assert!(
            opencode.skill_md.contains("- Read"),
            "expected a sequence, got: {}",
            opencode.skill_md
        );
        Ok(())
    }

    /// Codex moves provider-specific configuration out of the frontmatter into a
    /// sidecar, and the user already has one of these checked in.
    #[test]
    fn codex_emits_its_sidecar_when_there_is_something_to_put_in_it() -> Result<()> {
        let plain = render_for(ProviderId::Codex, &skill("A skill", &[]), "s")?;
        assert!(plain.sidecars.is_empty(), "no sidecar without content");

        let mut metadata = Mapping::new();
        metadata.insert(
            Value::String("short-description".to_string()),
            Value::String("Store a secret".to_string()),
        );
        let input = skill("A skill", &[("metadata", Value::Mapping(metadata))]);
        let rendered = render_for(ProviderId::Codex, &input, "s")?;
        let sidecar = rendered
            .sidecars
            .iter()
            .find(|s| s.relative_path == Path::new("agents/openai.yaml"))
            .expect("expected an openai.yaml sidecar");
        assert!(
            sidecar.contents.contains("short_description"),
            "got: {}",
            sidecar.contents
        );
        Ok(())
    }

    #[test]
    fn validation_enforces_the_limits_both_validators_share() {
        let long_name = "a".repeat(MAX_NAME_CHARS + 1);
        let diagnostics = validate(&long_name, &skill("A skill", &[]));
        assert!(
            diagnostics[0].message.contains("64"),
            "unexpected: {diagnostics:?}"
        );

        let diagnostics = validate("s", &skill("", &[]));
        assert!(
            diagnostics.iter().any(|d| d.message.contains("empty")),
            "unexpected: {diagnostics:?}"
        );

        let diagnostics = validate("s", &skill("uses <html> tags", &[]));
        assert!(
            diagnostics.iter().any(|d| d.message.contains("reject")),
            "unexpected: {diagnostics:?}"
        );

        let long = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
        let diagnostics = validate("s", &skill(&long, &[]));
        assert!(
            diagnostics.iter().any(|d| d.message.contains("1024")),
            "unexpected: {diagnostics:?}"
        );

        assert!(validate("s", &skill("A fine description", &[])).is_empty());
    }

    #[test]
    fn a_selector_still_drives_which_rules_apply() {
        // Guards the seam between manifest selection and target resolution.
        let rule = &rules("[[deploy]]\ntarget = \"claude:home\"\ninclude = [\"writing\"]\n")[0];
        assert_eq!(rule.include, vec![Selector::Name("writing".to_string())]);
    }
}
