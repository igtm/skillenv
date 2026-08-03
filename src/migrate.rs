//! Converting a v0 setup into a v1 manifest.
//!
//! Two orderings matter, and getting either wrong loses work.
//!
//! **Sweep before moving.** v0's deployments can only be recognised while the
//! layout they refer to still exists; once `skillenv/` has moved, every marker
//! points at a path that is gone. So the old deployments are cleared first, and
//! only then are files relocated. Do it the other way round and all 64
//! directories become orphans nothing can find.
//!
//! **Read the filesystem, not the config, for deploy rules.** v0's
//! `targets.{agents,claude}` is one global pair of booleans; it says nothing about
//! whether `$HOME` was ever linked. The only evidence is whether a `$HOME` target
//! directory actually holds this repository's entries.
//!
//! Planning is read-only and separate from applying, so the whole conversion can
//! be inspected before anything is written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::legacy_sweep::{self, SweepReport};
use crate::manifest::{MANIFEST_FILE, SkillId};
use crate::paths::slugify_or;
use crate::render::parse_frontmatter;
use crate::{Result, SkillenvError};

/// v0's layout constants, frozen here.
const V0_DIR: &str = "skillenv";
const V0_LOCK: &str = "skillenv.lock.json";
const V0_SCOPES: &[&str] = &["default", "local"];

/// v0's `skillenv.lock.json`, exactly as it was written.
#[derive(Debug, Deserialize)]
struct V0Lock {
    #[serde(default)]
    sources: Vec<V0Source>,
}

#[derive(Debug, Clone, Deserialize)]
struct V0Source {
    name: String,
    source: String,
    #[serde(default)]
    requested_ref: Option<String>,
    #[serde(default)]
    selected_skills: Vec<String>,
    #[serde(default)]
    resolved_revision: Option<String>,
}

/// A locally-authored skill to be relocated into the flat catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkill {
    pub id: SkillId,
    /// Where it lives now, e.g. `skillenv/default/draft-pr`.
    pub from: PathBuf,
    /// Where it will live, i.e. `skills/<id>`.
    pub to: PathBuf,
    pub description: Option<String>,
}

/// A managed source to be carried into the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePlan {
    pub name: String,
    /// The source string as the user originally wrote it.
    pub from: String,
    pub git_ref: Option<String>,
    /// Kept as an explicit list rather than `"*"`.
    ///
    /// v0 recorded expanded and hand-written selections identically, so which one
    /// this was cannot be recovered. Reproducing the explicit list keeps the
    /// migration faithful; choosing `"*"` would silently pull in every skill the
    /// source has gained since.
    pub skills: Vec<String>,
    pub revision: Option<String>,
}

/// A `[[deploy]]` rule inferred from a target directory that actually holds
/// entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployPlan {
    /// `<provider>:<scope>`.
    pub target: String,
    /// Directory the evidence came from.
    pub evidence: PathBuf,
    pub skill_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct MigrationPlan {
    pub root: PathBuf,
    pub local_skills: Vec<LocalSkill>,
    pub sources: Vec<SourcePlan>,
    pub deploys: Vec<DeployPlan>,
    /// One per v0 target directory, to be cleared before anything moves.
    pub legacy: Vec<SweepReport>,
    /// Things that must be resolved by hand before applying.
    pub blockers: Vec<String>,
    /// Actions for the user that this tool will not take on their behalf.
    pub manual_steps: Vec<String>,
}

impl MigrationPlan {
    pub fn can_apply(&self) -> bool {
        self.blockers.is_empty()
    }

    /// Total v0 deployments found, i.e. what will be cleared.
    pub fn legacy_count(&self) -> usize {
        self.legacy.iter().map(|report| report.entries.len()).sum()
    }
}

