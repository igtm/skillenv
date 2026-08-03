//! Finding and clearing what v0 deployed.
//!
//! This exists because of a specific way the migration would otherwise break a
//! working setup. v0's removal predicate required three things at once: the
//! directory name carried the expected prefix, the marker's `scope` matched the
//! scope filter, and the marker's `source` pointed inside a currently-known
//! source root.
//!
//! After migration none of the last two hold. Scopes are gone, and a real marker
//! reads
//!
//! ```json
//! { "source": "…/dotfiles/skillenv/remote/kinko/default/kinko", … }
//! ```
//!
//! which is under neither `skills/` nor `.skillenv/cache/`. So v1.0's own cleanup
//! would decline to touch any of it, `link` would report "linked N, removed 0",
//! and every target directory would end up holding both generations — half of
//! them pointing at a tree that no longer exists.
//!
//! The v0 marker schema is frozen here on purpose. This module is the only place
//! that understands it, it never shares types with the live format, and it can be
//! deleted once no v0 deployment remains in the wild.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Result, SkillenvError};

/// v0's marker filename. Unchanged in v1.0, which is why a sweep can find these.
const V0_MARKER_FILE: &str = ".skillenv-generated.json";

/// v0's `.skillenv-generated.json`, exactly as it was written.
///
/// `source` and `strategy` are read but deliberately not acted on: `source` is
/// the field whose staleness caused the problem, and `strategy` distinguishes the
/// symlink mode v1.0 drops.
#[derive(Debug, Clone, Deserialize)]
struct V0Marker {
    repo: String,
    scope: String,
    skill: String,
    generated_name: String,
    source: String,
    strategy: String,
}

/// One v0 deployment found in a target directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyEntry {
    pub path: PathBuf,
    pub dir_name: String,
    /// The repository slug v0 recorded.
    pub repo: String,
    /// v0's scope: `default`, `local`, or `profile:<name>`.
    pub scope: String,
    pub skill: String,
    /// Where v0 rendered it from. Advisory only — it is usually stale by the time
    /// a sweep runs, which is the whole reason this module exists.
    pub source: String,
    /// `render` or `symlink`.
    pub strategy: String,
}

impl LegacyEntry {
    /// Whether this came from v0's symlink strategy, which v1.0 does not have.
    pub fn was_symlink(&self) -> bool {
        self.strategy == "symlink"
    }

    /// v0's scope as a label, for a migration that wants to preserve grouping.
    ///
    /// `profile:review` becomes `review`; the two built-in scopes carry no
    /// meaning worth keeping as a label.
    pub fn scope_label(&self) -> Option<String> {
        self.scope
            .strip_prefix("profile:")
            .map(|name| name.to_string())
    }
}

/// What a sweep found in one directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub target: PathBuf,
    pub entries: Vec<LegacyEntry>,
    /// Directories carrying the prefix but no readable marker.
    ///
    /// Reported rather than removed: without a marker there is no evidence
    /// skillenv created it, and v0 had a bug that left exactly this shape behind
    /// when a render failed part-way.
    pub unmarked: Vec<PathBuf>,
}

impl SweepReport {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.unmarked.is_empty()
    }
}

/// Find v0 deployments for `repo_slug` in `target`.
///
/// Matching is by marker `repo` plus the generated-name prefix, and explicitly
/// **not** by `source`. Both of v0's name shapes are recognised:
/// `skillenv-<repo>-<scope>-<skill>` for a repository target and
/// `skillenv-<repo>-g<hash>-<scope>-<skill>` for a `$HOME` one.
pub fn sweep(target: &Path, repo_slug: &str) -> Result<SweepReport> {
    let mut report = SweepReport {
        target: target.to_path_buf(),
        ..Default::default()
    };
    if !target.is_dir() {
        return Ok(report);
    }

    let prefix = format!("skillenv-{repo_slug}-");
    let mut entries: Vec<_> = fs::read_dir(target)
        .map_err(|source| SkillenvError::ReadFile {
            path: target.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        match read_marker(&path)? {
            // A marker naming a different repository belongs to that repository's
            // deployment, not ours; $HOME is shared machine-wide.
            Some(marker) if marker.repo != repo_slug => {}

            // The marker should name the directory it sits in. If it does not,
            // something renamed one of the two, and we no longer know what this
            // directory is — so it is reported for a human rather than deleted.
            Some(marker) if marker.generated_name != name => report.unmarked.push(path),

            Some(marker) => report.entries.push(LegacyEntry {
                path,
                dir_name: name,
                repo: marker.repo,
                scope: marker.scope,
                skill: marker.skill,
                source: marker.source,
                strategy: marker.strategy,
            }),
            None => report.unmarked.push(path),
        }
    }

    Ok(report)
}

/// Remove the entries a sweep found.
///
/// Only marked directories are removed. Anything in `unmarked` is left alone and
/// reported, because there is no evidence skillenv created it.
pub fn remove(report: &SweepReport) -> Result<usize> {
    let mut removed = 0usize;
    for entry in &report.entries {
        // The marker proves we wrote this, so a recursive delete is warranted;
        // still check the type first so a replaced symlink is unlinked, not
        // followed.
        let metadata = match fs::symlink_metadata(&entry.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(SkillenvError::ReadFile {
                    path: entry.path.clone(),
                    source,
                });
            }
        };
        let result = if metadata.is_dir() {
            fs::remove_dir_all(&entry.path)
        } else {
            fs::remove_file(&entry.path)
        };
        result.map_err(|source| SkillenvError::WriteFile {
            path: entry.path.clone(),
            source,
        })?;
        removed += 1;
    }
    Ok(removed)
}

