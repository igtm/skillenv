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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub skills: Vec<SkillEntry>,
    pub sources: Vec<SourceEntry>,
    pub deploys: Vec<DeployRule>,
    pub safeguard: SafeguardConfig,
    pub fetch: FetchConfig,
}

/// What `fetch` will accept from a remote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchConfig {
    /// Exactly as written in the manifest, so a message about the setting quotes the
    /// user's own spelling. `7d` and `1w` are the same duration, and echoing back the
    /// one they did not write reads as the tool having decided something.
    pub minimum_revision_age_text: Option<String>,
    /// Refuse a revision younger than this, taking the newest one that is old
    /// enough instead.
    ///
    /// A supply-chain delay, in the spirit of uv's release-age setting: a
    /// compromised upstream is usually noticed within hours or days, and waiting
    /// puts that window between publication and the moment the content reaches an
    /// agent's context. `None` means take whatever the ref points at.
    pub minimum_revision_age: Option<std::time::Duration>,
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
    /// Deploy, and report on stderr — including under `--quiet`.
    ///
    /// Deliberately not a non-zero exit. This is the default for `high`, and a
    /// legitimate skill can carry a `high` finding for a long time (install
    /// instructions really do pipe a download into a shell). Failing every `link`
    /// would mean failing the shell hook on every directory change, and the hook
    /// would be removed — losing the report as well. `lint` is the command that
    /// exits non-zero on findings; `allow`, pinned to a digest, is how a reviewed
    /// finding stops being mentioned.
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
    // No word is reserved. An earlier version reserved "skillenv", "all",
    // "local", "home", and "repo", and a migration rehearsal against a real setup
    // showed that rejecting a skill genuinely named `skillenv`. None of them is
    // ambiguous in practice: the selector wildcard is `*`, a target scope only
    // appears after a `:`, and "local" only appears in a source position.
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

/// What a removal took out of the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovedKind {
    /// A `[[skill]]` entry.
    Skill,
    /// A `[[source]]` entry, and with it every skill it contributed.
    Source,
}