/// Inspect a v0 setup. Reads only.
pub fn plan(root: &Path, home: &Path) -> Result<MigrationPlan> {
    let mut plan = MigrationPlan {
        root: root.to_path_buf(),
        ..Default::default()
    };

    if root.join(MANIFEST_FILE).is_file() {
        plan.blockers.push(format!(
            "{MANIFEST_FILE} already exists; this repository has already been migrated"
        ));
        return Ok(plan);
    }
    let v0 = root.join(V0_DIR);
    if !v0.is_dir() {
        plan.blockers
            .push(format!("no {V0_DIR}/ directory; nothing to migrate"));
        return Ok(plan);
    }

    // Profiles have no equivalent, and inventing labels for them would be
    // guessing. Refuse rather than silently flatten them together.
    let profiles = v0.join("profiles");
    if let Ok(entries) = fs::read_dir(&profiles) {
        let named: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        if !named.is_empty() {
            plan.blockers.push(format!(
                "profiles are in use ({}); decide what label each should become and \
                 declare them in {MANIFEST_FILE} by hand, because collapsing them \
                 automatically would lose the distinction",
                named.join(", ")
            ));
        }
    }

    plan.local_skills = collect_local_skills(&v0)?;
    plan.sources = read_v0_lock(root)?;
    plan.deploys = infer_deploys(root, home)?;
    plan.legacy = sweep_targets(root, home)?;
    plan.manual_steps = manual_steps(root)?;

    if plan.local_skills.is_empty() && plan.sources.is_empty() {
        plan.blockers
            .push("found no skills and no managed sources to migrate".to_string());
    }
    Ok(plan)
}

/// Locally-authored skills under `default/` and `local/`.
///
/// Each scope is read independently. v0's own reader called into `default/`
/// unconditionally whenever any scope directory existed, so a repository holding
/// only `local/` failed with a read error.
fn collect_local_skills(v0: &Path) -> Result<Vec<LocalSkill>> {
    let mut skills = Vec::new();
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();

    for scope in V0_SCOPES {
        let scope_dir = v0.join(scope);
        let Ok(entries) = fs::read_dir(&scope_dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let dir = entry.path();
            let skill_md = dir.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let raw_name = entry.file_name().to_string_lossy().to_string();
            let id = SkillId::parse(&slugify_or(&raw_name, "skill")).map_err(|_| {
                SkillenvError::InvalidSkillId {
                    input: raw_name.clone(),
                    reason: format!(
                        "cannot be converted automatically; rename {} and run migrate again",
                        dir.display()
                    ),
                }
            })?;

            // The flat namespace means `default/x` and `local/x` can no longer
            // coexist. Reported rather than silently resolved either way.
            if let Some(first) = seen.get(id.as_str()) {
                return Err(SkillenvError::DuplicateSkillId {
                    id: id.to_string(),
                    first: first.display().to_string(),
                    second: dir.display().to_string(),
                });
            }
            seen.insert(id.as_str().to_string(), dir.clone());

            skills.push(LocalSkill {
                to: PathBuf::from("skills").join(id.as_str()),
                description: read_description(&skill_md),
                id,
                from: dir,
            });
        }
    }
    Ok(skills)
}

/// The `description` from a skill's frontmatter, if it has a usable one.
fn read_description(skill_md: &Path) -> Option<String> {
    let raw = fs::read_to_string(skill_md).ok()?;
    // A skill whose frontmatter does not parse still migrates; `lint` will report
    // it afterwards. Failing the whole migration over one malformed file would be
    // the same mistake v0 made with `link`.
    let (frontmatter, _) = parse_frontmatter(skill_md, &raw).ok()?;
    frontmatter
        .get(serde_yaml::Value::String("description".to_string()))
        .and_then(serde_yaml::Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_v0_lock(root: &Path) -> Result<Vec<SourcePlan>> {
    let path = root.join(V0_LOCK);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(SkillenvError::ReadFile { path, source }),
    };
    let lock: V0Lock =
        serde_json::from_str(&raw).map_err(|source| SkillenvError::ParseLock { path, source })?;

    Ok(lock
        .sources
        .into_iter()
        .map(|source| SourcePlan {
            name: source.name,
            from: source.source,
            git_ref: source.requested_ref,
            skills: source.selected_skills,
            revision: source
                .resolved_revision
                .filter(|revision| revision != "unversioned"),
        })
        .collect())
}

/// v0's four possible target directories, paired with the v1 target they become.
fn v0_targets(root: &Path, home: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (root.join(".agents/skills"), "agents:repo"),
        (root.join(".claude/skills"), "claude:repo"),
        (home.join(".agents/skills"), "agents:home"),
        (home.join(".claude/skills"), "claude:home"),
    ]
}

/// Infer deploy rules from directories that actually hold this repository's
/// entries.
fn infer_deploys(root: &Path, home: &Path) -> Result<Vec<DeployPlan>> {
    let slug = repo_slug(root);
    let mut plans = Vec::new();
    for (path, target) in v0_targets(root, home) {
        let report = legacy_sweep::sweep(&path, &slug)?;
        if !report.entries.is_empty() {
            plans.push(DeployPlan {
                target: target.to_string(),
                evidence: path,
                skill_count: report.entries.len(),
            });
        }
    }
    Ok(plans)
}

