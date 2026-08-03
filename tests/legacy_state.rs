//! The sweep must account for a real v0 deployment, not a synthetic one.
//!
//! `fixtures/v0_state/markers.json` is a frozen capture of an actual v0 setup: 4
//! target directories holding 16 skills each, every one with a marker, split
//! evenly between the repo-local and `$HOME` name shapes.
//!
//! What this pins down is the failure the migration would otherwise cause. v0's
//! own cleanup required a marker's `source` to point inside a currently-known
//! source root; after migration every one of these points at
//! `…/skillenv/remote/…`, which no longer exists. If the sweep missed them,
//! `link` would report "linked 16, removed 0" and each directory would end up
//! with 32 skills, half of them referring to a deleted tree.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Rebuild the captured state under `root`, returning the target directories.
fn rebuild(root: &Path) -> Vec<PathBuf> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/v0_state/markers.json");
    let raw = fs::read_to_string(&fixture)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", fixture.display()));
    let captured: Value = serde_json::from_str(&raw).unwrap();

    let mut targets = Vec::new();
    for (target_name, entries) in captured.as_object().unwrap() {
        // "$HOME/.claude/skills" -> "<root>/HOME/.claude/skills"
        let relative = target_name.trim_start_matches('$').replace("/Users/", "");
        let target = root.join(relative.trim_start_matches('/'));
        fs::create_dir_all(&target).unwrap();

        for entry in entries.as_array().unwrap() {
            let dir_name = entry["dir_name"].as_str().unwrap();
            let dir = target.join(dir_name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), "---\nname: x\n---\n\nbody\n").unwrap();
            fs::write(
                dir.join(".skillenv-generated.json"),
                serde_json::to_string_pretty(&entry["marker"]).unwrap(),
            )
            .unwrap();
        }
        targets.push(target);
    }
    targets
}

#[test]
fn the_sweep_accounts_for_every_directory_a_real_v0_setup_left_behind() {
    let root = tempfile::tempdir().unwrap();
    let targets = rebuild(root.path());
    assert_eq!(targets.len(), 4, "expected four target directories");

    let mut total_found = 0usize;
    for target in &targets {
        let report = skillenv::sweep_legacy(target, "dotfiles")
            .unwrap_or_else(|error| panic!("sweep failed for {}: {error}", target.display()));

        assert_eq!(
            report.entries.len(),
            16,
            "expected 16 v0 skills in {}, found {}",
            target.display(),
            report.entries.len()
        );
        assert!(
            report.unmarked.is_empty(),
            "every captured directory has a marker, so none should be unmarked: {:?}",
            report.unmarked
        );

        // Every marker's source points into the v0 layout, which migration
        // removes — the case that defeated v0's own removal predicate.
        for entry in &report.entries {
            assert!(
                entry.source.contains("/skillenv/"),
                "expected a v0-era source path, got {:?}",
                entry.source
            );
        }
        total_found += report.entries.len();
    }

    assert_eq!(total_found, 64, "the whole captured deployment");
}

#[test]
fn removing_leaves_the_target_directories_empty() {
    let root = tempfile::tempdir().unwrap();
    for target in rebuild(root.path()) {
        let report = skillenv::sweep_legacy(&target, "dotfiles").unwrap();
        assert_eq!(skillenv::remove_legacy(&report).unwrap(), 16);

        let left: Vec<_> = fs::read_dir(&target)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert!(left.is_empty(), "{} still holds {left:?}", target.display());
    }
}

/// A sweep for a different repository must not touch this one's deployment,
/// because `$HOME` targets are shared machine-wide.
#[test]
fn a_sweep_for_another_repository_finds_nothing_here() {
    let root = tempfile::tempdir().unwrap();
    for target in rebuild(root.path()) {
        let report = skillenv::sweep_legacy(&target, "some-other-repo").unwrap();
        assert!(
            report.entries.is_empty(),
            "{} should be untouched, found {:?}",
            target.display(),
            report.entries.len()
        );
    }
}
