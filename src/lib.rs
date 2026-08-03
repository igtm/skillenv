use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod catalog;
mod deploy;
mod inventory;
mod legacy_sweep;
mod lock;
mod manifest;
mod migrate;
mod paths;
mod provider;
mod render;
mod safeguard;
mod session;
mod source;
#[cfg(test)]
mod test_support;

pub use inventory::format_skill_inventory_report;
pub use legacy_sweep::{LegacyEntry, SweepReport};
pub use safeguard::{Finding, Severity};
pub use session::LinkReport;

/// Inspect a v0 setup and describe the conversion, without writing anything.
///
/// Read-only on purpose. The whole plan is inspectable before a single file is
/// touched, and the markers the plan depends on are destroyed by the conversion
/// itself, so there is no second chance to read them.
pub fn plan_migration(cwd: impl AsRef<Path>) -> Result<String> {
    let root = fs::canonicalize(cwd.as_ref()).unwrap_or_else(|_| cwd.as_ref().to_path_buf());
    let plan = migrate::plan(&root, &home_dir()?)?;
    let mut out = migrate::format_plan(&plan);
    if plan.can_apply() {
        out.push_str("\n\n--- proposed skillenv.toml ---\n");
        out.push_str(&migrate::render_manifest(&plan));
    }
    Ok(out)
}

/// Carry out the conversion `plan_migration` described.
///
/// Clears v0's deployments first, while their markers still refer to a layout that
/// exists, and leaves `skillenv/` in place unless `prune` is set — so the result
/// can be checked, and undone by deleting two files, before anything is lost.
pub fn apply_migration(cwd: impl AsRef<Path>, prune: bool) -> Result<String> {
    let root = fs::canonicalize(cwd.as_ref()).unwrap_or_else(|_| cwd.as_ref().to_path_buf());
    let plan = migrate::plan(&root, &home_dir()?)?;
    let report = migrate::apply(&plan, &migrate::ApplyOptions { prune })?;
    Ok(migrate::format_report(&report))
}

/// Populate the cache for every remote skill the manifest declares.
///
/// Without `update`, restores exactly what the lock records — which is what a
/// fresh clone needs, since the cache is not committed. With it, moves to whatever
/// each ref points at now.
pub fn fetch_manifest(cwd: impl AsRef<Path>, update: bool) -> Result<(String, Vec<String>, bool)> {
    let mut session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let report = session.fetch(update)?;
    let mut lines = vec![format!(
        "{} skill(s) cached{}{}",
        report.fetched.len(),
        if report.reused.is_empty() {
            String::new()
        } else {
            format!(", {} source(s) already current", report.reused.len())
        },
        if report.dropped.is_empty() {
            String::new()
        } else {
            format!(
                ", {} no longer declared and forgotten",
                report.dropped.len()
            )
        }
    )];
    for id in &report.fetched {
        lines.push(format!("  {id}"));
    }
    Ok((lines.join("\n"), report.warnings(), report.has_problems()))
}

/// Compare the lock against what each remote ref points at now. Reads only.
pub fn outdated_manifest(cwd: impl AsRef<Path>) -> Result<(String, bool)> {
    let session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let stale = session.outdated()?;
    if stale.is_empty() {
        return Ok(("everything is current".to_string(), false));
    }
    let short = |value: &Option<String>| {
        value
            .as_deref()
            .map(|revision| revision[..12.min(revision.len())].to_string())
            .unwrap_or_else(|| "none".to_string())
    };
    let mut lines = Vec::new();
    for entry in &stale {
        match &entry.note {
            Some(note) => lines.push(format!("{}: could not check: {note}", entry.source_name)),
            None => lines.push(format!(
                "{}: locked {} -> available {}",
                entry.source_name,
                short(&entry.locked),
                short(&entry.latest)
            )),
        }
    }
    lines.push("run `skillenv fetch --update` to move to these revisions".to_string());
    Ok((lines.join("\n"), true))
}

/// Lines `init` keeps in `.gitignore`.
///
/// The cache is not committed, and a repository that is itself a deploy target
/// would otherwise show every generated directory as untracked.
const V1_GITIGNORE: &[&str] = &[
    ".skillenv/",
    ".agents/skills/skillenv-*",
    ".claude/skills/skillenv-*",
    ".opencode/skills/skillenv-*",
];

/// The manifest `init` writes when there is none.
const MANIFEST_TEMPLATE: &str = r#"[skillenv]
version = 1

# Your own skills live in skills/<name>/SKILL.md.
# [[skill]]
# name = "my-skill"
# source = "local"
# labels = ["writing"]

# A source can contribute several skills. Use skills = "*" to follow all of them.
# [[source]]
# name = "upstream"
# from = "github:owner/repo"
# skills = ["one", "two"]

# Where the skills go. Scope is "home" ($HOME) or "repo" (whatever repository
# you are standing in), and `when.repo` limits a rule to one of them.
[[deploy]]
target = "claude:home"
include = ["*"]
"#;

/// Create the v1 layout: a manifest, a place for your own skills, and the
/// `.gitignore` lines that keep the generated output out of the way.
///
/// Never overwrites an existing manifest — that file is the only hand-written
/// input, so replacing it with a template would discard the whole configuration.
pub fn init_manifest(cwd: impl AsRef<Path>) -> Result<String> {
    let root = fs::canonicalize(cwd.as_ref()).unwrap_or_else(|_| cwd.as_ref().to_path_buf());
    let manifest_path = root.join(manifest::MANIFEST_FILE);
    let mut lines = Vec::new();

    if manifest_path.is_file() {
        lines.push(format!("{} already exists", manifest::MANIFEST_FILE));
    } else {
        fs::write(&manifest_path, MANIFEST_TEMPLATE).map_err(|source| {
            SkillenvError::WriteFile {
                path: manifest_path.clone(),
                source,
            }
        })?;
        lines.push(format!("created {}", manifest::MANIFEST_FILE));
    }

    let skills = root.join("skills");
    if !skills.is_dir() {
        fs::create_dir_all(&skills).map_err(|source| SkillenvError::WriteFile {
            path: skills.clone(),
            source,
        })?;
        lines.push("created skills/".to_string());
    }

    if append_gitignore(&root, V1_GITIGNORE)? {
        lines.push(".gitignore updated".to_string());
    }

    Ok(lines.join("\n"))
}