fn sweep_targets(root: &Path, home: &Path) -> Result<Vec<SweepReport>> {
    let slug = repo_slug(root);
    v0_targets(root, home)
        .into_iter()
        .map(|(path, _)| legacy_sweep::sweep(&path, &slug))
        .filter(|report| match report {
            Ok(report) => !report.is_empty(),
            Err(_) => true,
        })
        .collect()
}

fn repo_slug(root: &Path) -> String {
    slugify_or(
        root.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repo"),
        "repo",
    )
}

/// Actions left to the user.
fn manual_steps(root: &Path) -> Result<Vec<String>> {
    let mut steps = Vec::new();

    // A .gitignore entry does not untrack an already-committed file, so a tracked
    // remote tree would stay in history and keep showing up in diffs.
    let tracked = tracked_remote_files(root);
    if !tracked.is_empty() {
        steps.push(format!(
            "{} file(s) under {V0_DIR}/remote are committed; run \
             `git rm -r --cached {V0_DIR}/remote` so the cache stops being tracked \
             (a .gitignore entry alone will not untrack them)",
            tracked.len()
        ));
    }
    steps.push(format!(
        "review {MANIFEST_FILE}, then run `skillenv link` and compare against the \
         recorded plan"
    ));
    Ok(steps)
}

/// Files under `skillenv/remote` that git is tracking.
///
/// Best effort: if git is unavailable there is simply nothing to advise.
fn tracked_remote_files(root: &Path) -> Vec<String> {
    let Ok(output) = std::process::Command::new("git")
        .args(["ls-files", &format!("{V0_DIR}/remote")])
        .current_dir(root)
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Remove the v0 layout once the conversion is in place.
    ///
    /// Off by default: leaving `skillenv/` alone means the conversion can be
    /// checked, and reverted by deleting two files, before anything is lost.
    pub prune: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    pub cleared: usize,
    pub skills_copied: Vec<SkillId>,
    pub cached: Vec<SkillId>,
    /// Locked entries that could not be seeded from the v0 tree, so a `fetch` is
    /// needed before they can deploy.
    pub needs_fetch: Vec<SkillId>,
    pub pruned: bool,
    pub notes: Vec<String>,
}

/// Carry out a plan.
///
/// Order is load-bearing: v0's deployments are cleared first, while their markers
/// still refer to a layout that exists. Only then is anything relocated.
pub fn apply(plan: &MigrationPlan, options: &ApplyOptions) -> Result<MigrationReport> {
    if !plan.can_apply() {
        return Err(SkillenvError::InvalidManifest {
            path: plan.root.join(MANIFEST_FILE),
            message: format!("migration is blocked: {}", plan.blockers.join("; ")),
        });
    }
    let mut report = MigrationReport::default();

    // 1. Clear v0's deployments while their markers can still be matched.
    for sweep in &plan.legacy {
        report.cleared += legacy_sweep::remove(sweep)?;
        for path in &sweep.unmarked {
            report.notes.push(format!(
                "left {} in place: it carries the prefix but no marker, so there is no \
                 evidence skillenv created it",
                path.display()
            ));
        }
    }

    // 2. Copy local skills into the flat catalog. Copied, not moved, so the v0
    //    layout stays intact until `--prune`.
    for skill in &plan.local_skills {
        let destination = plan.root.join(&skill.to);
        copy_tree(&skill.from, &destination)?;
        report.skills_copied.push(skill.id.clone());
    }

    // 3. Seed the cache from v0's vendored copies, so `link` works offline
    //    immediately after migrating rather than needing a fetch first.
    let mut lock = crate::lock::LockFile::default();
    for source in &plan.sources {
        for raw_id in &source.skills {
            let Ok(id) = SkillId::parse(raw_id) else {
                report
                    .notes
                    .push(format!("skipped '{raw_id}': not a usable skill id"));
                continue;
            };
            let Some(revision) = &source.revision else {
                report.needs_fetch.push(id);
                continue;
            };
            match v0_installed_skill(&plan.root, &source.name, raw_id) {
                Some(from) => {
                    let cache = crate::source::cache_dir(&plan.root, &source.name, revision)
                        .join(id.as_str());
                    copy_tree(&from, &cache)?;
                    lock.upsert(crate::lock::LockedSkill {
                        id: id.clone(),
                        source: normalize_source(&source.from),
                        source_name: Some(source.name.clone()),
                        resolved_ref: source.git_ref.clone(),
                        resolved_revision: Some(revision.clone()),
                        content_digest: crate::lock::digest_tree(&cache)?,
                        safeguard: crate::lock::SafeguardState::default(),
                    });
                    report.cached.push(id);
                }
                None => report.needs_fetch.push(id),
            }
        }
    }

    // 4. Write the manifest and the lock.
    let manifest_path = plan.root.join(MANIFEST_FILE);
    fs::write(&manifest_path, render_manifest(plan)).map_err(|source| {
        SkillenvError::WriteFile {
            path: manifest_path,
            source,
        }
    })?;
    lock.save(&plan.root)?;

    // 5. The cache is generated content and must not be committed.
    if add_gitignore_entry(&plan.root, ".skillenv/")? {
        report
            .notes
            .push(".gitignore: added .skillenv/ so the cache is not tracked".to_string());
    }

    if options.prune {
        let v0 = plan.root.join(V0_DIR);
        if v0.is_dir() {
            fs::remove_dir_all(&v0)
                .map_err(|source| SkillenvError::WriteFile { path: v0, source })?;
            report.pruned = true;
        }
    } else {
        report.notes.push(format!(
            "{V0_DIR}/ was left in place; remove it with --prune once the result is \
             confirmed"
        ));
    }

    Ok(report)
}

