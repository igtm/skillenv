//! `skillenv.toml` — the hand-written declaration of what exists and where it goes.
//!
//! The manifest records *intent*; `crate::lock` records what that intent
//! resolved to. Keeping them apart is deliberate: v0 stored both in one
//! `selected_skills` list, so "track everything from this source" became
//! indistinguishable from "these exact skills", and a source that renamed a
//! skill upstream could never be followed again.
//!
//! The module has no public entry point yet, so everything in it reads as dead
//! code until the CLI wires `Manifest::load` up. The allow goes away then.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Result, SkillenvError};

pub(crate) const MANIFEST_FILE: &str = "skillenv.toml";

/// Longest skill id we accept.
///
/// Providers cap the frontmatter `name` at 64 characters and a deployed skill is
/// named `skillenv-<repo>-g<hash>-<id>`. For a repository slug of typical length
/// the prefix costs 32 characters — `skillenv-` (9) + `dotfiles` (8) + `-g` (2) +
/// twelve hex digits + `-` — so 32 is what remains.
///
/// This is a static budget for early, actionable feedback; it cannot be exact,
/// because the prefix grows with the repository name. `crate::deploy` measures the
/// real generated name and skips a skill that would still overflow, so a long
/// repository name cannot smuggle an invalid file past us.
pub(crate) const MAX_SKILL_ID_CHARS: usize = 32;

/// Words an id may not take, because a selector or target would become ambiguous.
const RESERVED_IDS: &[&str] = &["all", "none", "skillenv", "local", "home", "repo"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub skills: Vec<SkillEntry>,
    pub sources: Vec<SourceEntry>,
    pub deploys: Vec<DeployRule>,
    pub safeguard: SafeguardConfig,
}

/// One directly-declared skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub id: SkillId,
    pub source: SourceSpec,
    /// Required when the source carries no frontmatter of its own, since every
    /// provider demands a description. Gist-hosted skills are the common case.
    pub description: Option<String>,
    pub labels: Vec<String>,
}

/// A source contributing one or more skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    pub name: String,
    pub from: SourceSpec,
    pub git_ref: Option<String>,
    pub skills: SkillSelection,
    pub labels: Vec<String>,
}

/// Where a skill's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    /// `skills/<id>/` inside the manifest's own directory.
    Local,
    /// `gist:<id>` — cloned as a git repository like any other.
    Gist(String),
    /// `github:owner/repo`
    GitHub { owner: String, repo: String },
    /// Any other git remote, passed through verbatim.
    Git(String),
    /// A path on this machine.
    Path(PathBuf),
}