/// Add any of `patterns` that are missing, leaving the rest of the file alone.
fn append_gitignore(root: &Path, patterns: &[&str]) -> Result<bool> {
    let path = root.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(SkillenvError::ReadFile {
                path: path.clone(),
                source,
            });
        }
    };

    let missing: Vec<&str> = patterns
        .iter()
        .copied()
        .filter(|pattern| !existing.lines().any(|line| line.trim() == *pattern))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }

    let mut contents = existing;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    if !contents.is_empty() {
        contents.push('\n');
    }
    contents.push_str("# skillenv\n");
    for pattern in missing {
        contents.push_str(pattern);
        contents.push('\n');
    }
    fs::write(&path, contents).map_err(|source| SkillenvError::WriteFile { path, source })?;
    Ok(true)
}

/// Everything about how this invocation resolved: which manifest, which cache,
/// which targets. Reads only.
///
/// Deliberately broader than `status`. `status` answers "what is deployed"; this
/// answers "why did it go there", which is the question when a `link` deploys
/// somewhere unexpected or nowhere at all.
pub fn doctor_manifest(cwd: impl AsRef<Path>, json: bool) -> Result<String> {
    let session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let status = session.status()?;
    let cache = source::cache_root(&session.root);
    let cached = count_cached_sources(&cache);

    if json {
        let value = serde_json::json!({
            "manifest": session.root.join(manifest::MANIFEST_FILE),
            "root": session.root,
            "repo_root": session.repo_root,
            "home": session.home,
            "cache": { "path": cache, "sources": cached },
            "lock": { "skills": session.lock.skills.len() },
            "catalog": {
                "skills": session.catalog.entries.len(),
                "deploy_rules": session.catalog.deploys.len(),
            },
            "targets": status.targets.iter().map(|target| serde_json::json!({
                "path": target.path,
                "provider": target.provider,
                "manifest_id": target.manifest_id,
                "deployed": target.ours(),
                "missing": target.missing.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        return serde_json::to_string_pretty(&value).map_err(|source| {
            SkillenvError::SerializeLock {
                path: PathBuf::from("stdout"),
                source,
            }
        });
    }

    let mut lines = vec![
        format!(
            "manifest: {}",
            session.root.join(manifest::MANIFEST_FILE).display()
        ),
        format!(
            "repo: {}",
            session
                .repo_root
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none — repo-scoped rules will not apply".to_string())
        ),
        format!("home: {}", session.home.display()),
        format!("cache: {} ({cached} source(s))", cache.display()),
        format!(
            "declared: {} skill(s), {} deploy rule(s); lock records {}",
            session.catalog.entries.len(),
            session.catalog.deploys.len(),
            session.lock.skills.len()
        ),
    ];
    lines.push("targets:".to_string());
    if status.targets.is_empty() {
        lines.push("  none — no deploy rule applies from here".to_string());
    }
    for target in &status.targets {
        lines.push(format!(
            "  {} [{}] {} deployed{}",
            target.path.display(),
            target.provider,
            target.ours(),
            if target.missing.is_empty() {
                String::new()
            } else {
                format!(", {} missing", target.missing.len())
            }
        ));
    }
    Ok(lines.join("\n"))
}

/// How many source directories the cache holds. Absent cache counts as zero
/// rather than an error: a fresh clone has none, and that is the normal state.
fn count_cached_sources(cache: &Path) -> usize {
    fs::read_dir(cache)
        .map(|entries| entries.filter_map(|entry| entry.ok()).count())
        .unwrap_or(0)
}

/// Report what the manifest governing `cwd` has deployed. Reads only.
pub fn status_manifest(cwd: impl AsRef<Path>) -> Result<(String, bool)> {
    let session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let report = session.status()?;
    Ok((
        format_status_manifest_report(&report),
        report.has_problems(),
    ))
}

pub fn format_status_manifest_report(report: &session::StatusReport) -> String {
    if report.targets.is_empty() {
        return "no deploy rules apply here".to_string();
    }
    let mut lines = Vec::new();
    for target in &report.targets {
        lines.push(format!(
            "{} [{}] {} deployed",
            target.path.display(),
            target.provider,
            target.ours()
        ));
        for entry in &target.entries {
            let detail = match &entry.ownership {
                session::Ownership::Ours => match &entry.revision {
                    Some(revision) => format!(" @{}", &revision[..12.min(revision.len())]),
                    None => String::new(),
                },
                session::Ownership::OtherManifest(id) => format!(" (manifest {id})"),
                session::Ownership::Unmanaged => " (no marker, left alone)".to_string(),
            };
            lines.push(format!(
                "  {}{}",
                entry.skill.as_deref().unwrap_or(&entry.dir_name),
                detail
            ));
        }
        for id in &target.missing {
            lines.push(format!("  {id}: selected but not deployed"));
        }
    }
    lines.join("\n")
}

/// Drop a `[[skill]]` or `[[source]]` entry from the manifest, forget it in the
/// lock, and clear whatever it had deployed.
///
/// The order matters. The manifest is edited first so the re-link that follows sees
/// the entry gone and removes its directories; doing it the other way round would
/// deploy it again on the way out. v0 had no removal at all — a lock entry could
/// only be taken out by hand, and its deployments were then orphaned.
///
/// Not transactional: if saving the lock or the re-link fails after the manifest has
/// been rewritten, the manifest and the deployed state disagree until the next
/// `link`, which then reconciles them. Left that way deliberately — `link` is
/// already the operation that makes the two agree, so a rollback here would add a
/// second recovery path to keep correct.
pub fn remove_from_manifest(cwd: impl AsRef<Path>, name: &str) -> Result<RemoveReport> {
    let mut session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let manifest_path = session.root.join(manifest::MANIFEST_FILE);
    let kind = manifest::remove_entry(&manifest_path, name)?;

    match kind {
        // Here `name` is the skill's own id.
        manifest::RemovedKind::Skill => {
            if let Ok(id) = manifest::SkillId::parse(name) {
                session.lock.remove(&id);
            }
        }
        // Here it is a source's label, which is a *different* namespace: id
        // uniqueness is only checked between skill ids, so a source may share a
        // name with a skill some other source contributes. Matching on the id
        // would then delete that unrelated skill's lock entry, and the relink
        // below would take its directory with it.
        manifest::RemovedKind::Source => session
            .lock
            .skills
            .retain(|skill| skill.source_name.as_deref() != Some(name)),
    }
    session.lock.save(&session.root)?;

    // Reopened so the re-link reads the edited manifest rather than the copy this
    // session parsed before the edit.
    let mut session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let report = session.link()?;
    let cleared: usize = report
        .targets
        .iter()
        .map(|target| target.removed.len())
        .sum();
    let label = match kind {
        manifest::RemovedKind::Skill => "skill",
        manifest::RemovedKind::Source => "source",
    };
    Ok(RemoveReport {
        summary: format!("removed {label} {name}; {cleared} deployment(s) cleared"),
        warnings: report.warnings(),
        // The relink can block a skill or skip one. Every other command turns that
        // into a non-zero exit; discarding it here would make `skillenv remove` the
        // one place a scripted caller could not tell that something went wrong.
        problems: report.has_problems(),
    })
}

/// What a removal did, and whether anything needs a human's attention.
pub struct RemoveReport {
    pub summary: String,
    pub warnings: Vec<String>,
    pub problems: bool,
}

/// Remove every deployment belonging to the manifest governing `cwd`.
pub fn unlink_manifest(cwd: impl AsRef<Path>) -> Result<LinkReport> {
    let mut session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    session.unlink()
}

/// Remove the v0 layout from an already-migrated repository.
///
/// Its own entry point rather than a flag on the conversion, because the order that
/// makes sense — migrate, check, then discard — is two invocations.
pub fn prune_legacy_layout(cwd: impl AsRef<Path>) -> Result<String> {
    let root = fs::canonicalize(cwd.as_ref()).unwrap_or_else(|_| cwd.as_ref().to_path_buf());
    let removed = migrate::prune(&root)?;
    Ok(format!("removed {}", removed.display()))
}

/// Whether a v1 manifest governs `cwd`.
///
/// Callers use this to decide which engine to run. While both exist, a repository
/// still on the v0 layout keeps working untouched — a hard cutover would break a
/// live setup the moment the binary was replaced.
pub fn has_manifest(cwd: impl AsRef<Path>) -> bool {
    session::locate_manifest(cwd.as_ref()).is_ok()
}

/// Deploy every skill the manifest selects, to every target it names.
pub fn link_manifest(cwd: impl AsRef<Path>) -> Result<LinkReport> {
    let mut session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    session.link()
}

/// One line per skill: what it is, where it comes from, how it is labelled.
pub fn list_manifest(cwd: impl AsRef<Path>) -> Result<String> {
    let session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let mut lines = vec![format!("manifest: {}", session.root.display())];

    for entry in session.catalog.iter() {
        let mut parts = vec![format!("{}", entry.id)];
        parts.push(format!("source={}", describe_source(&entry.source)));
        if let Some(name) = &entry.source_name {
            parts.push(format!("via={name}"));
        }
        if !entry.labels.is_empty() {
            parts.push(format!("labels={}", entry.labels.join(",")));
        }
        match session.lock.get(&entry.id) {
            Some(locked) => match &locked.resolved_revision {
                Some(revision) => {
                    parts.push(format!("revision={}", &revision[..12.min(revision.len())]))
                }
                None => parts.push("revision=unversioned".to_string()),
            },
            None if entry.needs_fetch() => parts.push("revision=unfetched".to_string()),
            None => {}
        }
        lines.push(format!("  {}", parts.join(" ")));
    }

    for source in &session.catalog.wildcard_sources {
        lines.push(format!(
            "  ({} tracks every skill from {}; run `skillenv fetch` to resolve them)",
            source.name,
            describe_source(&source.from)
        ));
    }
    Ok(lines.join("\n"))
}

/// Scan every skill the manifest declares and report what the checks found.
pub fn lint_manifest(cwd: impl AsRef<Path>) -> Result<(String, bool)> {
    let session = session::Session::open(cwd.as_ref(), home_dir()?)?;
    let mut lines = Vec::new();
    let mut problems = false;

    for entry in session.catalog.iter() {
        let Some(dir) = entry.local_dir(&session.root) else {
            continue;
        };
        let skill_md = dir.join("SKILL.md");
        if !skill_md.is_file() {
            lines.push(format!(
                "{}: W014 [low]: no SKILL.md at {}",
                entry.id,
                skill_md.display()
            ));
            problems = true;
            continue;
        }
        let raw = fs::read_to_string(&skill_md).map_err(|source| SkillenvError::ReadFile {
            path: skill_md.clone(),
            source,
        })?;

        // Checked first and reported rather than propagated: unparseable
        // frontmatter is the single most common way a skill fails to deploy, and
        // the whole point of `lint` is to find it before `link` does.
        if let Err(error) = render::parse_frontmatter(&skill_md, &raw) {
            problems = true;
            lines.push(format!("{}: {error}", entry.id));
        }

        for finding in safeguard::scan_text(&raw) {
            problems = true;
            lines.push(format!("{}: {finding}", entry.id));
        }
    }

    if lines.is_empty() {
        lines.push("no findings".to_string());
    }
    Ok((lines.join("\n"), problems))
}

fn describe_source(spec: &manifest::SourceSpec) -> String {
    match spec {
        manifest::SourceSpec::Local => "local".to_string(),
        manifest::SourceSpec::Gist(id) => format!("gist:{id}"),
        manifest::SourceSpec::GitHub { owner, repo } => format!("github:{owner}/{repo}"),
        manifest::SourceSpec::Git(url) => url.clone(),
        manifest::SourceSpec::Path(path) => format!("path:{}", path.display()),
    }
}

/// Human-readable summary of a `link`, for stdout.
pub fn format_link_manifest_report(report: &LinkReport) -> String {
    let mut lines = Vec::new();
    for target in &report.targets {
        lines.push(format!(
            "{}: {} written, {} unchanged, {} removed",
            target.target.display(),
            target.written.len(),
            target.unchanged.len(),
            target.removed.len()
        ));
    }
    if lines.is_empty() {
        lines.push("no targets matched; check the [[deploy]] rules".to_string());
    }
    lines.join("\n")
}

/// Find what v0 deployed for `repo_slug` in `target`.
///
/// Matching keys on the marker's `repo` and the generated-name prefix, never on
/// its `source`, because migration moves the files that path refers to. v0's own
/// removal predicate required a live `source`, which is why a migrated setup would
/// otherwise be unable to clean up after itself.
pub fn sweep_legacy(target: &std::path::Path, repo_slug: &str) -> Result<SweepReport> {
    legacy_sweep::sweep(target, repo_slug)
}

/// Remove the v0 deployments a sweep found, leaving unmarked directories alone.
pub fn remove_legacy(report: &SweepReport) -> Result<usize> {
    legacy_sweep::remove(report)
}

/// Scan one `SKILL.md` for hidden instructions and unsafe patterns.
///
/// Frontmatter is included on purpose: `description` is loaded eagerly into agent
/// context while the body is not, which makes it the most valuable place to hide
/// an instruction.
pub fn scan_skill_text(text: &str) -> Vec<Finding> {
    safeguard::scan_text(text)
}
use inventory::take_inventory;
use paths::normalize_path;
use render::parse_frontmatter;

const GENERATED_MARKER_FILE: &str = ".skillenv-generated.json";

pub type Result<T> = std::result::Result<T, SkillenvError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillInventoryTool {
    Claude,
    Codex,
    Opencode,
    Antigravity,
}

impl SkillInventoryTool {
    fn all() -> [Self; 4] {
        [Self::Claude, Self::Codex, Self::Opencode, Self::Antigravity]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Antigravity => "antigravity",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SkillInventoryOptions {
    pub tools: Vec<SkillInventoryTool>,
    pub repo_tree: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillInventoryMode {
    #[serde(rename = "current")]
    Current,
    #[serde(rename = "current-and-repo-tree")]
    CurrentAndRepoTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SkillDiscoveryState {
    #[serde(rename = "current")]
    Current,
    #[serde(rename = "repo-tree-only")]
    RepoTreeOnly,
    #[serde(rename = "nested-on-demand")]
    NestedOnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillInventoryStatus {
    #[serde(rename = "shadowed")]
    Shadowed,
    #[serde(rename = "duplicate-visible")]
    DuplicateVisible,
    #[serde(rename = "invalid")]
    Invalid,
    #[serde(rename = "legacy")]
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInventoryReport {
    pub repo_root: Option<PathBuf>,
    pub mode: SkillInventoryMode,
    pub tools: Vec<SkillInventoryTool>,
    pub entries: Vec<SkillInventoryEntry>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInventoryEntry {
    pub tool: SkillInventoryTool,
    pub scope: String,
    pub discovery_state: SkillDiscoveryState,
    pub name: String,
    pub description: Option<String>,
    pub skill_dir: PathBuf,
    pub skill_md: Option<PathBuf>,
    pub skillenv_managed: bool,
    pub skillenv_origin: String,
    pub status: Vec<SkillInventoryStatus>,
}

#[derive(Debug, Error)]
pub enum SkillenvError {
    #[error("failed to read {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("failed to create directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("invalid config at {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        source: toml_edit::de::Error,
    },
    #[error("invalid manifest at {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: toml_edit::de::Error,
    },
    #[error("invalid manifest at {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },
    #[error("no [[skill]] or [[source]] named '{name}' in {path}")]
    UnknownEntry { name: String, path: PathBuf },
    #[error("invalid skill id '{input}': {reason}")]
    InvalidSkillId { input: String, reason: String },
    #[error("unknown provider '{name}'; known providers are {known}")]
    UnknownProvider { name: String, known: String },
    #[error("{program} did not finish within {seconds}s and was stopped")]
    CommandTimedOut { program: String, seconds: u64 },
    #[error("remote {transport} has no ref '{reference}'")]
    UnknownRemoteRef {
        transport: String,
        reference: String,
    },
    #[error("no SKILL.md at {path}")]
    MissingSkillFile { path: PathBuf },
    #[error(
        "no skillenv.toml found from {searched_from} upwards; create one or set SKILLENV_MANIFEST"
    )]
    ManifestNotFound { searched_from: PathBuf },
    #[error(
        "generated name '{name}' is {length} characters, over the {limit} providers accept; shorten the skill id or the repository directory name"
    )]
    GeneratedNameTooLong {
        name: String,
        length: usize,
        limit: usize,
    },
    #[error("refusing {path}: it is {reason}")]
    UnsafeSourceEntry { path: PathBuf, reason: String },
    #[error("{path} exceeds the limit of {limit}")]
    SourceTooLarge { path: PathBuf, limit: String },
    #[error(
        "skill id '{id}' is declared twice, by {first} and {second}; ids are unique across every source"
    )]
    DuplicateSkillId {
        id: String,
        first: String,
        second: String,
    },
    #[error(
        "lock file at {path} is version {found}, but this build understands only version {supported}; upgrade skillenv"
    )]
    UnsupportedLockVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("invalid frontmatter in {path}: {source}")]
    ParseFrontmatter {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("invalid metadata field in {path}: expected a mapping")]
    InvalidMetadataField { path: PathBuf },
    #[error("invalid lock file at {path}: {source}")]
    ParseLock {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to serialize lock file at {path}: {source}")]
    SerializeLock {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("duplicate skill '{skill}' in scope '{scope}' from '{first}' and '{second}'")]
    DuplicateSkill {
        scope: String,
        skill: String,
        first: String,
        second: String,
    },
    #[error("repo root not detected; this command requires a git repository")]
    RepoRequired,
    #[error("HOME is not set; global skill targets require a home directory")]
    HomeNotSet,
    #[error(
        "repo is not initialized for skillenv outputs; run `skillenv init` with the desired target flags first"
    )]
    RepoNotInitialized,
    #[error("invalid source '{input}': {message}")]
    InvalidSource { input: String, message: String },
    #[error("refusing to overwrite unmanaged target {path}")]
    TargetCollision { path: PathBuf },
    #[error("managed source collision at {path}")]
    ManagedSourceCollision { path: PathBuf },
    #[error("unknown managed source '{name}'")]
    UnknownManagedSource { name: String },
    #[error("failed to serialize marker for {path}: {source}")]
    SerializeMarker {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to run {program}: {source}")]
    RunCommand {
        program: String,
        cwd: Option<PathBuf>,
        source: io::Error,
    },
    #[error("command {program} failed in {cwd:?}: {stderr}")]
    CommandFailed {
        program: String,
        cwd: Option<PathBuf>,
        stderr: String,
    },
    #[error("unsupported shell '{0}'")]
    UnsupportedShell(String),
}

/// A v0 marker, as the inventory still finds them on disk.
///
/// Frozen: v0 wrote these and nothing writes them now, so the shape is whatever
/// v0 produced. `strategy` is a string rather than an enum for the same reason
/// [`legacy_sweep`] keeps it one — a value this code does not recognise must not
/// turn a readable marker into an unreadable one, since an unreadable marker means
/// the directory can never be identified as generated and so is never cleaned up.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeneratedMarker {
    repo: String,
    scope: String,
    skill: String,
    generated_name: String,
    source: String,
    strategy: String,
}

