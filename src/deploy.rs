//! Writing skills into a target directory, and taking them back out.
//!
//! Three properties here are direct consequences of the outage this rewrite came
//! out of, and every one of them is load-bearing.
//!
//! **The marker is written first.** v0 created the directory, copied assets,
//! rendered `SKILL.md`, and only then wrote the marker. A skill whose frontmatter
//! failed to parse therefore left a directory holding assets and *no marker* — and
//! since the marker is the only evidence skillenv created something, every later
//! run classified that residue as someone else's and refused to touch it, then
//! aborted. One typo froze an entire setup for six weeks. Writing the marker
//! before any content inverts this: an interrupted render leaves something we
//! recognise as ours, so the next run reclaims and replaces it. No staging
//! directory, no atomic rename, no cleanup pass.
//!
//! **One skill's failure does not stop the others.** But only for failures that
//! belong to one skill — a malformed SKILL.md, a target already occupied. An I/O
//! error affects every skill, so it still aborts rather than being reported N
//! times alongside a success exit code.
//!
//! **Removal and counting are the same walk.** v0 had two implementations that
//! disagreed: removal applied a scope filter to symlinked entries and counting did
//! not, so `status` could report a number `unlink` would not honour.
//!
//! This engine is not yet reachable from the CLI. The swap — pointing `main.rs`
//! here and deleting the scope-based path — is deliberately its own change, so the
//! deletion of the old code and the new command surface land together and can be
//! reviewed as one thing. The allow goes away then.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::catalog::CatalogEntry;
use crate::lock::hex_digest;
use crate::manifest::{SkillId, TargetScope};
use crate::paths::{ensure_dir, slugify_or};
use crate::provider::{CanonicalSkill, ProviderId, RenderedSkill, render_for, validate};
use crate::render::parse_frontmatter;
use crate::{Result, SkillenvError};

/// Marker filename. Unchanged from v0 so a v0 deployment is still discoverable by
/// `crate::legacy_sweep`, which is what lets a migration clean up after itself.
pub(crate) const MARKER_FILE: &str = ".skillenv-generated.json";

/// Prefix on every generated directory name.
///
/// Deliberately identical to v0's. Users have `.gitignore` entries matching
/// `skillenv-*`, and changing it would turn every repo-local deployment into
/// untracked noise.
const NAME_PREFIX: &str = "skillenv-";

/// Characters of path digest used to distinguish repositories in a shared
/// directory.
const DISCRIMINATOR_CHARS: usize = 12;

/// Identifies which manifest a deployment belongs to.
///
/// A `$HOME` target is shared by every repository on the machine while removal
/// keys on a name prefix, so without a discriminator one repository's `link` would
/// delete another's entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestId {
    slug: String,
    /// Present for `$HOME` targets only.
    discriminator: Option<String>,
}

impl ManifestId {
    /// Derive an id for a manifest root.
    ///
    /// The discriminator is `sha256` over the canonical path. v0 used a 48-bit
    /// FNV-1a hash of a path that fell back to the *un*-canonicalized form when
    /// `canonicalize` failed, so the same repository could hash two ways depending
    /// on whether the path happened to exist. Here a path that cannot be
    /// canonicalized is an error instead.
    pub fn for_root(root: &Path, scope: TargetScope) -> Result<Self> {
        let slug = slugify_or(
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("repo"),
            "repo",
        );
        let discriminator = match scope {
            TargetScope::Repo => None,
            TargetScope::Home => {
                let canonical =
                    fs::canonicalize(root).map_err(|source| SkillenvError::ReadFile {
                        path: root.to_path_buf(),
                        source,
                    })?;
                let digest = hex_digest(canonical.to_string_lossy().as_bytes());
                Some(digest[..DISCRIMINATOR_CHARS].to_string())
            }
        };
        Ok(Self {
            slug,
            discriminator,
        })
    }