/// Which skills to take from a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSelection {
    /// `skills = "*"` — follow whatever the source offers. The resolved set
    /// lives in the lock file, so this stays reproducible.
    All,
    /// An explicit list. A name that disappears upstream is reported per-skill
    /// rather than failing the whole command.
    Explicit(Vec<SkillId>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployRule {
    pub target: TargetRef,
    pub include: Vec<Selector>,
    pub exclude: Vec<Selector>,
    /// Restricts the rule to repositories whose path matches. Absent means
    /// "every repository", which is what a `home` target normally wants.
    pub when_repo: Option<String>,
}

/// `<provider>:<scope>`, e.g. `claude:home`. The provider is not resolved here;
/// `crate::provider` owns the mapping from provider id to an actual directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetRef {
    pub provider: String,
    pub scope: TargetScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetScope {
    /// Under `$HOME`, shared by every repository on the machine.
    Home,
    /// Inside the repository being linked.
    Repo,
}

/// A label matcher. `*` matches everything; anything else matches a skill's
/// label or its id, so a single skill can be named without inventing a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    All,
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeguardConfig {
    pub on_critical: Policy,
    pub on_high: Policy,
    pub on_medium: Policy,
    pub on_low: Policy,
    /// Suppressions, each bound to the content it was granted against.
    pub allow: Vec<AllowEntry>,
}

impl Default for SafeguardConfig {
    fn default() -> Self {
        Self {
            on_critical: Policy::Block,
            on_high: Policy::Warn,
            on_medium: Policy::Warn,
            on_low: Policy::Warn,
            allow: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Refuse to deploy the offending skill.
    Block,
    /// Deploy, but report on stderr and exit non-zero.
    Warn,
    /// Say nothing.
    Allow,
}

/// A suppression for one finding on one skill, pinned to a content digest.
///
/// The digest is what stops a suppression from silently outliving the content it
/// was reviewed against: change the skill and the allow no longer applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowEntry {
    pub code: String,
    pub skill: SkillId,
    pub digest: String,
}

/// A validated skill identifier.
///
/// Construction is the only way to get one, so an invalid id cannot reach the
/// rest of the crate. Comparison is case-insensitive because macOS is
/// case-insensitive by default: `Foo` and `foo` would pass a naive uniqueness
/// check and then collide on disk.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SkillId(String);

impl SkillId {
    pub fn parse(raw: &str) -> Result<Self> {
        let reason = validate_skill_id(raw);
        match reason {
            None => Ok(Self(raw.to_string())),
            Some(reason) => Err(SkillenvError::InvalidSkillId {
                input: raw.to_string(),
                reason,
            }),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key uniqueness is judged on. Ids are already lowercase, so this is
    /// the identity today; it exists so the rule survives any future relaxation
    /// of the character set.
    fn fold(&self) -> String {
        self.0.to_lowercase()
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserialization goes through `parse` rather than being derived, so a
/// hand-edited lock file cannot introduce an id the rest of the crate would
/// consider impossible.
impl<'de> Deserialize<'de> for SkillId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Why `raw` is not a usable id, or `None` if it is.
///
/// Non-ASCII input is rejected rather than transliterated. v0 ran names through
/// a slugifier that deleted non-ASCII characters and substituted a fallback
/// word, so `日本語ガイド` became the literal `skill` and two differently-named
/// Japanese skills silently became the same id.
fn validate_skill_id(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return Some("must not be empty".to_string());
    }
    if let Some(bad) = raw.chars().find(|ch| !ch.is_ascii()) {
        return Some(format!(
            "contains the non-ASCII character {bad:?}; give an explicit ASCII id \
             instead of relying on transliteration"
        ));
    }
    if let Some(bad) = raw
        .chars()
        .find(|ch| !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '-'))
    {
        return Some(format!(
            "contains {bad:?}; only lowercase ASCII letters, digits, and '-' are allowed"
        ));
    }
    if raw.starts_with('-') || raw.ends_with('-') {
        return Some("must not start or end with '-'".to_string());
    }
    if raw.contains("--") {
        return Some("must not contain consecutive hyphens".to_string());
    }
    let len = raw.chars().count();
    if len > MAX_SKILL_ID_CHARS {
        return Some(format!(
            "is {len} characters; the limit is {MAX_SKILL_ID_CHARS} so the deployed \
             name stays within the 64-character cap providers enforce"
        ));
    }
    if RESERVED_IDS.contains(&raw) {
        return Some(
            "is reserved; it would be ambiguous with a selector or target scope".to_string(),
        );
    }
    None
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).map_err(|source| SkillenvError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&raw, path)
    }

    pub fn parse(raw: &str, path: &Path) -> Result<Self> {
        let raw_manifest: RawManifest =
            toml_edit::de::from_str(raw).map_err(|source| SkillenvError::ParseManifest {
                path: path.to_path_buf(),
                source,
            })?;
        raw_manifest.into_manifest(path)
    }

    /// Every skill id the manifest declares directly. Ids contributed by a
    /// `[[source]]` with `skills = "*"` are only known after resolution, so they
    /// are not included.
    pub fn declared_ids(&self) -> Vec<&SkillId> {
        let mut ids: Vec<&SkillId> = self.skills.iter().map(|skill| &skill.id).collect();
        for source in &self.sources {
            if let SkillSelection::Explicit(names) = &source.skills {
                ids.extend(names.iter());
            }
        }
        ids
    }
}

impl Selector {
    /// Whether this selector picks a skill with `id` and `labels`.
    pub fn matches(&self, id: &SkillId, labels: &[String]) -> bool {
        match self {
            Selector::All => true,
            Selector::Name(name) => id.as_str() == name || labels.iter().any(|label| label == name),
        }
    }
}

impl DeployRule {
    /// Whether this rule wants a skill, i.e. some `include` matches and no
    /// `exclude` does.
    pub fn selects(&self, id: &SkillId, labels: &[String]) -> bool {
        let included = self
            .include
            .iter()
            .any(|selector| selector.matches(id, labels));
        if !included {
            return false;
        }
        !self
            .exclude
            .iter()
            .any(|selector| selector.matches(id, labels))
    }
}

impl fmt::Display for TargetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider, self.scope)
    }
}

impl fmt::Display for TargetScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TargetScope::Home => "home",
            TargetScope::Repo => "repo",
        })
    }
}