fn read_marker(dir: &Path) -> Result<Option<V0Marker>> {
    let path = dir.join(V0_MARKER_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(SkillenvError::ReadFile { path, source }),
    };
    // A marker we cannot parse is treated as absent rather than fatal: the
    // directory is then reported for a human to look at instead of removed.
    Ok(serde_json::from_str(&raw).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Create a v0-shaped deployment.
    fn v0_entry(target: &Path, dir_name: &str, repo: &str, scope: &str, skill: &str) {
        let dir = target.join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: x\n---\n\nbody\n").unwrap();
        fs::write(
            dir.join(V0_MARKER_FILE),
            serde_json::to_string_pretty(&json!({
                "repo": repo,
                "scope": scope,
                "skill": skill,
                "generated_name": dir_name,
                // Deliberately a path that no longer exists after migration.
                "source": format!("/work/{repo}/skillenv/remote/{skill}/default/{skill}"),
                "strategy": "render",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn finds_both_repo_local_and_global_name_shapes() -> Result<()> {
        let target = TempDir::new().unwrap();
        v0_entry(
            target.path(),
            "skillenv-dotfiles-default-kinko",
            "dotfiles",
            "default",
            "kinko",
        );
        v0_entry(
            target.path(),
            "skillenv-dotfiles-gd3c434ebf0ec-default-draft-pr",
            "dotfiles",
            "default",
            "draft-pr",
        );

        let report = sweep(target.path(), "dotfiles")?;
        // Ordered by directory name, so the result is stable across filesystems.
        // The repo-local shape sorts before the global one because `d` < `g`.
        let dirs: Vec<_> = report.entries.iter().map(|e| e.dir_name.as_str()).collect();
        assert_eq!(
            dirs,
            vec![
                "skillenv-dotfiles-default-kinko",
                "skillenv-dotfiles-gd3c434ebf0ec-default-draft-pr",
            ]
        );
        let mut skills: Vec<_> = report.entries.iter().map(|e| e.skill.as_str()).collect();
        skills.sort_unstable();
        assert_eq!(skills, vec!["draft-pr", "kinko"]);
        assert!(report.unmarked.is_empty());
        Ok(())
    }

    /// The point of the module: a marker whose `source` no longer resolves is
    /// still recognised. v0's own predicate required `source` to be live, which is
    /// exactly what fails after files move.
    #[test]
    fn a_stale_source_path_does_not_prevent_recognition() -> Result<()> {
        let target = TempDir::new().unwrap();
        v0_entry(
            target.path(),
            "skillenv-dotfiles-default-kinko",
            "dotfiles",
            "default",
            "kinko",
        );

        let report = sweep(target.path(), "dotfiles")?;
        assert_eq!(report.entries.len(), 1);
        assert!(
            report.entries[0].source.contains("skillenv/remote"),
            "the stale path is kept for reporting: {:?}",
            report.entries[0].source
        );
        Ok(())
    }

    /// `$HOME` is shared, so another repository's deployment must be left alone.
    #[test]
    fn another_repositorys_deployment_is_ignored() -> Result<()> {
        let target = TempDir::new().unwrap();
        v0_entry(
            target.path(),
            "skillenv-dotfiles-default-kinko",
            "dotfiles",
            "default",
            "kinko",
        );
        v0_entry(
            target.path(),
            "skillenv-dotfiles-gabc123456789-default-other",
            "elsewhere",
            "default",
            "other",
        );

        let report = sweep(target.path(), "dotfiles")?;
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].skill, "kinko");
        Ok(())
    }

    /// v0 could leave a directory with assets but no marker when a render failed.
    /// Without a marker there is no evidence we created it, so it is reported and
    /// never removed.
    #[test]
    fn a_prefixed_directory_without_a_marker_is_reported_not_removed() -> Result<()> {
        let target = TempDir::new().unwrap();
        let orphan = target.path().join("skillenv-dotfiles-default-residue");
        fs::create_dir_all(orphan.join("assets")).unwrap();
        fs::write(orphan.join("assets/t.md"), "left behind\n").unwrap();

        let report = sweep(target.path(), "dotfiles")?;
        assert!(report.entries.is_empty());
        assert_eq!(report.unmarked, vec![orphan.clone()]);

        assert_eq!(remove(&report)?, 0);
        assert!(orphan.is_dir(), "an unmarked directory must survive");
        Ok(())
    }

    /// A marker naming a different directory means one of the two was renamed, so
    /// we can no longer say what the directory is and must not delete it.
    #[test]
    fn a_marker_disagreeing_with_its_directory_name_is_reported_not_removed() -> Result<()> {
        let target = TempDir::new().unwrap();
        v0_entry(
            target.path(),
            "skillenv-dotfiles-default-kinko",
            "dotfiles",
            "default",
            "kinko",
        );
        let dir = target.path().join("skillenv-dotfiles-default-kinko");
        let renamed = target.path().join("skillenv-dotfiles-default-renamed");
        fs::rename(&dir, &renamed).unwrap();

        let report = sweep(target.path(), "dotfiles")?;
        assert!(report.entries.is_empty());
        assert_eq!(report.unmarked, vec![renamed.clone()]);
        assert_eq!(remove(&report)?, 0);
        assert!(renamed.is_dir());
        Ok(())
    }

    #[test]
    fn an_unparseable_marker_is_treated_as_unmarked() -> Result<()> {
        let target = TempDir::new().unwrap();
        let dir = target.path().join("skillenv-dotfiles-default-broken");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(V0_MARKER_FILE), "{ not json").unwrap();

        let report = sweep(target.path(), "dotfiles")?;
        assert!(report.entries.is_empty());
        assert_eq!(report.unmarked, vec![dir]);
        Ok(())
    }

    #[test]
    fn unrelated_directories_are_untouched() -> Result<()> {
        let target = TempDir::new().unwrap();
        fs::create_dir_all(target.path().join("some-other-skill")).unwrap();
        fs::write(target.path().join("README.md"), "hi\n").unwrap();

        let report = sweep(target.path(), "dotfiles")?;
        assert!(report.is_empty());
        Ok(())
    }

    #[test]
    fn a_missing_target_directory_is_not_an_error() -> Result<()> {
        let report = sweep(Path::new("/nonexistent/skills"), "dotfiles")?;
        assert!(report.is_empty());
        Ok(())
    }

    #[test]
    fn removing_clears_only_the_marked_entries() -> Result<()> {
        let target = TempDir::new().unwrap();
        v0_entry(
            target.path(),
            "skillenv-dotfiles-default-kinko",
            "dotfiles",
            "default",
            "kinko",
        );
        fs::create_dir_all(target.path().join("keep-me")).unwrap();

        let report = sweep(target.path(), "dotfiles")?;
        assert_eq!(remove(&report)?, 1);
        assert!(
            !target
                .path()
                .join("skillenv-dotfiles-default-kinko")
                .exists()
        );
        assert!(target.path().join("keep-me").is_dir());
        Ok(())
    }

    #[test]
    fn removing_twice_is_not_an_error() -> Result<()> {
        let target = TempDir::new().unwrap();
        v0_entry(
            target.path(),
            "skillenv-dotfiles-default-kinko",
            "dotfiles",
            "default",
            "kinko",
        );
        let report = sweep(target.path(), "dotfiles")?;
        assert_eq!(remove(&report)?, 1);
        assert_eq!(remove(&report)?, 0, "a second pass should be a no-op");
        Ok(())
    }

    /// A profile scope is the only one carrying information worth turning into a
    /// label; `default` and `local` do not.
    #[test]
    fn only_a_profile_scope_becomes_a_label() {
        let entry = |scope: &str| LegacyEntry {
            path: PathBuf::from("/x"),
            dir_name: "d".to_string(),
            repo: "dotfiles".to_string(),
            scope: scope.to_string(),
            skill: "s".to_string(),
            source: "/old".to_string(),
            strategy: "render".to_string(),
        };
        assert_eq!(
            entry("profile:review").scope_label().as_deref(),
            Some("review")
        );
        assert_eq!(entry("default").scope_label(), None);
        assert_eq!(entry("local").scope_label(), None);
    }

    #[test]
    fn a_symlinked_deployment_is_identified() {
        let mut entry = LegacyEntry {
            path: PathBuf::from("/x"),
            dir_name: "d".to_string(),
            repo: "dotfiles".to_string(),
            scope: "default".to_string(),
            skill: "s".to_string(),
            source: "/old".to_string(),
            strategy: "render".to_string(),
        };
        assert!(!entry.was_symlink());
        entry.strategy = "symlink".to_string();
        assert!(entry.was_symlink());
    }
}