/// Where v0 installed one skill of a managed source.
///
/// Both scopes are probed independently, since a source may have used either.
fn v0_installed_skill(root: &Path, source_name: &str, skill: &str) -> Option<PathBuf> {
    let install_root = root.join(V0_DIR).join("remote").join(source_name);
    V0_SCOPES
        .iter()
        .map(|scope| install_root.join(scope).join(skill))
        .find(|candidate| candidate.join("SKILL.md").is_file())
}

/// Copy a directory tree, refusing symlinks rather than following them.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    crate::paths::ensure_dir(to)?;
    for entry in walkdir::WalkDir::new(from).follow_links(false) {
        let entry = entry.map_err(|error| SkillenvError::ReadFile {
            path: from.to_path_buf(),
            source: std::io::Error::other(error),
        })?;
        let relative =
            entry
                .path()
                .strip_prefix(from)
                .map_err(|error| SkillenvError::ReadFile {
                    path: from.to_path_buf(),
                    source: std::io::Error::other(error),
                })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = to.join(relative);
        if entry.file_type().is_dir() {
            crate::paths::ensure_dir(&target)?;
            continue;
        }
        // A symlink in a v0 tree would be carried into the catalog, where the
        // acceptance checks refuse them anyway; skipping keeps the copy honest.
        if entry.file_type().is_symlink() {
            continue;
        }
        if let Some(parent) = target.parent() {
            crate::paths::ensure_dir(parent)?;
        }
        fs::copy(entry.path(), &target).map_err(|source| SkillenvError::WriteFile {
            path: target,
            source,
        })?;
    }
    Ok(())
}

/// Append one `.gitignore` line if it is not already there.
fn add_gitignore_entry(root: &Path, entry: &str) -> Result<bool> {
    let path = root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(false);
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(entry);
    updated.push('\n');
    fs::write(&path, updated).map_err(|source| SkillenvError::WriteFile { path, source })?;
    Ok(true)
}