// --- wire format -----------------------------------------------------------
//
// Kept separate from the validated types above so parsing and validation are
// distinguishable: serde produces `Raw*`, and `into_manifest` is the only place
// that can turn one into a `Manifest`.

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    skillenv: RawMeta,
    #[serde(default, rename = "skill")]
    skills: Vec<RawSkill>,
    #[serde(default, rename = "source")]
    sources: Vec<RawSource>,
    #[serde(default, rename = "deploy")]
    deploys: Vec<RawDeploy>,
    #[serde(default)]
    safeguard: RawSafeguard,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    #[serde(default = "default_version")]
    version: u32,
}

/// Hand-written rather than derived: a derived `Default` would yield version 0,
/// and `#[serde(default = ...)]` on the field only applies when the `[skillenv]`
/// table is present. Omitting the table entirely has to mean version 1, not an
/// unsupported version 0.
impl Default for RawMeta {
    fn default() -> Self {
        Self {
            version: default_version(),
        }
    }
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSkill {
    name: String,
    source: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    name: String,
    from: String,
    #[serde(default, rename = "ref")]
    git_ref: Option<String>,
    #[serde(default)]
    skills: RawSelection,
    #[serde(default)]
    labels: Vec<String>,
}

/// `skills` accepts either `"*"` or a list of names.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawSelection {
    Wildcard(String),
    Explicit(Vec<String>),
}