    /// Prefix shared by every directory this manifest owns in a target.
    pub fn prefix(&self) -> String {
        match &self.discriminator {
            Some(discriminator) => format!("{NAME_PREFIX}{}-g{discriminator}-", self.slug),
            None => format!("{NAME_PREFIX}{}-", self.slug),
        }
    }

    /// Directory name for one skill.
    ///
    /// No scope segment: there are no scopes, which is what keeps the name inside
    /// the 64 characters providers allow.
    pub fn generated_name(&self, id: &SkillId) -> String {
        format!("{}{}", self.prefix(), id.as_str())
    }

    /// Stable string recorded in a marker, so removal can tell whose it is
    /// without consulting a path that may since have moved.
    pub fn as_str(&self) -> String {
        match &self.discriminator {
            Some(discriminator) => format!("{}-g{discriminator}", self.slug),
            None => self.slug.clone(),
        }
    }
}

/// What skillenv records about a directory it created.
///
/// Deliberately free of any path into the source tree. v0 stored one and made
/// removal conditional on it still resolving, which is why a migrated setup could
/// not clean up after itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// Matches [`ManifestId::as_str`].
    pub manifest: String,
    pub skill: String,
    pub generated_name: String,
    pub provider: String,
    /// Revision the content came from, when it came from git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Digest of the source tree, so a re-link can tell whether anything changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    /// Digest of the bytes actually written, which also covers a change in how we
    /// render for this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_digest: Option<String>,
}

/// One directory in a target, as found by a walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingEntry {
    pub path: PathBuf,
    pub dir_name: String,
    /// `None` when the directory carries our prefix but no readable marker.
    pub marker: Option<Marker>,
}

impl ExistingEntry {
    /// Whether this belongs to `id`, and so may be replaced or removed.
    pub fn belongs_to(&self, id: &ManifestId) -> bool {
        self.marker
            .as_ref()
            .is_some_and(|marker| marker.manifest == id.as_str())
    }
}

/// Why a skill was not deployed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSkill {
    pub id: SkillId,
    pub generated_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeployReport {
    pub target: PathBuf,
    pub written: Vec<SkillId>,
    /// Deployments already current, left alone.
    pub unchanged: Vec<SkillId>,
    pub removed: Vec<String>,
    pub skipped: Vec<SkippedSkill>,
    /// Directories carrying the prefix with no marker, left in place.
    pub unmanaged: Vec<PathBuf>,
    /// Non-fatal observations, e.g. a key a provider could not accept.
    pub notes: Vec<String>,
}

impl DeployReport {
    /// Whether anything needs a human's attention.
    pub fn has_problems(&self) -> bool {
        !self.skipped.is_empty() || !self.unmanaged.is_empty()
    }
}