/// Human-readable rendering of what an apply did.
pub fn format_report(report: &MigrationReport) -> String {
    let mut lines = vec![format!(
        "cleared {} v0 deployment(s), copied {} skill(s) into skills/, seeded {} from the \
         v0 cache",
        report.cleared,
        report.skills_copied.len(),
        report.cached.len()
    )];
    if !report.needs_fetch.is_empty() {
        lines.push(format!(
            "run `skillenv fetch` for: {}",
            report
                .needs_fetch
                .iter()
                .map(SkillId::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if report.pruned {
        lines.push(format!("removed {V0_DIR}/"));
    }
    for note in &report.notes {
        lines.push(format!("  - {note}"));
    }
    lines.push("next: `skillenv lint`, then `skillenv link`".to_string());
    lines.join("\n")
}

/// Render the manifest this plan describes.
///
/// Written out as text rather than serialized, so the result carries the comments
/// that explain why it looks the way it does. The file is meant to be edited by
/// hand afterwards.
pub fn render_manifest(plan: &MigrationPlan) -> String {
    let mut out = String::from("[skillenv]\nversion = 1\n");

    for skill in &plan.local_skills {
        out.push_str(&format!(
            "\n[[skill]]\nname = \"{}\"\nsource = \"local\"\n",
            skill.id
        ));
        if let Some(description) = &skill.description {
            // Only carried when the source has no frontmatter of its own to read;
            // kept here so a hand-edit has the text to hand.
            out.push_str(&format!("# description: {}\n", one_line(description)));
        }
    }

    for source in &plan.sources {
        out.push_str(&format!(
            "\n[[source]]\nname = \"{}\"\nfrom = \"{}\"\n",
            source.name,
            normalize_source(&source.from)
        ));
        if let Some(git_ref) = &source.git_ref {
            out.push_str(&format!("ref = \"{git_ref}\"\n"));
        }
        let listed = source
            .skills
            .iter()
            .map(|skill| format!("\"{skill}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("skills = [{listed}]\n"));
        out.push_str(
            "# Kept as an explicit list because v0 recorded expanded and hand-written\n\
             # selections identically. Change to skills = \"*\" to follow this source.\n",
        );
    }

    if plan.deploys.is_empty() {
        out.push_str(
            "\n# No target directory held this repository's skills, so no deploy rule\n\
             # could be inferred. Add one, e.g.:\n\
             # [[deploy]]\n# target = \"claude:home\"\n# include = [\"*\"]\n",
        );
    }
    for deploy in &plan.deploys {
        out.push_str(&format!(
            "\n[[deploy]]\ntarget = \"{}\"\ninclude = [\"*\"]\n",
            deploy.target
        ));
        out.push_str(&format!(
            "# Inferred from {} skill(s) found in {}\n",
            deploy.skill_count,
            deploy.evidence.display()
        ));
    }
    out
}

/// v0 accepted bare `owner/repo`; v1 wants the scheme spelled out.
fn normalize_source(raw: &str) -> String {
    if raw.contains("://") || raw.starts_with("git@") || raw.starts_with("github:") {
        return raw.to_string();
    }
    match raw.split('/').collect::<Vec<_>>().as_slice() {
        [owner, repo] if !owner.is_empty() && !repo.is_empty() => {
            format!("github:{owner}/{repo}")
        }
        _ => raw.to_string(),
    }
}

fn one_line(value: &str) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 100 {
        flat
    } else {
        format!("{}…", flat.chars().take(100).collect::<String>())
    }
}

/// Human-readable rendering of a plan.
pub fn format_plan(plan: &MigrationPlan) -> String {
    let mut lines = vec![format!("migrating {}", plan.root.display())];

    if !plan.blockers.is_empty() {
        lines.push(String::new());
        lines.push("cannot proceed:".to_string());
        for blocker in &plan.blockers {
            lines.push(format!("  - {blocker}"));
        }
        return lines.join("\n");
    }

    lines.push(format!(
        "\n{} local skill(s) move into skills/:",
        plan.local_skills.len()
    ));
    for skill in &plan.local_skills {
        lines.push(format!(
            "  {} -> {}",
            skill.from.display(),
            skill.to.display()
        ));
    }

    lines.push(format!("\n{} managed source(s):", plan.sources.len()));
    for source in &plan.sources {
        let revision = source
            .revision
            .as_deref()
            .map(|revision| &revision[..12.min(revision.len())])
            .unwrap_or("unversioned");
        lines.push(format!(
            "  {} from {} at {revision}, skills = [{}]",
            source.name,
            normalize_source(&source.from),
            source.skills.join(", ")
        ));
    }

    lines.push(format!("\n{} deploy rule(s) inferred:", plan.deploys.len()));
    for deploy in &plan.deploys {
        lines.push(format!(
            "  {} ({} skill(s) currently in {})",
            deploy.target,
            deploy.skill_count,
            deploy.evidence.display()
        ));
    }

    lines.push(format!(
        "\n{} v0 deployment(s) will be cleared first, before anything moves:",
        plan.legacy_count()
    ));
    for report in &plan.legacy {
        lines.push(format!(
            "  {} ({} entries{})",
            report.target.display(),
            report.entries.len(),
            if report.unmarked.is_empty() {
                String::new()
            } else {
                format!(", {} left alone for review", report.unmarked.len())
            }
        ));
    }

    if !plan.manual_steps.is_empty() {
        lines.push("\nafterwards:".to_string());
        for step in &plan.manual_steps {
            lines.push(format!("  - {step}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Build a v0 setup: skills, a lock, and deployments in two targets.
    fn v0_setup() -> (TempDir, TempDir) {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let slug = repo_slug(root.path());
        // v0 required a git repository, and a repo-scoped deploy rule still needs
        // one to resolve a directory.
        fs::create_dir_all(root.path().join(".git")).unwrap();

        for (scope, id, description) in [
            ("default", "draft-pr", "Draft PR を作る"),
            ("default", "japanese-tech-writing", "文章規範"),
        ] {
            let dir = root.path().join(V0_DIR).join(scope).join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {id}\ndescription: {description}\n---\n\nBody\n"),
            )
            .unwrap();
        }

        fs::write(
            root.path().join(V0_LOCK),
            serde_json::to_string_pretty(&json!({
                "version": 1,
                "sources": [{
                    "name": "kinko",
                    "source": "igtm/kinko",
                    "kind": "git",
                    "transport": "https://github.com/igtm/kinko.git",
                    "requested_ref": null,
                    "subdir": null,
                    "install_root": "skillenv/remote/kinko",
                    "selected_skills": ["kinko"],
                    "resolved_revision": "71947fdd89bebe9f58a2efa0404c36cb7e24b099",
                }],
            }))
            .unwrap(),
        )
        .unwrap();

        // Deployed in the repo and in $HOME, which is the only evidence that both
        // were in use.
        for (base, name) in [
            (
                root.path().join(".claude/skills"),
                format!("skillenv-{slug}-default-draft-pr"),
            ),
            (
                home.path().join(".claude/skills"),
                format!("skillenv-{slug}-gabc123456789-default-draft-pr"),
            ),
        ] {
            let dir = base.join(&name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(".skillenv-generated.json"),
                serde_json::to_string(&json!({
                    "repo": slug,
                    "scope": "default",
                    "skill": "draft-pr",
                    "generated_name": name,
                    "source": format!("{}/skillenv/default/draft-pr", root.path().display()),
                    "strategy": "render",
                }))
                .unwrap(),
            )
            .unwrap();
        }
        (root, home)
    }

    #[test]
    fn a_plan_reads_skills_sources_and_deployments() -> Result<()> {
        let (root, home) = v0_setup();
        let plan = plan(root.path(), home.path())?;

        assert!(plan.can_apply(), "blockers: {:?}", plan.blockers);
        let ids: Vec<_> = plan
            .local_skills
            .iter()
            .map(|skill| skill.id.to_string())
            .collect();
        assert_eq!(ids, vec!["draft-pr", "japanese-tech-writing"]);
        assert_eq!(plan.local_skills[0].to, PathBuf::from("skills/draft-pr"));
        assert_eq!(
            plan.local_skills[0].description.as_deref(),
            Some("Draft PR を作る")
        );

        assert_eq!(plan.sources.len(), 1);
        assert_eq!(plan.sources[0].skills, vec!["kinko".to_string()]);
        assert!(
            plan.sources[0]
                .revision
                .as_deref()
                .unwrap()
                .starts_with("71947fdd")
        );
        Ok(())
    }

    /// The config cannot say whether $HOME was linked; only the filesystem can.
    #[test]
    fn deploy_rules_come_from_directories_that_actually_hold_entries() -> Result<()> {
        let (root, home) = v0_setup();
        let plan = plan(root.path(), home.path())?;

        let targets: Vec<_> = plan.deploys.iter().map(|d| d.target.as_str()).collect();
        assert_eq!(targets, vec!["claude:repo", "claude:home"]);
        // .agents/skills was never used here, so no rule is invented for it.
        assert!(!targets.contains(&"agents:home"));
        Ok(())
    }

    /// These must be cleared before files move, because afterwards every marker
    /// points at a path that no longer exists.
    #[test]
    fn the_plan_accounts_for_the_v0_deployments_to_clear() -> Result<()> {
        let (root, home) = v0_setup();
        let plan = plan(root.path(), home.path())?;
        assert_eq!(plan.legacy_count(), 2);
        Ok(())
    }

    #[test]
    fn an_already_migrated_repository_is_refused() -> Result<()> {
        let (root, home) = v0_setup();
        fs::write(root.path().join(MANIFEST_FILE), "[skillenv]\nversion = 1\n").unwrap();
        let plan = plan(root.path(), home.path())?;
        assert!(!plan.can_apply());
        assert!(
            plan.blockers[0].contains("already been migrated"),
            "unexpected: {:?}",
            plan.blockers
        );
        Ok(())
    }

    /// Profiles carry a distinction that cannot be recovered, so the migration
    /// stops rather than collapsing them.
    #[test]
    fn profiles_block_the_migration_and_name_themselves() -> Result<()> {
        let (root, home) = v0_setup();
        fs::create_dir_all(root.path().join(V0_DIR).join("profiles/review")).unwrap();
        let plan = plan(root.path(), home.path())?;
        assert!(!plan.can_apply());
        assert!(
            plan.blockers.iter().any(|b| b.contains("review")),
            "unexpected: {:?}",
            plan.blockers
        );
        Ok(())
    }

    /// The flat namespace cannot hold `default/x` and `local/x` at once.
    #[test]
    fn the_same_id_in_two_scopes_is_reported() {
        let (root, home) = v0_setup();
        let dir = root.path().join(V0_DIR).join("local/draft-pr");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: draft-pr\n---\n\nBody\n").unwrap();

        let error = plan(root.path(), home.path()).unwrap_err().to_string();
        assert!(error.contains("draft-pr"), "unexpected: {error}");
        assert!(error.contains("twice"), "unexpected: {error}");
    }

    /// A skill whose frontmatter is malformed still migrates; `lint` reports it
    /// afterwards. Refusing the whole conversion over one file would repeat v0's
    /// mistake.
    #[test]
    fn a_malformed_skill_still_migrates_without_a_description() -> Result<()> {
        let (root, home) = v0_setup();
        let dir = root.path().join(V0_DIR).join("default/broken");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: broken\ndescription: Agent Skill: broken\n---\n\nBody\n",
        )
        .unwrap();

        let plan = plan(root.path(), home.path())?;
        let broken = plan
            .local_skills
            .iter()
            .find(|skill| skill.id.as_str() == "broken")
            .expect("it should still be migrated");
        assert_eq!(broken.description, None);
        Ok(())
    }

    #[test]
    fn the_rendered_manifest_keeps_the_explicit_skill_list_and_says_why() -> Result<()> {
        let (root, home) = v0_setup();
        let rendered = render_manifest(&plan(root.path(), home.path())?);

        assert!(rendered.contains("[skillenv]\nversion = 1"));
        assert!(rendered.contains("name = \"draft-pr\"\nsource = \"local\""));
        // Bare owner/repo becomes an explicit scheme.
        assert!(
            rendered.contains("from = \"github:igtm/kinko\""),
            "got:\n{rendered}"
        );
        assert!(rendered.contains("skills = [\"kinko\"]"));
        assert!(
            rendered.contains("Change to skills = \"*\""),
            "the reason should be in the file:\n{rendered}"
        );
        assert!(rendered.contains("target = \"claude:home\""));
        Ok(())
    }

    #[test]
    fn a_setup_without_deployments_gets_a_commented_placeholder() -> Result<()> {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let dir = root.path().join(V0_DIR).join("default/only");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: only\n---\n\nBody\n").unwrap();

        let plan = plan(root.path(), home.path())?;
        assert!(plan.deploys.is_empty());
        let rendered = render_manifest(&plan);
        assert!(
            rendered.contains("no deploy rule"),
            "should explain the gap:\n{rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_repository_with_nothing_to_migrate_is_refused() -> Result<()> {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let plan = plan(root.path(), home.path())?;
        assert!(!plan.can_apply());
        assert!(plan.blockers[0].contains("nothing to migrate"));
        Ok(())
    }

    #[test]
    fn the_plan_reads_as_a_summary() -> Result<()> {
        let (root, home) = v0_setup();
        let text = format_plan(&plan(root.path(), home.path())?);
        assert!(text.contains("2 local skill(s)"), "got:\n{text}");
        assert!(text.contains("1 managed source(s)"), "got:\n{text}");
        assert!(text.contains("2 deploy rule(s)"), "got:\n{text}");
        assert!(
            text.contains("before anything moves"),
            "the ordering guarantee should be visible:\n{text}"
        );
        Ok(())
    }

    /// The conversion has to be checkable, not just plausible: the skills
    /// deployed afterwards must be the same set that was deployed before.
    #[test]
    fn applying_reproduces_the_same_skill_set() -> Result<()> {
        let (root, home) = v0_setup();
        // Give the managed source a vendored copy, as v0 would have.
        let installed = root.path().join(V0_DIR).join("remote/kinko/default/kinko");
        fs::create_dir_all(&installed).unwrap();
        fs::write(
            installed.join("SKILL.md"),
            "---\nname: kinko\ndescription: Stores secrets\n---\n\nBody\n",
        )
        .unwrap();

        let before = plan(root.path(), home.path())?;
        let deployed_before: Vec<String> = before
            .legacy
            .iter()
            .flat_map(|sweep| sweep.entries.iter().map(|entry| entry.skill.clone()))
            .collect();
        assert!(!deployed_before.is_empty());

        let report = apply(&before, &ApplyOptions::default())?;

        // v0's deployments are gone, and the catalog is in place.
        assert_eq!(report.cleared, 2);
        assert_eq!(report.skills_copied.len(), 2);
        assert!(root.path().join("skills/draft-pr/SKILL.md").is_file());
        assert!(root.path().join(MANIFEST_FILE).is_file());
        assert!(root.path().join("skillenv.lock").is_file());

        // The managed skill was seeded from the v0 tree, so no fetch is required.
        assert_eq!(report.cached.len(), 1, "{report:?}");
        assert!(report.needs_fetch.is_empty(), "{report:?}");

        // v0 is untouched until --prune.
        assert!(root.path().join(V0_DIR).is_dir());
        assert!(!report.pruned);

        // The cache is generated content, so it must not be tracked.
        let gitignore = fs::read_to_string(root.path().join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|line| line.trim() == ".skillenv/"));

        // And the new engine deploys the same skills, offline.
        let mut session = crate::session::Session::open(root.path(), home.path().to_path_buf())?;
        let linked = session.link()?;
        let deployed_after: Vec<String> = linked
            .targets
            .iter()
            .flat_map(|target| target.written.iter().map(SkillId::to_string))
            .collect();
        assert!(
            deployed_after.contains(&"draft-pr".to_string()),
            "expected the previously-deployed skill back: {deployed_after:?}"
        );
        assert!(
            deployed_after.contains(&"kinko".to_string()),
            "the seeded managed skill should deploy without a fetch: {deployed_after:?}"
        );
        assert!(linked.unavailable.is_empty(), "{:?}", linked.unavailable);
        Ok(())
    }

    #[test]
    fn prune_removes_the_v0_layout_only_when_asked() -> Result<()> {
        let (root, home) = v0_setup();
        let plan = plan(root.path(), home.path())?;
        let report = apply(&plan, &ApplyOptions { prune: true })?;
        assert!(report.pruned);
        assert!(!root.path().join(V0_DIR).exists());
        Ok(())
    }

    /// A source with no recorded revision cannot be seeded, so it is named as
    /// needing a fetch rather than silently omitted.
    #[test]
    fn an_unversioned_source_is_reported_as_needing_a_fetch() -> Result<()> {
        let (root, home) = v0_setup();
        fs::write(
            root.path().join(V0_LOCK),
            serde_json::to_string(&json!({
                "version": 1,
                "sources": [{
                    "name": "local-src",
                    "source": "../shared",
                    "kind": "local",
                    "transport": "/tmp/shared",
                    "requested_ref": null,
                    "subdir": null,
                    "install_root": "skillenv/remote/local-src",
                    "selected_skills": ["shared-skill"],
                    "resolved_revision": "unversioned",
                }],
            }))
            .unwrap(),
        )
        .unwrap();

        let plan = plan(root.path(), home.path())?;
        let report = apply(&plan, &ApplyOptions::default())?;
        assert_eq!(
            report
                .needs_fetch
                .iter()
                .map(SkillId::to_string)
                .collect::<Vec<_>>(),
            vec!["shared-skill"]
        );
        Ok(())
    }

    #[test]
    fn applying_a_blocked_plan_is_refused() -> Result<()> {
        let (root, home) = v0_setup();
        fs::create_dir_all(root.path().join(V0_DIR).join("profiles/review")).unwrap();
        let plan = plan(root.path(), home.path())?;
        let error = apply(&plan, &ApplyOptions::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("blocked"), "unexpected: {error}");
        // Nothing was written.
        assert!(!root.path().join(MANIFEST_FILE).exists());
        Ok(())
    }

    #[test]
    fn bare_owner_repo_gains_an_explicit_scheme() {
        assert_eq!(normalize_source("igtm/kinko"), "github:igtm/kinko");
        assert_eq!(
            normalize_source("https://github.com/o/r"),
            "https://github.com/o/r"
        );
        assert_eq!(
            normalize_source("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
    }
}