impl Default for RawSelection {
    fn default() -> Self {
        RawSelection::Wildcard("*".to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeploy {
    target: String,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    when: Option<RawWhen>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWhen {
    #[serde(default)]
    repo: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSafeguard {
    on_critical: Option<Policy>,
    on_high: Option<Policy>,
    on_medium: Option<Policy>,
    on_low: Option<Policy>,
    #[serde(default)]
    allow: Vec<String>,
}

impl RawManifest {
    fn into_manifest(self, path: &Path) -> Result<Manifest> {
        let invalid = |message: String| SkillenvError::InvalidManifest {
            path: path.to_path_buf(),
            message,
        };

        if self.skillenv.version != 1 {
            return Err(invalid(format!(
                "unsupported manifest version {}; this build understands version 1",
                self.skillenv.version
            )));
        }

        let mut skills = Vec::new();
        for raw in self.skills {
            skills.push(SkillEntry {
                id: SkillId::parse(&raw.name)?,
                source: parse_source_spec(&raw.source)?,
                description: raw.description,
                labels: raw.labels,
            });
        }

        let mut sources = Vec::new();
        for raw in self.sources {
            let skills = match raw.skills {
                RawSelection::Wildcard(value) if value == "*" => SkillSelection::All,
                RawSelection::Wildcard(value) => {
                    return Err(invalid(format!(
                        "source '{}' has skills = {value:?}; use \"*\" for every skill \
                         or a list of names",
                        raw.name
                    )));
                }
                RawSelection::Explicit(names) => SkillSelection::Explicit(
                    names
                        .iter()
                        .map(|name| SkillId::parse(name))
                        .collect::<Result<Vec<_>>>()?,
                ),
            };
            sources.push(SourceEntry {
                name: raw.name,
                from: parse_source_spec(&raw.from)?,
                git_ref: raw.git_ref,
                skills,
                labels: raw.labels,
            });
        }

        let mut deploys = Vec::new();
        for raw in self.deploys {
            let include = if raw.include.is_empty() {
                // A rule with no include would silently deploy nothing, which
                // reads as a broken rule rather than an intentional one.
                return Err(invalid(format!(
                    "deploy rule for target '{}' has no include; use include = [\"*\"] \
                     to mean every skill",
                    raw.target
                )));
            } else {
                raw.include.iter().map(|s| parse_selector(s)).collect()
            };
            deploys.push(DeployRule {
                target: parse_target_ref(&raw.target).map_err(invalid)?,
                include,
                exclude: raw.exclude.iter().map(|s| parse_selector(s)).collect(),
                when_repo: raw.when.and_then(|when| when.repo),
            });
        }

        let defaults = SafeguardConfig::default();
        let safeguard = SafeguardConfig {
            on_critical: self.safeguard.on_critical.unwrap_or(defaults.on_critical),
            on_high: self.safeguard.on_high.unwrap_or(defaults.on_high),
            on_medium: self.safeguard.on_medium.unwrap_or(defaults.on_medium),
            on_low: self.safeguard.on_low.unwrap_or(defaults.on_low),
            allow: self
                .safeguard
                .allow
                .iter()
                .map(|entry| parse_allow_entry(entry).map_err(invalid))
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };

        let manifest = Manifest {
            version: self.skillenv.version,
            skills,
            sources,
            deploys,
            safeguard,
        };
        check_duplicate_ids(&manifest, path)?;
        Ok(manifest)
    }
}

/// Reject two declarations of the same id up front.
///
/// v0 keyed this on `(scope, id)` and only discovered a clash during `link`,
/// after the source had already been installed and the lock file written.
fn check_duplicate_ids(manifest: &Manifest, path: &Path) -> Result<()> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for id in manifest.declared_ids() {
        if let Some(first) = seen.insert(id.fold(), id.to_string()) {
            return Err(SkillenvError::InvalidManifest {
                path: path.to_path_buf(),
                message: format!(
                    "skill id '{id}' is declared twice (also as '{first}'); ids must be \
                     unique regardless of case, because case-insensitive filesystems \
                     would collide on disk"
                ),
            });
        }
    }
    Ok(())
}

fn parse_source_spec(raw: &str) -> Result<SourceSpec> {
    let invalid = |message: &str| {
        Err(SkillenvError::InvalidSource {
            input: raw.to_string(),
            message: message.to_string(),
        })
    };

    if raw == "local" {
        return Ok(SourceSpec::Local);
    }
    if let Some(id) = raw.strip_prefix("gist:") {
        if id.is_empty() {
            return invalid("gist source needs an id, e.g. gist:fd287c31…");
        }
        return Ok(SourceSpec::Gist(id.to_string()));
    }
    if let Some(slug) = raw.strip_prefix("github:") {
        let mut parts = slug.splitn(2, '/');
        return match (parts.next(), parts.next()) {
            (Some(owner), Some(repo)) if !owner.is_empty() && !repo.is_empty() => {
                Ok(SourceSpec::GitHub {
                    owner: owner.to_string(),
                    repo: repo.trim_end_matches(".git").to_string(),
                })
            }
            _ => invalid("github source must look like github:owner/repo"),
        };
    }
    if let Some(path) = raw.strip_prefix("path:") {
        if path.is_empty() {
            return invalid("path source needs a path");
        }
        return Ok(SourceSpec::Path(PathBuf::from(path)));
    }
    if raw.starts_with("git@")
        || raw.starts_with("ssh://")
        || raw.starts_with("https://")
        || raw.ends_with(".git")
    {
        return Ok(SourceSpec::Git(raw.to_string()));
    }
    invalid(
        "unrecognized source; use local, gist:<id>, github:owner/repo, path:<path>, \
         or a git URL",
    )
}

fn parse_selector(raw: &str) -> Selector {
    if raw == "*" {
        Selector::All
    } else {
        Selector::Name(raw.to_string())
    }
}

fn parse_target_ref(raw: &str) -> std::result::Result<TargetRef, String> {
    let Some((provider, scope)) = raw.split_once(':') else {
        return Err(format!(
            "target '{raw}' must look like <provider>:<scope>, e.g. claude:home"
        ));
    };
    if provider.is_empty() {
        return Err(format!("target '{raw}' has an empty provider"));
    }
    let scope = match scope {
        "home" => TargetScope::Home,
        "repo" => TargetScope::Repo,
        other => {
            return Err(format!(
                "target '{raw}' has scope '{other}'; expected 'home' or 'repo'"
            ));
        }
    };
    Ok(TargetRef {
        provider: provider.to_string(),
        scope,
    })
}

/// `<code>:<skill>:<digest>` — the digest is mandatory so a suppression cannot
/// outlive the content it was reviewed against.
fn parse_allow_entry(raw: &str) -> std::result::Result<AllowEntry, String> {
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(format!(
            "safeguard allow entry '{raw}' must look like <code>:<skill>:<digest>, \
             so the suppression stops applying when the content changes"
        ));
    }
    let skill = SkillId::parse(parts[1]).map_err(|error| error.to_string())?;
    if parts[0].is_empty() || parts[2].is_empty() {
        return Err(format!(
            "safeguard allow entry '{raw}' has an empty code or digest"
        ));
    }
    Ok(AllowEntry {
        code: parts[0].to_string(),
        skill,
        digest: parts[2].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Result<Manifest> {
        Manifest::parse(raw, Path::new("skillenv.toml"))
    }

    fn id(raw: &str) -> SkillId {
        SkillId::parse(raw).expect("test id should be valid")
    }

    #[test]
    fn parses_a_full_manifest() -> Result<()> {
        let manifest = parse(
            r#"
[skillenv]
version = 1

[[skill]]
name = "japanese-tech-writing"
source = "gist:fd287c3133457c4fd8f5601d34aa817d"
description = "日本語技術文書の文章規範"
labels = ["writing"]

[[skill]]
name = "draft-pr"
source = "local"

[[source]]
name = "igtm-skills"
from = "github:igtm/skills"
ref = "main"
skills = ["user-context"]
labels = ["tools"]

[[source]]
name = "vercel"
from = "github:vercel-labs/agent-skills"
skills = "*"

[[deploy]]
target = "claude:home"
include = ["*"]

[[deploy]]
target = "claude:repo"
include = ["writing"]
exclude = ["draft-pr"]
when.repo = "~/tmp/kaijin-web"

[safeguard]
on_high = "block"
allow = ["W012:draft-pr:sha256:abc"]
"#,
        )?;

        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.skills.len(), 2);
        assert_eq!(
            manifest.skills[0].source,
            SourceSpec::Gist("fd287c3133457c4fd8f5601d34aa817d".to_string())
        );
        assert_eq!(
            manifest.skills[0].description.as_deref(),
            Some("日本語技術文書の文章規範")
        );
        assert_eq!(manifest.skills[1].source, SourceSpec::Local);
        assert!(manifest.skills[1].labels.is_empty());

        assert_eq!(
            manifest.sources[0].skills,
            SkillSelection::Explicit(vec![id("user-context")])
        );
        assert_eq!(manifest.sources[0].git_ref.as_deref(), Some("main"));
        assert_eq!(manifest.sources[1].skills, SkillSelection::All);

        assert_eq!(manifest.deploys[0].target.to_string(), "claude:home");
        assert_eq!(manifest.deploys[1].target.scope, TargetScope::Repo);
        assert_eq!(
            manifest.deploys[1].when_repo.as_deref(),
            Some("~/tmp/kaijin-web")
        );

        assert_eq!(manifest.safeguard.on_high, Policy::Block);
        // Unset policies keep their defaults rather than becoming permissive.
        assert_eq!(manifest.safeguard.on_critical, Policy::Block);
        assert_eq!(manifest.safeguard.allow[0].code, "W012");
        assert_eq!(manifest.safeguard.allow[0].digest, "sha256:abc");
        Ok(())
    }

    #[test]
    fn an_empty_manifest_is_valid_and_blocks_critical_by_default() -> Result<()> {
        let manifest = parse("")?;
        assert_eq!(manifest.version, 1);
        assert!(manifest.skills.is_empty());
        assert_eq!(manifest.safeguard.on_critical, Policy::Block);
        Ok(())
    }

    #[test]
    fn rejects_an_unknown_key() {
        let error = parse("[[skill]]\nname = \"a\"\nsource = \"local\"\nlabel = [\"x\"]\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("label"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_a_future_manifest_version() {
        let error = parse("[skillenv]\nversion = 2\n").unwrap_err().to_string();
        assert!(error.contains("version 2"), "unexpected error: {error}");
    }

    /// Non-ASCII ids are refused rather than transliterated, because v0's
    /// slugifier turned every all-Japanese name into the same fallback word.
    #[test]
    fn rejects_a_non_ascii_id_and_says_what_to_do() {
        let error = SkillId::parse("日本語ガイド").unwrap_err().to_string();
        assert!(error.contains("non-ASCII"), "unexpected error: {error}");
        assert!(error.contains("explicit ASCII id"), "unexpected: {error}");
    }

    #[test]
    fn rejects_malformed_ids() {
        for (raw, expected) in [
            ("", "empty"),
            ("Foo", "'F'"),
            ("has space", "' '"),
            ("-lead", "start or end"),
            ("trail-", "start or end"),
            ("double--hyphen", "consecutive"),
            ("all", "reserved"),
            ("skillenv", "reserved"),
        ] {
            let error = SkillId::parse(raw).unwrap_err().to_string();
            assert!(
                error.contains(expected),
                "id {raw:?} gave {error:?}, expected it to mention {expected:?}"
            );
        }
    }

    /// The cap exists so the deployed name stays inside the 64-character limit
    /// both official validators enforce.
    #[test]
    fn rejects_an_id_longer_than_the_name_budget() {
        let long = "a".repeat(MAX_SKILL_ID_CHARS + 1);
        let error = SkillId::parse(&long).unwrap_err().to_string();
        assert!(error.contains("64-character"), "unexpected error: {error}");
        assert!(SkillId::parse(&"a".repeat(MAX_SKILL_ID_CHARS)).is_ok());
    }

    /// A case-insensitive filesystem would collide even though the strings differ,
    /// so uniqueness is judged case-insensitively.
    #[test]
    fn rejects_ids_differing_only_in_case() {
        // `Foo` cannot even be constructed, so the collision is expressed with
        // two entries that fold together once a source contributes one of them.
        let error = parse(
            r#"
[[skill]]
name = "foo"
source = "local"

[[source]]
name = "s"
from = "github:o/r"
skills = ["foo"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("declared twice"), "unexpected: {error}");
        assert!(error.contains("regardless of case"), "unexpected: {error}");
    }

    #[test]
    fn rejects_a_deploy_rule_with_no_include() {
        let error = parse("[[deploy]]\ntarget = \"claude:home\"\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no include"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_a_malformed_target() {
        for (target, expected) in [
            ("claude", "<provider>:<scope>"),
            (":home", "empty provider"),
            ("claude:global", "expected 'home' or 'repo'"),
        ] {
            let raw = format!("[[deploy]]\ntarget = \"{target}\"\ninclude = [\"*\"]\n");
            let error = parse(&raw).unwrap_err().to_string();
            assert!(
                error.contains(expected),
                "target {target:?} gave {error:?}, expected {expected:?}"
            );
        }
    }

    /// A suppression without a digest would keep applying after the content it
    /// was granted against changed.
    #[test]
    fn rejects_an_allow_entry_without_a_digest() {
        let error = parse("[safeguard]\nallow = [\"W021:draft-pr\"]\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("<digest>"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_skills_that_is_neither_a_list_nor_a_wildcard() {
        let error =
            parse("[[source]]\nname = \"s\"\nfrom = \"github:o/r\"\nskills = \"user-context\"\n")
                .unwrap_err()
                .to_string();
        assert!(error.contains("use \"*\""), "unexpected error: {error}");
    }

    #[test]
    fn parses_every_source_spec_form() -> Result<()> {
        assert_eq!(parse_source_spec("local")?, SourceSpec::Local);
        assert_eq!(
            parse_source_spec("github:igtm/kinko")?,
            SourceSpec::GitHub {
                owner: "igtm".to_string(),
                repo: "kinko".to_string()
            }
        );
        // A trailing .git is tolerated so a pasted URL slug still works.
        assert_eq!(
            parse_source_spec("github:igtm/kinko.git")?,
            SourceSpec::GitHub {
                owner: "igtm".to_string(),
                repo: "kinko".to_string()
            }
        );
        assert_eq!(
            parse_source_spec("gist:abc123")?,
            SourceSpec::Gist("abc123".to_string())
        );
        assert_eq!(
            parse_source_spec("git@github.com:igtm/kinko.git")?,
            SourceSpec::Git("git@github.com:igtm/kinko.git".to_string())
        );
        assert_eq!(
            parse_source_spec("path:../shared")?,
            SourceSpec::Path(PathBuf::from("../shared"))
        );
        assert!(parse_source_spec("igtm/kinko").is_err());
        assert!(parse_source_spec("gist:").is_err());
        assert!(parse_source_spec("github:igtm").is_err());
        Ok(())
    }

    #[test]
    fn selectors_match_by_label_or_id() {
        let writing = id("japanese-tech-writing");
        let labels = vec!["writing".to_string()];
        assert!(Selector::All.matches(&writing, &labels));
        assert!(Selector::Name("writing".to_string()).matches(&writing, &labels));
        // Naming a single skill directly avoids inventing a label for it.
        assert!(Selector::Name("japanese-tech-writing".to_string()).matches(&writing, &labels));
        assert!(!Selector::Name("tools".to_string()).matches(&writing, &labels));
    }

    #[test]
    fn exclude_overrides_include() {
        let rule = DeployRule {
            target: TargetRef {
                provider: "claude".to_string(),
                scope: TargetScope::Home,
            },
            include: vec![Selector::All],
            exclude: vec![Selector::Name("writing".to_string())],
            when_repo: None,
        };
        assert!(rule.selects(&id("draft-pr"), &["tools".to_string()]));
        assert!(!rule.selects(&id("japanese-tech-writing"), &["writing".to_string()]));
    }
}