pub fn skill_inventory(
    cwd: impl AsRef<Path>,
    options: SkillInventoryOptions,
) -> Result<SkillInventoryReport> {
    take_inventory(cwd.as_ref(), &options)
}

/// A shell hook that relinks when you change directory.
///
/// Runs `link --quiet`, which exits silently outside a managed tree: a hook that
/// complained on every `cd` would be turned off within the hour. Warnings about a
/// skill that failed to deploy still reach stderr, because that is the failure the
/// hook exists to surface.
///
/// Set `SKILLENV_MANIFEST` to deploy into repositories that do not contain the
/// manifest themselves — the usual arrangement, with the manifest in a dotfiles
/// checkout and `[[deploy]]` rules naming the repositories it applies to.
pub fn hook_script(shell: Shell) -> String {
    match shell {
        Shell::Zsh => r#"autoload -Uz add-zsh-hook
_skillenv_chpwd() {
  command skillenv link --quiet
}
add-zsh-hook chpwd _skillenv_chpwd
_skillenv_chpwd
"#
        .to_string(),
        Shell::Bash => r#"_skillenv_last_repo_root=""
_skillenv_prompt_hook() {
  local current_repo_root
  current_repo_root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
  if [ "$current_repo_root" = "$_skillenv_last_repo_root" ]; then
    return
  fi
  _skillenv_last_repo_root="$current_repo_root"
  command skillenv link --quiet
}
case ";${PROMPT_COMMAND};" in
  *";_skillenv_prompt_hook;"*) ;;
  *)
    if [ -n "${PROMPT_COMMAND:-}" ]; then
      PROMPT_COMMAND="_skillenv_prompt_hook;${PROMPT_COMMAND}"
    else
      PROMPT_COMMAND="_skillenv_prompt_hook"
    fi
    ;;