/// Delete a `[[skill]]` or `[[source]]` entry by name, in place.
///
/// Edited with `toml_edit` rather than serialized from a struct, because the
/// manifest is written by hand: a round-trip through serde would discard every
/// comment and reorder what is left. v0 had no removal at all — a lock entry could
/// only be taken out by hand.
///
/// A comment directly above the removed entry goes with it, since it belongs to
/// that entry; leaving it behind would orphan an explanation above something else.
/// Every other entry keeps its comments and its formatting verbatim.
pub fn remove_entry(path: &Path, name: &str) -> Result<RemovedKind> {
    let raw = fs::read_to_string(path).map_err(|source| SkillenvError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let mut document =
        raw.parse::<toml_edit::DocumentMut>()
            .map_err(|error| SkillenvError::InvalidManifest {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;

    // `[[skill]]` is keyed on `name`; `[[source]]` likewise, so one helper serves
    // both. Skills are tried first: a name is far more likely to be a skill, and
    // removing a source takes its whole contribution with it.
    let removed = match take_named(&mut document, "skill", name) {
        true => RemovedKind::Skill,
        false => match take_named(&mut document, "source", name) {
            true => RemovedKind::Source,
            false => {
                return Err(SkillenvError::UnknownEntry {
                    name: name.to_string(),
                    path: path.to_path_buf(),
                });
            }
        },
    };

    fs::write(path, document.to_string()).map_err(|source| SkillenvError::WriteFile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(removed)
}

/// Drop the first element of the `key` array-of-tables whose `name` matches.
fn take_named(document: &mut toml_edit::DocumentMut, key: &str, name: &str) -> bool {
    let Some(array) = document
        .get_mut(key)
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        return false;
    };
    let Some(index) = array.iter().position(|table| {
        table
            .get("name")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == name)
    }) else {
        return false;
    };
    array.remove(index);
    true
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
    /// The compact spelling: source spec → the skills to take from it.
    #[serde(default, rename = "skills")]
    skill_lists: BTreeMap<String, RawSelection>,
    #[serde(default, rename = "source")]
    sources: Vec<RawSource>,
    #[serde(default, rename = "deploy")]
    deploys: Vec<RawDeploy>,
    #[serde(default)]
    safeguard: RawSafeguard,
    #[serde(default)]
    fetch: RawFetch,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFetch {
    minimum_revision_age: Option<String>,
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

        // `[skills]` is the same model in fewer lines: each key is a source and each
        // value the skills to take from it. Converted here rather than carried through,
        // so everything downstream — fetch, the wildcard path, the lock, `diff` — sees
        // one shape regardless of which spelling produced it.
        let mut skills = Vec::new();
        let mut listed_sources = Vec::new();
        for (spec_text, selection) in &self.skill_lists {
            let spec = parse_source_spec(spec_text)?;
            let selection = read_selection(selection, spec_text).map_err(invalid)?;

            if matches!(spec, SourceSpec::Local) {
                let SkillSelection::Explicit(ids) = selection else {
                    // There is no tree to enumerate: a local skill is a directory the
                    // user created, so "all of them" would mean whatever happens to be
                    // in `skills/`, which is a different and surprising thing to mean.
                    return Err(invalid(
                        "skills.local cannot be \"*\"; name each local skill, or use \
                         a path: source to follow a directory"
                            .to_string(),
                    ));
                };
                for id in ids {
                    skills.push(SkillEntry {
                        id,
                        source: SourceSpec::Local,
                        description: None,
                        labels: Vec::new(),
                    });
                }
                continue;
            }

            listed_sources.push(SourceEntry {
                // Derived, because this spelling has nowhere to put a name. It is what
                // `via=` reports and what names the cache directory, so it has to be
                // stable and recognisable — the repository or gist is both.
                name: derived_source_name(&spec),
                from: spec,
                git_ref: None,
                skills: selection,
                labels: Vec::new(),
            });
        }

        for raw in self.skills {
            skills.push(SkillEntry {
                id: SkillId::parse(&raw.name)?,
                source: parse_source_spec(&raw.source)?,
                description: raw.description,
                labels: raw.labels,
            });
        }

        let mut sources = listed_sources;
        for raw in self.sources {
            let skills = read_selection(&raw.skills, &format!("source '{}' skills", raw.name))
                .map_err(invalid)?;
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

        let fetch = FetchConfig {
            minimum_revision_age_text: self
                .fetch
                .minimum_revision_age
                .as_deref()
                .map(|raw| raw.trim().to_string()),
            minimum_revision_age: self
                .fetch
                .minimum_revision_age
                .as_deref()
                .map(parse_duration)
                .transpose()
                .map_err(invalid)?,
        };

        let manifest = Manifest {
            version: self.skillenv.version,
            skills,
            sources,
            deploys,
            safeguard,
            fetch,
        };
        check_duplicate_ids(&manifest, path)?;
        Ok(manifest)
    }
}

/// Parse a duration written the way a person would: `30m`, `72h`, `3d`, `2w`.
///
/// A bare number is rejected rather than assumed to be seconds. `minimum_revision_age
/// = 3` could reasonably mean seconds, hours, or days, and picking one silently would
/// give a supply-chain delay three orders of magnitude off what was intended.
fn parse_duration(raw: &str) -> std::result::Result<std::time::Duration, String> {
    let trimmed = raw.trim();
    let (digits, unit) = trimmed.split_at(
        trimmed
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("minimum_revision_age = {raw:?} needs a unit, e.g. \"3d\""))?,
    );
    let amount: u64 = digits
        .parse()
        .map_err(|_| format!("minimum_revision_age = {raw:?} does not start with a number"))?;
    let seconds = match unit.trim() {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        other => {
            return Err(format!(
                "minimum_revision_age = {raw:?} has unknown unit {other:?}; use s, m, h, d or w"
            ));
        }
    };
    Ok(std::time::Duration::from_secs(amount * seconds))
}

/// Turn a raw selection into the model's, used by both spellings.
///
/// A list may hold `"*"` on its own; an id can never be `*`, so there is nothing
/// ambiguous about it, and `["*"]` reads more naturally in a table of lists than a
/// bare string would.
fn read_selection(
    raw: &RawSelection,
    context: &str,
) -> std::result::Result<SkillSelection, String> {
    let names = match raw {
        RawSelection::Wildcard(value) if value == "*" => return Ok(SkillSelection::All),
        RawSelection::Wildcard(value) => {
            return Err(format!(
                "{context} = {value:?}; use \"*\" for every skill or a list of names"
            ));
        }
        RawSelection::Explicit(names) => names,
    };
    if names.len() == 1 && names[0] == "*" {
        return Ok(SkillSelection::All);
    }
    names
        .iter()
        .map(|name| SkillId::parse(name).map_err(|error| error.to_string()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map(SkillSelection::Explicit)
}

/// A name for a source that had nowhere to write one.
///
/// The repository or gist it came from, which is what makes `via=` and the cache
/// directory recognisable. Two sources that would derive the same name are caught by
/// the duplicate check, so this does not have to be clever.
fn derived_source_name(spec: &SourceSpec) -> String {
    match spec {
        SourceSpec::GitHub { repo, .. } => repo.clone(),
        SourceSpec::Gist(id) => id.clone(),
        SourceSpec::Git(url) => url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit(['/', ':'])
            .next()
            .unwrap_or(url)
            .to_string(),
        SourceSpec::Path(path) => path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "path".to_string()),
        SourceSpec::Local => "local".to_string(),
    }
}

/// Reject two declarations of the same id up front.
///
/// v0 keyed this on `(scope, id)` and only discovered a clash during `link`,
/// after the source had already been installed and the lock file written.
fn check_duplicate_ids(manifest: &Manifest, path: &Path) -> Result<()> {
    // Source names too, not only skill ids. A name is the cache directory and what
    // `via=` reports, and `remote_sources` groups on it — so two sources sharing one
    // would quietly become a single source, with one of them simply gone. The
    // `[skills]` spelling derives its names, which is what makes this reachable.
    let mut names: BTreeMap<&str, ()> = BTreeMap::new();
    for source in &manifest.sources {
        if names.insert(source.name.as_str(), ()).is_some() {
            return Err(SkillenvError::InvalidManifest {
                path: path.to_path_buf(),
                message: format!(
                    "two sources are both named '{}'; a [skills] key takes its name from \
                     the repository or gist, so give one of them an explicit [[source]] \
                     entry with a different name",
                    source.name
                ),
            });
        }
    }

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
        // A local clone reached as a git remote, so its history is available.
        // `path:` reads a working tree and has no revisions at all, which means no
        // `minimum_revision_age` and no `outdated` — this is how you get those for a
        // repository that happens to be on the same machine.
        || raw.starts_with("file://")
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

    /// The compact spelling has to produce exactly the model the long one does, or the
    /// two forms would behave differently for the same intent.
    #[test]
    fn a_skills_table_becomes_the_same_model_as_the_long_form() -> Result<()> {
        let compact = parse(
            "[skillenv]\nversion = 1\n\n[skills]\n\
             local = [\"draft-pr\", \"handoff\"]\n\
             \"github:igtm/skills\" = [\"user-context\"]\n\
             \"gist:fd287c31\" = [\"jp-writing\"]\n",
        )?;

        assert_eq!(
            compact
                .skills
                .iter()
                .map(|entry| entry.id.to_string())
                .collect::<Vec<_>>(),
            vec!["draft-pr", "handoff"]
        );
        // A remote key becomes a source, named after the repository or gist, since the
        // table has nowhere to write a name.
        let names: Vec<&str> = compact.sources.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["fd287c31", "skills"]);
        assert_eq!(
            compact.sources[1].skills,
            SkillSelection::Explicit(vec![id("user-context")])
        );
        Ok(())
    }

    /// Both spellings in one manifest, which is the point: the table for the simple
    /// majority, an entry for the source that needs a ref or labels.
    #[test]
    fn both_spellings_coexist() -> Result<()> {
        let manifest = parse(
            "[skillenv]\nversion = 1\n\n\
             [skills]\nlocal = [\"draft-pr\"]\n\n\
             [[source]]\nname = \"kinko\"\nfrom = \"github:igtm/kinko\"\n\
             ref = \"v2\"\nlabels = [\"secrets\"]\nskills = [\"kinko\"]\n",
        )?;
        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.sources.len(), 1);
        assert_eq!(manifest.sources[0].git_ref.as_deref(), Some("v2"));
        assert_eq!(manifest.sources[0].labels, vec!["secrets".to_string()]);
        Ok(())
    }

    /// `["*"]` follows the whole source. A skill id can never be `*`, so a
    /// single-element list saying so is unambiguous.
    #[test]
    fn a_wildcard_works_in_either_spelling() -> Result<()> {
        let table = parse("[skillenv]\nversion = 1\n\n[skills]\n\"github:o/r\" = [\"*\"]\n")?;
        assert_eq!(table.sources[0].skills, SkillSelection::All);

        let long = parse(
            "[skillenv]\nversion = 1\n\n[[source]]\nname = \"r\"\n\
             from = \"github:o/r\"\nskills = \"*\"\n",
        )?;
        assert_eq!(long.sources[0].skills, SkillSelection::All);
        Ok(())
    }

    /// `local` has no tree to enumerate — "every local skill" would mean whatever
    /// happens to be in `skills/`, which is a different thing to mean.
    #[test]
    fn a_local_wildcard_is_refused() {
        let error = parse("[skillenv]\nversion = 1\n\n[skills]\nlocal = [\"*\"]\n").unwrap_err();
        assert!(error.to_string().contains("cannot be"), "got: {error}");
    }

    /// A derived name can collide with a declared one. Left alone, the two sources
    /// would share a cache directory and one would silently disappear when they were
    /// grouped by name.
    #[test]
    fn a_derived_source_name_colliding_with_a_declared_one_is_refused() {
        let error = parse(
            "[skillenv]\nversion = 1\n\n\
             [skills]\n\"github:someone/tools\" = [\"alpha\"]\n\n\
             [[source]]\nname = \"tools\"\nfrom = \"github:other/thing\"\n\
             skills = [\"beta\"]\n",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("both named 'tools'"),
            "got: {error}"
        );
    }

    /// A bare number is rejected rather than guessed at: `minimum_revision_age = 3`
    /// could mean seconds, hours or days, and picking one silently would give a
    /// supply-chain delay orders of magnitude off what was meant.
    #[test]
    fn a_revision_age_needs_a_unit() {
        use std::time::Duration;
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("15m"), Ok(Duration::from_secs(900)));
        assert_eq!(parse_duration("72h"), Ok(Duration::from_secs(259_200)));
        assert_eq!(parse_duration("3d"), Ok(Duration::from_secs(259_200)));
        assert_eq!(parse_duration("2w"), Ok(Duration::from_secs(1_209_600)));
        assert_eq!(parse_duration(" 7d "), Ok(Duration::from_secs(604_800)));

        for bad in ["3", "", "d", "3y", "-1d", "3 days"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    /// The table is optional, and absent means no limit rather than a zero one.
    #[test]
    fn a_manifest_without_a_fetch_table_has_no_limit() -> Result<()> {
        let manifest = parse("[skillenv]\nversion = 1\n")?;
        assert_eq!(manifest.fetch.minimum_revision_age, None);

        let limited = parse("[skillenv]\nversion = 1\n\n[fetch]\nminimum_revision_age = \"3d\"\n")?;
        assert_eq!(
            limited.fetch.minimum_revision_age,
            Some(std::time::Duration::from_secs(259_200))
        );
        Ok(())
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

    /// No word is reserved. An earlier version reserved "skillenv", "all", "local",
    /// "home", and "repo", which broke a real skill named `skillenv` during a
    /// migration rehearsal. None of them is actually ambiguous: the selector
    /// wildcard is `*`, a target scope only ever appears after a `:`, and "local"
    /// only appears in a source position.
    #[test]
    fn no_word_is_reserved() {
        for raw in ["skillenv", "all", "none", "local", "home", "repo"] {
            assert!(SkillId::parse(raw).is_ok(), "{raw:?} should be a usable id");
        }
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

    /// The manifest is written by hand, so an edit must keep the comments a
    /// serde round-trip would destroy.
    ///
    /// A comment sitting directly above an entry belongs to that entry, so
    /// removing the entry takes it too — which is what a reader would expect, and
    /// avoids leaving an explanation orphaned above something else. Comments
    /// belonging to other entries are untouched.
    #[test]
    fn removing_an_entry_keeps_the_comments_that_belong_to_others() -> Result<()> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skillenv.toml");
        std::fs::write(
            &path,
            r#"[skillenv]
version = 1

# explains draft-pr, and goes with it
[[skill]]
name = "draft-pr"
source = "local"

# explains writing, and must stay
[[skill]]
name = "writing"
source = "local"
labels = ["prose"]
"#,
        )
        .unwrap();

        assert_eq!(remove_entry(&path, "draft-pr")?, RemovedKind::Skill);
        let after = std::fs::read_to_string(&path).unwrap();

        assert!(!after.contains("draft-pr"), "got:\n{after}");
        assert!(
            !after.contains("explains draft-pr"),
            "its own comment should go with it:\n{after}"
        );
        assert!(
            after.contains("# explains writing, and must stay"),
            "another entry's comment must survive:\n{after}"
        );
        // Formatting the other entry kept is preserved verbatim, not re-emitted.
        assert!(after.contains("labels = [\"prose\"]"), "got:\n{after}");
        // And the result is still a valid manifest.
        let reparsed = Manifest::parse(&after, &path)?;
        assert_eq!(reparsed.skills.len(), 1);
        Ok(())
    }

    #[test]
    fn removing_a_source_is_distinguished_from_removing_a_skill() -> Result<()> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skillenv.toml");
        std::fs::write(
            &path,
            "[[source]]\nname = \"igtm-skills\"\nfrom = \"github:igtm/skills\"\n\
             skills = [\"user-context\"]\n",
        )
        .unwrap();
        assert_eq!(remove_entry(&path, "igtm-skills")?, RemovedKind::Source);
        assert!(
            Manifest::parse(&std::fs::read_to_string(&path).unwrap(), &path)?
                .sources
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn removing_a_name_that_is_not_there_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skillenv.toml");
        std::fs::write(&path, "[[skill]]\nname = \"a\"\nsource = \"local\"\n").unwrap();
        let error = remove_entry(&path, "missing").unwrap_err().to_string();
        assert!(error.contains("missing"), "unexpected: {error}");
        // The file is untouched when nothing matched.
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("name = \"a\"")
        );
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