/// Enumerate every directory in `target` carrying `id`'s prefix.
///
/// The single walk both removal and counting use, so the two cannot disagree.
pub fn enumerate(target: &Path, id: &ManifestId) -> Result<Vec<ExistingEntry>> {
    if !target.is_dir() {
        return Ok(Vec::new());
    }
    let prefix = id.prefix();
    let mut found = Vec::new();

    let mut entries: Vec<_> = fs::read_dir(target)
        .map_err(|source| SkillenvError::ReadFile {
            path: target.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if !dir_name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        let marker = read_marker(&path)?;
        found.push(ExistingEntry {
            path,
            dir_name,
            marker,
        });
    }
    Ok(found)
}

/// Write the selected skills into `target` and remove what no longer belongs.
///
/// `resolve` supplies each skill's on-disk directory; a skill it cannot resolve is
/// skipped with that explanation rather than aborting the run.
pub fn apply<F>(
    target: &Path,
    id: &ManifestId,
    provider: ProviderId,
    selected: &[&CatalogEntry],
    mut resolve: F,
) -> Result<DeployReport>
where
    F: FnMut(&CatalogEntry) -> Result<SkillContent>,
{
    let mut report = DeployReport {
        target: target.to_path_buf(),
        ..Default::default()
    };
    ensure_dir(target)?;

    let existing = enumerate(target, id)?;
    let wanted: BTreeSet<String> = selected
        .iter()
        .map(|entry| id.generated_name(&entry.id))
        .collect();

    // Remove ours that are no longer selected. Anything without a marker is
    // reported and left: without one there is no evidence we created it.
    for entry in &existing {
        if entry.marker.is_none() {
            report.unmanaged.push(entry.path.clone());
            continue;
        }
        if !entry.belongs_to(id) {
            continue;
        }
        if !wanted.contains(&entry.dir_name) {
            remove_entry(&entry.path)?;
            report.removed.push(entry.dir_name.clone());
        }
    }

    let by_name: BTreeMap<&str, &ExistingEntry> = existing
        .iter()
        .map(|entry| (entry.dir_name.as_str(), entry))
        .collect();

    for entry in selected {
        let generated_name = id.generated_name(&entry.id);
        let existing = by_name.get(generated_name.as_str()).copied();

        match deploy_one(target, id, provider, entry, existing, &mut resolve) {
            Ok(Outcome::Written { notes }) => {
                report.written.push(entry.id.clone());
                report.notes.extend(notes);
            }
            Ok(Outcome::Unchanged) => report.unchanged.push(entry.id.clone()),
            Err(error) if is_skill_local(&error) => report.skipped.push(SkippedSkill {
                id: entry.id.clone(),
                generated_name,
                reason: error.to_string(),
            }),
            // Systemic: a read-only filesystem or a full disk hits every skill,
            // and reporting it once per skill while exiting successfully would
            // bury it.
            Err(error) => return Err(error),
        }
    }

    Ok(report)
}

/// A skill's bytes and provenance, as resolved by the caller.
#[derive(Debug, Clone)]
pub struct SkillContent {
    pub dir: PathBuf,
    pub revision: Option<String>,
    pub content_digest: Option<String>,
    /// Overrides the skill's own frontmatter description. Needed for sources that
    /// carry no frontmatter at all, such as a gist.
    pub description: Option<String>,
}

enum Outcome {
    Written { notes: Vec<String> },
    Unchanged,
}

fn deploy_one<F>(
    target: &Path,
    id: &ManifestId,
    provider: ProviderId,
    entry: &CatalogEntry,
    existing: Option<&ExistingEntry>,
    resolve: &mut F,
) -> Result<Outcome>
where
    F: FnMut(&CatalogEntry) -> Result<SkillContent>,
{
    let generated_name = id.generated_name(&entry.id);

    // The manifest's id cap is a static budget that cannot account for a long
    // repository name, so the real limit is checked here against the actual name.
    // Writing a file the provider will reject is worse than skipping it with a
    // reason.
    let length = generated_name.chars().count();
    if length > crate::provider::MAX_NAME_CHARS {
        return Err(SkillenvError::GeneratedNameTooLong {
            name: generated_name,
            length,
            limit: crate::provider::MAX_NAME_CHARS,
        });
    }

    let destination = target.join(&generated_name);

    // Occupied by something we did not create: never overwritten. Reported as a
    // skip so the rest of the run continues.
    if let Some(existing) = existing {
        if existing.marker.is_none() {
            return Err(SkillenvError::TargetCollision { path: destination });
        }
        if !existing.belongs_to(id) {
            return Err(SkillenvError::TargetCollision { path: destination });
        }
    }

    let content = resolve(entry)?;
    let canonical = load_canonical(&content.dir, &entry.id, content.description.as_deref())?;
    let rendered = render_for(provider, &canonical, &generated_name)?;
    let rendered_digest = hex_digest(rendered.skill_md.as_bytes());

    // Nothing to do when both the source and our rendering of it are unchanged.
    // Without this the shell hook re-renders every skill on every directory
    // change, churning mtimes for no reason.
    if let Some(existing) = existing {
        if let Some(marker) = &existing.marker {
            let same_render = marker.rendered_digest.as_deref() == Some(rendered_digest.as_str());
            let same_content = marker.content_digest == content.content_digest;
            if same_render && same_content && existing.path.join("SKILL.md").is_file() {
                return Ok(Outcome::Unchanged);
            }
        }
        remove_entry(&existing.path)?;
    }

    let marker = Marker {
        manifest: id.as_str(),
        skill: entry.id.to_string(),
        generated_name: generated_name.clone(),
        provider: provider.as_str().to_string(),
        revision: content.revision.clone(),
        content_digest: content.content_digest.clone(),
        rendered_digest: Some(rendered_digest),
    };

    write_deployment(&destination, &marker, &rendered, &content.dir)?;

    let mut notes = Vec::new();
    for key in &rendered.dropped_keys {
        notes.push(format!(
            "{}: {provider} does not accept the frontmatter key {key:?}, so it was omitted",
            entry.id
        ));
    }
    for diagnostic in validate(&generated_name, &canonical) {
        notes.push(format!("{}: {}", entry.id, diagnostic.message));
    }
    Ok(Outcome::Written { notes })
}

/// Write one deployment, marker first.
///
/// The ordering is the whole point: if anything after the marker fails, what is
/// left behind still identifies itself as ours, so the next run replaces it
/// instead of refusing to touch it.
fn write_deployment(
    destination: &Path,
    marker: &Marker,
    rendered: &RenderedSkill,
    source_dir: &Path,
) -> Result<()> {
    ensure_dir(destination)?;

    let marker_path = destination.join(MARKER_FILE);
    let body =
        serde_json::to_string_pretty(marker).map_err(|source| SkillenvError::SerializeMarker {
            path: marker_path.clone(),
            source,
        })?;
    fs::write(&marker_path, format!("{body}\n")).map_err(|source| SkillenvError::WriteFile {
        path: marker_path,
        source,
    })?;

    copy_assets(source_dir, destination)?;

    let skill_md = destination.join("SKILL.md");
    fs::write(&skill_md, &rendered.skill_md).map_err(|source| SkillenvError::WriteFile {
        path: skill_md,
        source,
    })?;

    for sidecar in &rendered.sidecars {
        let path = destination.join(&sidecar.relative_path);
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        fs::write(&path, &sidecar.contents)
            .map_err(|source| SkillenvError::WriteFile { path, source })?;
    }
    Ok(())
}

/// Copy everything except the files we generate ourselves.
fn copy_assets(source_dir: &Path, destination: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(source_dir).follow_links(false) {
        let entry = entry.map_err(|error| SkillenvError::ReadFile {
            path: source_dir.to_path_buf(),
            source: std::io::Error::other(error),
        })?;
        let relative =
            entry
                .path()
                .strip_prefix(source_dir)
                .map_err(|error| SkillenvError::ReadFile {
                    path: source_dir.to_path_buf(),
                    source: std::io::Error::other(error),
                })?;
        // SKILL.md is rendered, not copied; the marker is ours; .DS_Store is noise.
        let name = relative.to_string_lossy();
        if relative.as_os_str().is_empty()
            || name == "SKILL.md"
            || name == MARKER_FILE
            || name.contains(".DS_Store")
        {
            continue;
        }

        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            ensure_dir(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            ensure_dir(parent)?;
        }
        fs::copy(entry.path(), &target).map_err(|source| SkillenvError::WriteFile {
            path: target,
            source,
        })?;
    }
    Ok(())
}

/// Read a skill directory into the form providers render from.
fn load_canonical(
    dir: &Path,
    id: &SkillId,
    description_override: Option<&str>,
) -> Result<CanonicalSkill> {
    let skill_md = dir.join("SKILL.md");
    let raw = fs::read_to_string(&skill_md).map_err(|source| SkillenvError::ReadFile {
        path: skill_md.clone(),
        source,
    })?;
    let (frontmatter, body) = parse_frontmatter(&skill_md, &raw)?;

    let mut extra = BTreeMap::new();
    for (key, value) in &frontmatter {
        let Some(key) = key.as_str() else { continue };
        // `name` is ours to set, and `description` is handled below.
        if key == "name" || key == "description" {
            continue;
        }
        extra.insert(key.to_string(), value.clone());
    }

    let description = description_override
        .map(str::to_string)
        .or_else(|| {
            frontmatter
                .get(Value::String("description".to_string()))
                .and_then(Value::as_str)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| {
            // Every provider requires a description, so one is synthesized rather
            // than emitting a file that will not validate. A gist has no
            // frontmatter at all, which is how this arises in practice.
            format!(
                "Instructions for the {} skill.",
                id.as_str().replace('-', " ")
            )
        });

    Ok(CanonicalSkill {
        id: id.clone(),
        description,
        body,
        extra,
    })
}

/// Whether a failure belongs to one skill, so the rest of the run may continue.
///
/// I/O deliberately does not qualify: a read-only filesystem or a full disk
/// affects every skill, and turning that into N warnings plus a success exit code
/// would hide it.
fn is_skill_local(error: &SkillenvError) -> bool {
    matches!(
        error,
        SkillenvError::ParseFrontmatter { .. }
            | SkillenvError::InvalidMetadataField { .. }
            | SkillenvError::TargetCollision { .. }
            | SkillenvError::MissingSkillFile { .. }
            | SkillenvError::UnsafeSourceEntry { .. }
            | SkillenvError::SourceTooLarge { .. }
            | SkillenvError::GeneratedNameTooLong { .. }
    )
}

fn read_marker(dir: &Path) -> Result<Option<Marker>> {
    let path = dir.join(MARKER_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(SkillenvError::ReadFile { path, source }),
    };
    // Unreadable is treated as absent, so the directory is reported rather than
    // deleted on the strength of a marker we could not parse.
    Ok(serde_json::from_str(&raw).ok())
}

fn remove_entry(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SkillenvError::ReadFile {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    // Only a real directory is recursed into; a symlink is unlinked, so removal
    // cannot escape the target directory.
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|source| SkillenvError::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SourceSpec;
    use tempfile::TempDir;

    fn id_for(root: &Path) -> ManifestId {
        ManifestId::for_root(root, TargetScope::Repo).expect("repo scope needs no canonicalize")
    }

    fn skill_id(raw: &str) -> SkillId {
        SkillId::parse(raw).expect("test id should be valid")
    }

    fn catalog_entry(id: &str) -> CatalogEntry {
        CatalogEntry {
            id: skill_id(id),
            source: SourceSpec::Local,
            source_name: None,
            git_ref: None,
            description: None,
            labels: Vec::new(),
        }
    }

    /// A source directory holding one skill.
    fn source_skill(root: &Path, id: &str, frontmatter: &str) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("SKILL.md"), frontmatter).unwrap();
        fs::write(dir.join("assets/t.md"), "asset\n").unwrap();
        dir
    }

    fn resolver(sources: PathBuf) -> impl FnMut(&CatalogEntry) -> Result<SkillContent> {
        move |entry: &CatalogEntry| {
            let dir = sources.join(entry.id.as_str());
            Ok(SkillContent {
                dir,
                revision: Some("abc123".to_string()),
                content_digest: Some("sha256:fixed".to_string()),
                description: entry.description.clone(),
            })
        }
    }

    fn valid(name: &str) -> String {
        format!("---\nname: {name}\ndescription: A skill for testing\n---\n\nBody\n")
    }

    #[test]
    fn writes_a_skill_with_its_marker_rendered_file_and_assets() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));
        let target = work.path().join("target");
        let id = id_for(work.path());

        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;

        assert_eq!(report.written, vec![skill_id("kinko")]);
        let dir = target.join(id.generated_name(&skill_id("kinko")));
        assert!(dir.join(MARKER_FILE).is_file());
        assert!(dir.join("assets/t.md").is_file());

        let written = fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(written.contains(&format!("name: {}", id.generated_name(&skill_id("kinko")))));
        Ok(())
    }

    /// The central lesson from the outage: an interrupted render must leave
    /// something we recognise, so the next run can replace it.
    #[test]
    fn a_marker_only_directory_is_reclaimed_rather_than_blocking() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));
        let target = work.path().join("target");
        let id = id_for(work.path());
        let generated = id.generated_name(&skill_id("kinko"));

        // Exactly what a crash after the marker write leaves: our marker, no
        // SKILL.md.
        let dir = target.join(&generated);
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(
            dir.join(MARKER_FILE),
            serde_json::to_string(&Marker {
                manifest: id.as_str(),
                skill: "kinko".to_string(),
                generated_name: generated.clone(),
                provider: "claude".to_string(),
                revision: None,
                content_digest: None,
                rendered_digest: None,
            })
            .unwrap(),
        )
        .unwrap();

        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;

        assert_eq!(report.written, vec![skill_id("kinko")]);
        assert!(report.skipped.is_empty(), "must not be blocked: {report:?}");
        assert!(dir.join("SKILL.md").is_file(), "the render should complete");
        Ok(())
    }

    /// One malformed skill must not strand the others.
    #[test]
    fn a_broken_skill_is_skipped_and_the_rest_are_written() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "alpha", &valid("alpha"));
        // Unquoted `: ` makes this invalid YAML — the original failure.
        source_skill(
            &sources,
            "broken",
            "---\nname: broken\ndescription: Agent Skill: broken\n---\n\nBody\n",
        );
        source_skill(&sources, "zeta", &valid("zeta"));

        let target = work.path().join("target");
        let id = id_for(work.path());
        let entries = [
            catalog_entry("alpha"),
            catalog_entry("broken"),
            catalog_entry("zeta"),
        ];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;

        assert_eq!(report.written.len(), 2, "{report:?}");
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].id, skill_id("broken"));
        assert!(report.skipped[0].reason.contains("frontmatter"));
        assert!(target.join(id.generated_name(&skill_id("alpha"))).is_dir());
        assert!(target.join(id.generated_name(&skill_id("zeta"))).is_dir());
        Ok(())
    }

    /// Repairing the source is enough; no manual cleanup.
    #[test]
    fn a_repaired_skill_deploys_on_the_next_run() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        let dir = source_skill(
            &sources,
            "kinko",
            "---\nname: kinko\ndescription: Agent Skill: broken\n---\n\nBody\n",
        );
        let target = work.path().join("target");
        let id = id_for(work.path());
        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();

        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources.clone()),
        )?;
        assert_eq!(report.skipped.len(), 1);

        fs::write(dir.join("SKILL.md"), valid("kinko")).unwrap();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;
        assert_eq!(report.written, vec![skill_id("kinko")]);
        assert!(report.skipped.is_empty());
        Ok(())
    }

    /// A directory we did not create is never overwritten, and the run continues.
    #[test]
    fn an_unmanaged_directory_is_reported_and_left_intact() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));
        let target = work.path().join("target");
        let id = id_for(work.path());

        let occupied = target.join(id.generated_name(&skill_id("kinko")));
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("README.md"), "hand written\n").unwrap();

        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;

        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].reason.contains("refusing to overwrite"));
        assert_eq!(
            fs::read_to_string(occupied.join("README.md")).unwrap(),
            "hand written\n"
        );
        Ok(())
    }

    /// Re-running must not rewrite files, or the shell hook churns mtimes on
    /// every directory change.
    #[test]
    fn an_unchanged_skill_is_left_alone_on_a_second_run() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));
        let target = work.path().join("target");
        let id = id_for(work.path());
        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();

        apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources.clone()),
        )?;
        let skill_md = target
            .join(id.generated_name(&skill_id("kinko")))
            .join("SKILL.md");
        let first = fs::metadata(&skill_md).unwrap().modified().unwrap();

        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;
        assert_eq!(report.unchanged, vec![skill_id("kinko")]);
        assert!(report.written.is_empty());
        assert_eq!(
            fs::metadata(&skill_md).unwrap().modified().unwrap(),
            first,
            "an unchanged skill must not be rewritten"
        );
        Ok(())
    }

    #[test]
    fn a_deselected_skill_is_removed() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));
        source_skill(&sources, "draft-pr", &valid("draft-pr"));
        let target = work.path().join("target");
        let id = id_for(work.path());

        let both = [catalog_entry("kinko"), catalog_entry("draft-pr")];
        let selected: Vec<&CatalogEntry> = both.iter().collect();
        apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources.clone()),
        )?;

        let one = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = one.iter().collect();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;
        assert_eq!(
            report.removed,
            vec![id.generated_name(&skill_id("draft-pr"))]
        );
        assert!(
            !target
                .join(id.generated_name(&skill_id("draft-pr")))
                .exists()
        );
        Ok(())
    }

    /// `$HOME` is shared, so another manifest's deployment must survive.
    #[test]
    fn another_manifests_deployment_is_not_removed() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));
        let target = work.path().join("target");
        let id = id_for(work.path());

        // Same prefix, different manifest.
        let theirs = target.join(format!("{}other", id.prefix()));
        fs::create_dir_all(&theirs).unwrap();
        fs::write(
            theirs.join(MARKER_FILE),
            serde_json::to_string(&Marker {
                manifest: "someone-else".to_string(),
                skill: "other".to_string(),
                generated_name: "x".to_string(),
                provider: "claude".to_string(),
                revision: None,
                content_digest: None,
                rendered_digest: None,
            })
            .unwrap(),
        )
        .unwrap();

        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();
        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;
        assert!(report.removed.is_empty(), "{report:?}");
        assert!(theirs.is_dir());
        Ok(())
    }

    /// The one walk both removal and counting use, so they cannot disagree.
    #[test]
    fn enumerate_reports_marked_and_unmarked_alike() -> Result<()> {
        let work = TempDir::new().unwrap();
        let target = work.path().join("target");
        let id = id_for(work.path());
        fs::create_dir_all(&target).unwrap();

        fs::create_dir_all(target.join(format!("{}unmarked", id.prefix()))).unwrap();
        fs::create_dir_all(target.join("unrelated")).unwrap();

        let found = enumerate(&target, &id)?;
        assert_eq!(found.len(), 1, "only prefixed entries: {found:?}");
        assert!(found[0].marker.is_none());
        assert!(!found[0].belongs_to(&id));
        Ok(())
    }

    /// A gist has no frontmatter, so the description has to come from elsewhere.
    #[test]
    fn a_description_override_is_used_when_the_source_has_no_frontmatter() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        let dir = sources.join("jp-writing");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# 日本語技術文書の文章規範\n\n本文\n").unwrap();

        let target = work.path().join("target");
        let id = id_for(work.path());
        let mut entry = catalog_entry("jp-writing");
        entry.description = Some("日本語の文章規範".to_string());
        let entries = [entry];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();

        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;
        assert_eq!(report.written.len(), 1, "{report:?}");
        let written = fs::read_to_string(
            target
                .join(id.generated_name(&skill_id("jp-writing")))
                .join("SKILL.md"),
        )
        .unwrap();
        assert!(written.contains("日本語の文章規範"), "got: {written}");
        // The heading is body content, not frontmatter, so it survives verbatim.
        assert!(written.contains("# 日本語技術文書の文章規範"));
        Ok(())
    }

    #[test]
    fn a_home_scope_name_carries_a_discriminator_and_a_repo_one_does_not() -> Result<()> {
        let work = TempDir::new().unwrap();
        let repo = work.path().join("dotfiles");
        fs::create_dir_all(&repo).unwrap();

        let repo_local = ManifestId::for_root(&repo, TargetScope::Repo)?;
        assert_eq!(repo_local.prefix(), "skillenv-dotfiles-");
        assert_eq!(repo_local.as_str(), "dotfiles");

        let home = ManifestId::for_root(&repo, TargetScope::Home)?;
        assert!(
            home.prefix().starts_with("skillenv-dotfiles-g"),
            "got {}",
            home.prefix()
        );
        // 12 hex characters, so two repositories sharing $HOME stay distinct.
        assert_eq!(home.prefix().len(), "skillenv-dotfiles-g".len() + 12 + 1);
        Ok(())
    }

    /// v0 fell back to an un-canonicalized path when canonicalize failed, so one
    /// repository could hash two ways. Here it is an error.
    #[test]
    fn a_home_id_for_a_missing_root_is_an_error() {
        let error = ManifestId::for_root(Path::new("/nonexistent/dotfiles"), TargetScope::Home);
        assert!(error.is_err());
    }

    /// Names must stay inside the 64 characters providers allow, which is why
    /// there is no scope segment and ids are capped. The budget is exact for a
    /// repository slug of typical length.
    #[test]
    fn a_generated_name_fits_the_provider_limit() -> Result<()> {
        let work = TempDir::new().unwrap();
        let repo = work.path().join("dotfiles");
        fs::create_dir_all(&repo).unwrap();
        let id = ManifestId::for_root(&repo, TargetScope::Home)?;
        let longest = skill_id(&"a".repeat(crate::manifest::MAX_SKILL_ID_CHARS));
        let name = id.generated_name(&longest);
        assert_eq!(
            name.chars().count(),
            crate::provider::MAX_NAME_CHARS,
            "the static id cap should exactly spend the budget: {name}"
        );
        Ok(())
    }

    /// The static cap cannot account for a long repository name, so the real
    /// limit is enforced against the actual generated name. Writing a file the
    /// provider would reject is worse than skipping it with a reason.
    #[test]
    fn a_name_overflowing_because_of_a_long_repo_name_is_skipped() -> Result<()> {
        let work = TempDir::new().unwrap();
        // Long enough that even a short id overflows once the discriminator is
        // added.
        let repo = work
            .path()
            .join("a-very-long-repository-directory-name-indeed");
        fs::create_dir_all(&repo).unwrap();
        let sources = work.path().join("src");
        source_skill(&sources, "kinko", &valid("kinko"));

        let id = ManifestId::for_root(&repo, TargetScope::Home)?;
        let target = work.path().join("target");
        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();

        let report = apply(
            &target,
            &id,
            ProviderId::Claude,
            &selected,
            resolver(sources),
        )?;
        assert!(report.written.is_empty(), "{report:?}");
        assert_eq!(report.skipped.len(), 1);
        assert!(
            report.skipped[0].reason.contains("64"),
            "the reason should name the limit: {}",
            report.skipped[0].reason
        );
        // Nothing was written, so no invalid file reaches the provider.
        assert!(!target.join(id.generated_name(&skill_id("kinko"))).exists());
        Ok(())
    }

    #[test]
    fn io_failures_are_fatal_while_skill_problems_are_not() {
        assert!(is_skill_local(&SkillenvError::TargetCollision {
            path: PathBuf::from("/x")
        }));
        assert!(is_skill_local(&SkillenvError::MissingSkillFile {
            path: PathBuf::from("/x")
        }));
        assert!(!is_skill_local(&SkillenvError::WriteFile {
            path: PathBuf::from("/x"),
            source: std::io::Error::other("disk full"),
        }));
        assert!(!is_skill_local(&SkillenvError::CreateDir {
            path: PathBuf::from("/x"),
            source: std::io::Error::other("read-only"),
        }));
    }

    /// A key a provider cannot accept is reported, not silently discarded.
    #[test]
    fn a_dropped_provider_key_is_reported() -> Result<()> {
        let work = TempDir::new().unwrap();
        let sources = work.path().join("src");
        source_skill(
            &sources,
            "kinko",
            "---\nname: kinko\ndescription: A skill\ncompatibility: Requires Node\n---\n\nBody\n",
        );
        let target = work.path().join("target");
        let id = id_for(work.path());
        let entries = [catalog_entry("kinko")];
        let selected: Vec<&CatalogEntry> = entries.iter().collect();

        // Codex rejects `compatibility`.
        let report = apply(
            &target,
            &id,
            ProviderId::Codex,
            &selected,
            resolver(sources),
        )?;
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("compatibility")),
            "expected a note: {:?}",
            report.notes
        );
        Ok(())
    }
}