esac
"#
        .to_string(),
    }
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(SkillenvError::HomeNotSet)
}

fn detect_repo_root(cwd: &Path) -> Option<PathBuf> {
    let start = if cwd.is_absolute() {
        normalize_path(cwd)
    } else {
        normalize_path(&env::current_dir().ok()?.join(cwd))
    };

    for candidate in start.ancestors() {
        let git_path = candidate.join(".git");
        if git_path.exists() {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zsh => write!(f, "zsh"),
            Self::Bash => write!(f, "bash"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{ensure_dir, slugify_or};
    use crate::test_support::set_home_for_test;
    use std::path::Path;
    use tempfile::TempDir;

    /// A directory symlink, for the inventory tests that check skillenv follows
    /// one a user made by hand. Deployment no longer creates any — v1 renders
    /// only — so this lives here rather than in `paths`.
    #[cfg(unix)]
    fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(source, destination)
    }

    #[cfg(windows)]
    fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(source, destination)
    }

    #[test]
    fn slugify_normalizes_values() {
        assert_eq!(slugify_or("My Repo", "repo"), "my-repo");
        assert_eq!(slugify_or("---A__B---", "repo"), "a-b");
        assert_eq!(slugify_or("Already-kebab", "repo"), "already-kebab");
        assert_eq!(slugify_or("!!!", "repo"), "repo");
    }

    #[test]
    fn detect_repo_root_normalizes_dot_segments() -> Result<()> {
        let repo = repo_fixture()?;
        let detected = detect_repo_root(&repo.path().join(".")).unwrap();
        assert_eq!(detected, repo.path());
        Ok(())
    }

    #[test]
    fn hook_scripts_call_quiet_link() {
        assert!(hook_script(Shell::Zsh).contains("skillenv link --quiet"));
        assert!(hook_script(Shell::Bash).contains("skillenv link --quiet"));
    }

    #[test]
    fn skill_inventory_lists_repo_local_tool_directories() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/research",
            Some(
                r#"---
name: research
description: repo agent
---
"#,
            ),
            "repo agent",
        )?;
        write_skill(
            repo.path(),
            ".claude/skills/review",
            Some(
                r#"---
name: review
description: repo claude
---
"#,
            ),
            "repo claude",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![
                    SkillInventoryTool::Codex,
                    SkillInventoryTool::Claude,
                    SkillInventoryTool::Opencode,
                    SkillInventoryTool::Antigravity,
                ],
                repo_tree: false,
            },
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.scope == "repository"
                && entry.name == "research"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Opencode
                && entry.scope == "repository"
                && entry.name == "research"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Antigravity
                && entry.scope == "workspace"
                && entry.name == "research"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Claude
                && entry.scope == "project"
                && entry.name == "review"
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_lists_user_and_global_roots() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            home.path(),
            ".agents/skills/user-agent",
            Some(
                r#"---
name: user-agent
---
"#,
            ),
            "user agent",
        )?;
        write_skill(
            home.path(),
            ".claude/skills/personal-review",
            Some(
                r#"---
name: personal-review
---
"#,
            ),
            "personal review",
        )?;
        write_skill(
            home.path(),
            ".config/opencode/skills/global-open",
            Some(
                r#"---
name: global-open
---
"#,
            ),
            "global open",
        )?;
        write_skill(
            home.path(),
            ".gemini/antigravity/skills/gravity",
            Some(
                r#"---
name: gravity
---
"#,
            ),
            "gravity",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![
                    SkillInventoryTool::Codex,
                    SkillInventoryTool::Claude,
                    SkillInventoryTool::Opencode,
                    SkillInventoryTool::Antigravity,
                ],
                repo_tree: false,
            },
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.scope == "user"
                && entry.name == "user-agent"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Claude
                && entry.scope == "user"
                && entry.name == "personal-review"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Opencode
                && entry.scope == "global"
                && entry.name == "global-open"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Antigravity
                && entry.scope == "global"
                && entry.name == "gravity"
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_marks_codex_duplicates_as_visible() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_skill = r#"---
name: shared-skill
description: duplicate
---
"#;
        write_skill(
            repo.path(),
            ".agents/skills/repo-shared",
            Some(shared_skill),
            "repo shared",
        )?;
        write_skill(
            home.path(),
            ".agents/skills/user-shared",
            Some(shared_skill),
            "user shared",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;

        let duplicate_entries: Vec<_> = report
            .entries
            .iter()
            .filter(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == "shared-skill")
            .collect();
        assert_eq!(duplicate_entries.len(), 2);
        assert!(duplicate_entries.iter().all(|entry| {
            entry
                .status
                .contains(&SkillInventoryStatus::DuplicateVisible)
        }));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate visible skill 'shared-skill'"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_marks_shadowed_claude_project_skills() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_skill = r#"---
name: review
description: duplicate
---
"#;
        write_skill(
            repo.path(),
            ".claude/skills/project-review",
            Some(shared_skill),
            "project review",
        )?;
        write_skill(
            home.path(),
            ".claude/skills/user-review",
            Some(shared_skill),
            "user review",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Claude],
                repo_tree: false,
            },
        )?;

        let project_entry = report
            .entries
            .iter()
            .find(|entry| {
                entry.tool == SkillInventoryTool::Claude
                    && entry.scope == "project"
                    && entry.name == "review"
            })
            .unwrap();
        let user_entry = report
            .entries
            .iter()
            .find(|entry| {
                entry.tool == SkillInventoryTool::Claude
                    && entry.scope == "user"
                    && entry.name == "review"
            })
            .unwrap();
        assert!(
            project_entry
                .status
                .contains(&SkillInventoryStatus::Shadowed)
        );
        assert!(!user_entry.status.contains(&SkillInventoryStatus::Shadowed));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("shadowed visible skill 'review'"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_reports_invalid_skills_without_panicking() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/bad-frontmatter",
            Some(
                r#"---
name: [broken
---
"#,
            ),
            "broken",
        )?;
        ensure_dir(&repo.path().join(".agents/skills/missing-skill-md"))?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.name == "bad-frontmatter" && entry.status.contains(&SkillInventoryStatus::Invalid)
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.name == "missing-skill-md"
                && entry.status.contains(&SkillInventoryStatus::Invalid)
        }));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("invalid frontmatter"))
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("missing SKILL.md"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_repo_tree_marks_nested_skill_dirs() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            "apps/demo/.claude/skills/nested-review",
            Some(
                r#"---
name: nested-review
---
"#,
            ),
            "nested review",
        )?;
        write_skill(
            repo.path(),
            "tools/.agents/skills/nested-agent",
            Some(
                r#"---
name: nested-agent
---
"#,
            ),
            "nested agent",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Claude, SkillInventoryTool::Codex],
                repo_tree: true,
            },
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Claude
                && entry.name == "nested-review"
                && entry.discovery_state == SkillDiscoveryState::NestedOnDemand
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.name == "nested-agent"
                && entry.discovery_state == SkillDiscoveryState::RepoTreeOnly
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_repo_tree_marks_duplicates_outside_current_scope() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_skill = r#"---
name: shared
---
"#;
        write_skill(
            repo.path(),
            ".agents/skills/current-shared",
            Some(shared_skill),
            "current",
        )?;
        write_skill(
            repo.path(),
            "apps/demo/.agents/skills/nested-shared",
            Some(shared_skill),
            "nested",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: true,
            },
        )?;

        let duplicate_entries: Vec<_> = report
            .entries
            .iter()
            .filter(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == "shared")
            .collect();
        assert_eq!(duplicate_entries.len(), 2);
        assert!(duplicate_entries.iter().all(|entry| {
            entry
                .status
                .contains(&SkillInventoryStatus::DuplicateVisible)
        }));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate visible skill 'shared'"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_repo_tree_discovers_symlinked_skill_roots() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_root = repo.path().join("skill-roots/shared-root");
        write_skill(
            repo.path(),
            "skill-roots/shared-root/symlinked-agent",
            Some(
                r#"---
name: symlinked-agent
---
"#,
            ),
            "symlinked agent",
        )?;
        ensure_dir(&repo.path().join("apps/demo/.agents"))?;
        create_symlink(&shared_root, &repo.path().join("apps/demo/.agents/skills")).unwrap();

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: true,
            },
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.name == "symlinked-agent"
                && entry.discovery_state == SkillDiscoveryState::RepoTreeOnly
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_outside_repo_still_lists_user_skills() -> Result<()> {
        let outside = Path::new("/");
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            home.path(),
            ".agents/skills/user-only",
            Some(
                r#"---
name: user-only
---
"#,
            ),
            "user only",
        )?;

        let report = take_inventory(
            outside,
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;

        assert!(report.repo_root.is_none());
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.scope == "user"
                && entry.name == "user-only"
        }));
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("repo root not detected"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_report_serializes_stable_shape() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/research",
            Some(
                r#"---
name: research
description: repo agent
---
"#,
            ),
            "repo agent",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;
        let json = serde_json::to_value(&report).unwrap();

        assert!(json.get("repo_root").is_some());
        assert!(json.get("mode").is_some());
        assert!(json.get("tools").is_some());
        assert!(json.get("entries").is_some());
        assert!(json.get("notes").is_some());
        assert!(json.get("warnings").is_some());

        let entry = json
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
            .unwrap();
        assert!(entry.get("tool").is_some());
        assert!(entry.get("scope").is_some());
        assert!(entry.get("discovery_state").is_some());
        assert!(entry.get("name").is_some());
        assert!(entry.get("description").is_some());
        assert!(entry.get("skill_dir").is_some());
        assert!(entry.get("skill_md").is_some());
        assert!(entry.get("skillenv_managed").is_some());
        assert!(entry.get("skillenv_origin").is_some());
        assert!(entry.get("status").is_some());
        Ok(())
    }

    /// Write a generated directory with a marker, the way `link` would, without
    /// going through a deployment. The point is to fix the two marker shapes the
    /// inventory has to read: v0 wrote one, v1 writes the other, and both exist on
    /// a machine that has migrated.
    fn write_marked_skill(target: &Path, name: &str, marker: &str) -> Result<()> {
        let dir = target.join(name);
        ensure_dir(&dir)?;
        for (file, body) in [
            ("SKILL.md", "---\ndescription: marked\n---\n"),
            (GENERATED_MARKER_FILE, marker),
        ] {
            let path = dir.join(file);
            fs::write(&path, body).map_err(|source| SkillenvError::WriteFile { path, source })?;
        }
        Ok(())
    }

    fn codex_entry<'a>(report: &'a SkillInventoryReport, name: &str) -> &'a SkillInventoryEntry {
        report
            .entries
            .iter()
            .find(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == name)
            .unwrap_or_else(|| panic!("no codex entry named {name}"))
    }

    /// A v0 marker records the path it was rendered from. That path is reported
    /// as-is rather than matched back against the scope directories it used to live
    /// under: inferring `repo:default` required the inventory to know the whole v0
    /// layout, and went wrong the moment those directories moved.
    #[test]
    fn inventory_reports_a_v0_marker_by_its_recorded_source_path() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));

        write_marked_skill(
            &repo.path().join(".agents/skills"),
            "skillenv-demo-default-research",
            r#"{"repo":"demo","scope":"default","skill":"research",
                "generated_name":"skillenv-demo-default-research",
                "source":"/somewhere/skillenv/default/research","strategy":"render"}"#,
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;
        let entry = codex_entry(&report, "skillenv-demo-default-research");
        assert!(entry.skillenv_managed);
        assert!(
            entry.skillenv_origin.starts_with("legacy:"),
            "unexpected origin: {}",
            entry.skillenv_origin
        );
        assert!(entry.skillenv_origin.contains("skillenv/default/research"));
        Ok(())
    }

    /// A v1 marker has no `repo` or `scope` field. Reading it must not fail: a
    /// marker that cannot be parsed is a directory that can never be identified as
    /// generated, and so can never be cleaned up. This shape was rejected once.
    #[test]
    fn inventory_reads_a_v1_marker_without_the_v0_fields() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));

        write_marked_skill(
            &repo.path().join(".agents/skills"),
            "skillenv-demo-research",
            r#"{"manifest":"demo-a1b2c3d4e5f6","skill":"research",
                "generated_name":"skillenv-demo-research","provider":"agents",
                "revision":"cea10b92","content_digest":"sha256:abc"}"#,
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;
        let entry = codex_entry(&report, "skillenv-demo-research");
        assert!(entry.skillenv_managed);
        assert_eq!(entry.skillenv_origin, "manifest:demo-a1b2c3d4e5f6");
        Ok(())
    }

    /// A directory carrying the prefix but no marker is still listed — it is
    /// visible to the tool, which is what this report is about — and is not claimed
    /// as skillenv's, because there is no evidence it is.
    #[test]
    fn inventory_does_not_claim_a_prefixed_directory_without_a_marker() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/skillenv-demo-handmade",
            Some("---\ndescription: mine\n---\n"),
            "mine",
        )?;

        let report = take_inventory(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
        )?;
        let entry = codex_entry(&report, "skillenv-demo-handmade");
        assert!(!entry.skillenv_managed);
        assert_eq!(entry.skillenv_origin, "manual");
        Ok(())
    }

    /// `init` writes a manifest a subsequent `link` can actually load, and leaves
    /// an existing one alone: it is the only hand-written input, so overwriting it
    /// would discard the whole configuration.
    #[test]
    fn init_writes_a_loadable_manifest_and_never_replaces_one() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));

        let first = init_manifest(repo.path())?;
        assert!(first.contains("created skillenv.toml"));
        assert!(first.contains("created skills/"));
        assert!(first.contains(".gitignore updated"));

        let manifest = repo.path().join("skillenv.toml");
        fs::write(&manifest, "[skillenv]\nversion = 1\n").unwrap();
        let second = init_manifest(repo.path())?;
        assert!(second.contains("already exists"));
        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            "[skillenv]\nversion = 1\n"
        );
        // Second run adds nothing: the entries are already there.
        assert!(!second.contains(".gitignore updated"));

        let gitignore = fs::read_to_string(repo.path().join(".gitignore")).unwrap();
        for pattern in V1_GITIGNORE {
            assert!(gitignore.contains(pattern), "missing {pattern}");
        }
        Ok(())
    }

    /// The template has to parse and resolve, or `init` hands the user a file that
    /// fails on the next command.
    #[test]
    fn the_init_template_is_a_valid_manifest() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        init_manifest(repo.path())?;

        assert!(has_manifest(repo.path()));
        // No skills declared yet, so this reports an empty catalog rather than
        // failing.
        let (listed, _) = status_manifest(repo.path())?;
        assert!(!listed.is_empty());
        let (_, findings) = lint_manifest(repo.path())?;
        assert!(!findings, "a fresh manifest should have no findings");
        Ok(())
    }

    /// `status` reports directories belonging to another manifest instead of
    /// hiding them. Under `$HOME` two repositories share one directory, and a count
    /// that silently excluded the other's would disagree with `ls`.
    #[test]
    fn status_reports_another_manifests_deployment_without_claiming_it() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        fs::write(
            repo.path().join("skillenv.toml"),
            "[skillenv]\nversion = 1\n\n[[deploy]]\ntarget = \"agents:repo\"\ninclude = [\"*\"]\n",
        )
        .unwrap();
        write_marked_skill(
            &repo.path().join(".agents/skills"),
            "skillenv-other-research",
            r#"{"manifest":"other-000000000000","skill":"research",
                "generated_name":"skillenv-other-research","provider":"agents"}"#,
        )?;

        let (text, problems) = status_manifest(repo.path())?;
        assert!(text.contains("0 deployed"), "unexpected report: {text}");
        assert!(text.contains("manifest other-000000000000"));
        assert!(!problems);
        Ok(())
    }

    /// `doctor` answers "why did it go there", so the manifest it chose and the
    /// cache it would read have to appear. The JSON form carries the same facts.
    #[test]
    fn doctor_reports_the_resolved_manifest_and_cache() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        init_manifest(repo.path())?;

        let text = doctor_manifest(repo.path(), false)?;
        assert!(text.contains("manifest:"));
        assert!(text.contains("skillenv.toml"));
        assert!(text.contains("cache:"));
        assert!(text.contains("0 source(s)"));

        let json: serde_json::Value =
            serde_json::from_str(&doctor_manifest(repo.path(), true)?).unwrap();
        assert_eq!(json["catalog"]["skills"], 0);
        assert_eq!(json["cache"]["sources"], 0);
        assert!(
            json["manifest"]
                .as_str()
                .unwrap()
                .ends_with("skillenv.toml")
        );
        Ok(())
    }

    /// Removing a `[[source]]` must not touch a skill that merely shares its name.
    ///
    /// Id uniqueness is only enforced between skill ids, and a source's label is a
    /// different namespace — so a source may be called `review-tools` while a
    /// *different* source contributes a skill with that id. Keying the lock removal
    /// on the parsed id rather than on which kind of entry was removed deleted that
    /// unrelated skill's lock entry, and the relink then took its directory too,
    /// reported only inside the removal's own count.
    ///
    /// The name must not appear as a `[[skill]]` entry, or the removal would match
    /// that first and never reach the source branch.
    #[test]
    fn removing_a_source_spares_another_sources_skill_of_the_same_name() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));

        fs::write(
            repo.path().join("skillenv.toml"),
            "[skillenv]\nversion = 1\n\n\
             [[source]]\nname = \"review-tools\"\nfrom = \"github:me/review-tools\"\n\
             skills = [\"formatter\"]\n\n\
             [[source]]\nname = \"other\"\nfrom = \"github:me/other-repo\"\n\
             skills = [\"review-tools\"]\n\n\
             [[deploy]]\ntarget = \"claude:home\"\ninclude = [\"*\"]\n",
        )
        .unwrap();
        // Both already fetched, as far as the lock is concerned. Written by hand so
        // the test needs no network.
        fs::write(
            repo.path().join("skillenv.lock"),
            r#"{"version":1,"skills":[
                {"id":"formatter","source":"github:me/review-tools",
                 "source_name":"review-tools","resolved_revision":"aaaa",
                 "content_digest":"sha256:aa","safeguard":{}},
                {"id":"review-tools","source":"github:me/other-repo",
                 "source_name":"other","resolved_revision":"bbbb",
                 "content_digest":"sha256:bb","safeguard":{}}]}"#,
        )
        .unwrap();

        remove_from_manifest(repo.path(), "review-tools")?;

        let manifest = fs::read_to_string(repo.path().join("skillenv.toml")).unwrap();
        assert!(
            !manifest.contains("github:me/review-tools"),
            "the source should be gone: {manifest}"
        );
        assert!(
            manifest.contains("github:me/other-repo"),
            "the other source must remain: {manifest}"
        );

        let lock = fs::read_to_string(repo.path().join("skillenv.lock")).unwrap();
        assert!(
            !lock.contains("\"formatter\""),
            "the removed source's own skill should go with it: {lock}"
        );
        assert!(
            lock.contains("\"review-tools\""),
            "the other source's identically-named skill must survive: {lock}"
        );
        assert!(lock.contains("\"other\""));
        Ok(())
    }

    /// A removal relinks, and that relink can block or skip a skill. Every other
    /// command turns that into a non-zero exit; this one must too, or a scripted
    /// caller cannot tell that something went wrong.
    #[test]
    fn removing_a_skill_reports_a_problem_the_relink_found() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));

        let dir = repo.path().join("skills/keeper");
        ensure_dir(&dir)?;
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: keeper\ndescription: Stays behind.\n---\n\nBody\n",
        )
        .unwrap();
        // `absent` is declared with no directory, so the relink cannot prepare it.
        fs::write(
            repo.path().join("skillenv.toml"),
            "[skillenv]\nversion = 1\n\n\
             [[skill]]\nname = \"keeper\"\nsource = \"local\"\n\n\
             [[skill]]\nname = \"absent\"\nsource = \"local\"\n\n\
             [[skill]]\nname = \"doomed\"\nsource = \"local\"\n\n\
             [[deploy]]\ntarget = \"claude:home\"\ninclude = [\"*\"]\n",
        )
        .unwrap();

        let report = remove_from_manifest(repo.path(), "doomed")?;
        assert!(report.summary.contains("removed skill doomed"));
        assert!(
            report.problems,
            "the relink could not prepare `absent`; that must reach the exit code"
        );
        assert!(
            report.warnings.iter().any(|line| line.contains("absent")),
            "got: {:?}",
            report.warnings
        );
        Ok(())
    }

    fn repo_fixture() -> Result<TempDir> {
        let dir = TempDir::new().unwrap();
        ensure_dir(&dir.path().join(".git"))?;
        Ok(dir)
    }

    fn write_skill(
        repo_root: &Path,
        relative: &str,
        skill_md: Option<&str>,
        fallback_body: &str,
    ) -> Result<PathBuf> {
        let dir = repo_root.join(relative);
        ensure_dir(&dir)?;
        let content = skill_md.unwrap_or(fallback_body);
        let skill_md_path = dir.join("SKILL.md");
        fs::write(&skill_md_path, content).map_err(|source| SkillenvError::WriteFile {
            path: skill_md_path,
            source,
        })?;
        Ok(dir)
    }
}
