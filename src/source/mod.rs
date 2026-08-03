//! Acquiring a skill's bytes, and refusing the ones we should not accept.
//!
//! v0 copied a fetched tree verbatim: no exclusions, no size limits, no symlink
//! handling, no traversal check on the requested subdirectory. Whatever was in
//! someone else's repository landed in the deploy path unexamined.
//!
//! The cache is keyed by resolved revision, so a fetch is idempotent and a
//! revision already on disk is simply reused. That is what lets `fetch` restore a
//! machine from the lock file alone, and what makes `diff` cheap.

mod git;

use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::lock::digest_tree;
use crate::manifest::SourceSpec;
use crate::paths::{ensure_dir, normalize_path};
use crate::{Result, SkillenvError};

/// Where fetched content lives, relative to the manifest root.
pub(crate) const CACHE_DIR: &str = ".skillenv/cache";

/// Names never copied out of a fetched tree.
///
/// `.git` because a shallow checkout carries one and it is not part of the skill;
/// `.DS_Store` because macOS creates it wherever a user looks and it would
/// otherwise perturb the content digest.
const NEVER_COPIED: &[&str] = &[".git", ".DS_Store"];

/// Caps on what a single skill may contain.
///
/// These exist so a hostile or accidentally-huge source cannot fill the disk or
/// stall a shell hook. The numbers are far above any real skill: the largest one
/// installed here is a few tens of kilobytes.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 500;

/// A skill's bytes, ready to be rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSkill {
    /// Directory holding `SKILL.md` and any assets.
    pub dir: PathBuf,
    /// Resolved commit for a git source; `None` for an unversioned local path.
    pub revision: Option<String>,
    pub content_digest: String,
    /// Things accepted but worth reporting, e.g. an executable asset.
    pub notes: Vec<String>,
}

/// Show what changed between two texts, as unified diff text.
///
/// Uses `git diff --no-index`, which works outside a repository, rather than
/// carrying a diff implementation. Goes through the same hardened runner as every
/// other git call, so it cannot prompt or read system config either. `git diff`
/// exits 1 when the contents differ, so it uses the runner that keeps output
/// regardless of exit status — the erroring path would have made every diff empty.
///
/// The two texts are written under short directory names and compared by relative
/// path, because git echoes whatever path it was given: passing the real locations
/// put an absolute path in the output three times, which buried the change itself.
pub fn diff_text(
    before: &str,
    after: &str,
    before_label: &str,
    after_label: &str,
) -> Result<String> {
    let scratch = tempfile::TempDir::new().map_err(|source| SkillenvError::WriteFile {
        path: PathBuf::from("temporary directory"),
        source,
    })?;
    let mut relative = Vec::new();
    for (label, text) in [(before_label, before), (after_label, after)] {
        let dir = scratch.path().join(label);
        ensure_dir(&dir)?;
        let path = dir.join("SKILL.md");
        fs::write(&path, text).map_err(|source| SkillenvError::WriteFile {
            path: path.clone(),
            source,
        })?;
        relative.push(format!("{label}/SKILL.md"));
    }

    Ok(git::run_reporting_status(
        &[
            "diff",
            "--no-index",
            "--no-color",
            "--",
            &relative[0],
            &relative[1],
        ],
        Some(scratch.path()),
    )
    .unwrap_or_default())
}

/// Resolve the current revision of a source without fetching or writing.
///
/// Used by `outdated`, which must be able to answer "is this stale?" without
/// touching the cache or the lock.
pub fn peek_revision(spec: &SourceSpec, git_ref: Option<&str>) -> Result<Option<String>> {
    match git::transport_for(spec) {
        Some(transport) => git::remote_revision(&transport, git_ref).map(Some),
        None => Ok(None),
    }
}

/// The directory every fetched source is cached under.
pub fn cache_root(manifest_root: &Path) -> PathBuf {
    manifest_root.join(CACHE_DIR)
}

/// Directory a revision of a source caches into.
///
/// Keyed by revision so two revisions coexist and a re-fetch of one already
/// present is a no-op.
pub fn cache_dir(manifest_root: &Path, source_name: &str, revision: &str) -> PathBuf {
    manifest_root
        .join(CACHE_DIR)
        .join(sanitize_component(source_name))
        .join(sanitize_component(revision))
}

/// Where a fetch put a source, and which revision it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSource {
    /// The tree root, after any subdirectory has been applied.
    pub root: PathBuf,
    pub revision: String,
    /// True when the revision was already cached and nothing was downloaded.
    pub reused: bool,
}

/// Fetch a git-backed source into the cache.
///
/// `pin` selects the revision: `Some` restores exactly that one, which is what
/// `fetch` does from a lock file, and `None` resolves whatever the ref points at
/// now, which is what `update` does. `subdir` is validated before any git command
/// runs.
pub fn fetch_git(
    manifest_root: &Path,
    source_name: &str,
    spec: &SourceSpec,
    git_ref: Option<&str>,
    subdir: Option<&Path>,
    pin: Option<&str>,
    // Refuse anything newer than this git-readable timestamp, taking the newest
    // revision at or before it. `None` takes whatever the ref points at.
    not_after: Option<&str>,
) -> Result<FetchedSource> {
    if let Some(subdir) = subdir {
        git::validate_subdir(subdir)?;
    }
    let transport = git::transport_for(spec).ok_or_else(|| SkillenvError::InvalidSource {
        input: format!("{spec:?}"),
        message: "not a git-backed source".to_string(),
    })?;

    // With an age limit and no pin, which revision we end up on is only knowable
    // after fetching history — so the cache cannot be keyed on it in advance. Resolve
    // beside the cache and move the result in once the answer is known.
    if let (None, Some(cutoff)) = (pin, not_after) {
        return fetch_aged(
            manifest_root,
            source_name,
            &transport,
            git_ref,
            subdir,
            cutoff,
        );
    }

    // The revision is settled before anything is written, so the cache key is
    // known up front and an already-present revision costs nothing.
    let revision = match pin {
        Some(pin) => pin.to_string(),
        None => git::remote_revision(&transport, git_ref)?,
    };
    let destination = cache_dir(manifest_root, source_name, &revision);
    if destination.join(".git").is_dir() {
        return Ok(FetchedSource {
            root: git::resolve_subdir(&destination, subdir)?,
            revision,
            reused: true,
        });
    }

    ensure_dir(&destination)?;
    // A pinned revision is fetched by sha, so the age limit does not apply: the lock
    // already names the revision, and re-checking its age would refuse to restore a
    // machine from a lock that was fine when it was written.
    let fetched = git::fetch_into(&transport, Some(&revision), &destination, None)?;
    Ok(FetchedSource {
        root: git::resolve_subdir(&destination, subdir)?,
        revision: fetched,
        reused: false,
    })
}

/// Fetch the newest revision at least as old as `cutoff`.
///
/// Resolved in a scratch directory inside the cache root — same filesystem, so the
/// move into place is a rename rather than a copy — because the revision that names
/// the cache entry is the answer, not the question.
fn fetch_aged(
    manifest_root: &Path,
    source_name: &str,
    transport: &str,
    git_ref: Option<&str>,
    subdir: Option<&Path>,
    cutoff: &str,
) -> Result<FetchedSource> {
    let root = cache_root(manifest_root);
    ensure_dir(&root)?;
    let scratch = tempfile::TempDir::new_in(&root).map_err(|source| SkillenvError::WriteFile {
        path: root.clone(),
        source,
    })?;
    let work = scratch.path().join("resolving");

    let revision = git::fetch_into(transport, git_ref, &work, Some(cutoff))?;
    let destination = cache_dir(manifest_root, source_name, &revision);
    if destination.join(".git").is_dir() {
        // Already have it. The scratch copy goes away with the guard.
        return Ok(FetchedSource {
            root: git::resolve_subdir(&destination, subdir)?,
            revision,
            reused: true,
        });
    }

    if let Some(parent) = destination.parent() {
        ensure_dir(parent)?;
    }
    fs::rename(&work, &destination).map_err(|source| SkillenvError::WriteFile {
        path: destination.clone(),
        source,
    })?;
    Ok(FetchedSource {
        root: git::resolve_subdir(&destination, subdir)?,
        revision,
        reused: false,
    })
}

/// Copy one skill directory out of a source tree, checking what it contains.
///
/// This is the boundary where someone else's repository becomes our deploy input,
/// so it is where the checks belong.
pub fn accept_skill(
    source_dir: &Path,
    destination: &Path,
    revision: Option<String>,
) -> Result<FetchedSkill> {
    if !source_dir.join("SKILL.md").is_file() {
        return Err(SkillenvError::MissingSkillFile {
            path: source_dir.join("SKILL.md"),
        });
    }

    let mut notes = Vec::new();
    let mut total_bytes = 0u64;
    let mut file_count = 0usize;

    ensure_dir(destination)?;
    // Not following symlinks: a link is inspected, never traversed, so a link to
    // a directory cannot pull an unrelated tree into the copy.
    for entry in WalkDir::new(source_dir)
        .follow_links(false)
        .sort_by_file_name()
    {
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
        if relative.as_os_str().is_empty() || is_excluded(relative) {
            continue;
        }

        let metadata =
            entry
                .path()
                .symlink_metadata()
                .map_err(|source| SkillenvError::ReadFile {
                    path: entry.path().to_path_buf(),
                    source,
                })?;

        // A symlink inside a skill has no legitimate use and is the obvious way
        // to smuggle a reference to /etc/passwd or out of the tree entirely.
        if metadata.file_type().is_symlink() {
            return Err(SkillenvError::UnsafeSourceEntry {
                path: entry.path().to_path_buf(),
                reason: "a symlink; skills must contain only regular files".to_string(),
            });
        }

        let target = destination.join(relative);
        // `relative` comes from `strip_prefix` on a walked path, so it should never
        // climb out. Checked anyway: this is the last point before a write, and the
        // cost of being wrong here is a file placed outside the cache.
        if !contains(destination, &target) {
            return Err(SkillenvError::UnsafeSourceEntry {
                path: entry.path().to_path_buf(),
                reason: format!("resolves outside {}", destination.display()),
            });
        }
        if metadata.is_dir() {
            ensure_dir(&target)?;
            continue;
        }

        file_count += 1;
        if file_count > MAX_FILE_COUNT {
            return Err(SkillenvError::SourceTooLarge {
                path: source_dir.to_path_buf(),
                limit: format!("{MAX_FILE_COUNT} files"),
            });
        }
        let size = metadata.len();
        if size > MAX_FILE_BYTES {
            return Err(SkillenvError::SourceTooLarge {
                path: entry.path().to_path_buf(),
                limit: format!("{MAX_FILE_BYTES} bytes per file"),
            });
        }
        total_bytes += size;
        if total_bytes > MAX_TREE_BYTES {
            return Err(SkillenvError::SourceTooLarge {
                path: source_dir.to_path_buf(),
                limit: format!("{MAX_TREE_BYTES} bytes in total"),
            });
        }

        if is_executable(&metadata) {
            notes.push(format!(
                "{} is executable; confirm that is intended",
                relative.display()
            ));
        }

        if let Some(parent) = target.parent() {
            ensure_dir(parent)?;
        }
        fs::copy(entry.path(), &target).map_err(|source| SkillenvError::WriteFile {
            path: target.clone(),
            source,
        })?;
    }

    Ok(FetchedSkill {
        content_digest: digest_tree(destination)?,
        dir: destination.to_path_buf(),
        revision,
        notes,
    })
}

/// Locate a skill inside a fetched source tree.
///
/// Accepts the layouts that occur in practice: the tree is itself one skill, or
/// it holds a `skills/` directory, or the skill sits directly at the top level.
/// Unlike v0 this never assumes a `default/`, `local/`, and `profiles/` trio —
/// v0's reader unconditionally read `default/` whenever any of the three existed,
/// so a source holding only `local/` failed with a read error.
pub fn locate_skill(root: &Path, id: &str) -> Option<PathBuf> {
    let candidates = [
        root.to_path_buf(),
        root.join(id),
        root.join("skills").join(id),
        root.join(".agents").join("skills").join(id),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.join("SKILL.md").is_file())
}

fn is_excluded(relative: &Path) -> bool {
    relative.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| NEVER_COPIED.contains(&name))
    })
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

/// Make a string safe to use as one path component.
///
/// Source names and revisions reach us from a manifest and from git, so neither
/// is trusted to be a well-behaved filename.
fn sanitize_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    // `.` and `..` would name the parent or self rather than a new directory.
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The committer date of `revision`, as seconds since the epoch.
///
/// Read from the cache, where the revision was checked out. `None` when it is not
/// cached, which is the same as not knowing.
pub fn cached_commit_time(manifest_root: &Path, source_name: &str, revision: &str) -> Option<u64> {
    let dir = cache_dir(manifest_root, source_name, revision);
    if !dir.join(".git").is_dir() {
        return None;
    }
    let dir = dir.to_string_lossy().to_string();
    git::run(&["-C", &dir, "log", "-1", "--format=%ct", revision], None)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Run a git command, for tests in sibling modules that need to build a fixture repo.
#[cfg(test)]
pub(crate) fn run_git_for_test(args: &[&str]) -> Result<String> {
    git::run(args, None)
}

/// Whether `path` stays inside `root` once both are normalized.
///
/// Used before writing, so a crafted relative path cannot place a file outside
/// the cache.
fn contains(root: &Path, path: &Path) -> bool {
    normalize_path(path).starts_with(normalize_path(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a repository whose commits sit at known ages, so an age limit has
    /// something unambiguous to choose between.
    fn dated_repo(ages_in_days: &[u64]) -> TempDir {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        git::run(
            &["init", "--quiet", "--initial-branch", "main", &path],
            None,
        )
        .unwrap();
        for days in ages_in_days {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - days * 86_400;
            fs::write(dir.path().join("SKILL.md"), format!("body at {days}\n")).unwrap();
            git::run(&["-C", &path, "add", "-A"], None).unwrap();
            // The date has to be set for both author and committer: `--before` reads
            // the committer date, and leaving it as "now" would make every commit new.
            let stamp = std::ffi::OsString::from(format!("{seconds} +0000"));
            // Held only for this one commit. `rev-list --before` reads the *committer*
            // date, and the environment is the only way to set it — so without the
            // guard, concurrent tests read each other's timestamp and a repository
            // built to be old came out new.
            let _env = crate::test_support::set_env_for_test(&[
                ("GIT_AUTHOR_DATE", Some(stamp.clone())),
                ("GIT_COMMITTER_DATE", Some(stamp)),
            ]);
            git::run(
                &[
                    "-C",
                    &path,
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "user.name=t",
                    "commit",
                    "--quiet",
                    "-m",
                    &format!("{days} days old"),
                ],
                None,
            )
            .unwrap();
        }
        dir
    }

    fn cutoff_days_ago(days: u64) -> String {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - days * 86_400;
        crate::session::format_epoch_utc_for_test(seconds)
    }

    /// The whole point of the age limit: the tip is skipped and the newest revision
    /// that is old enough is taken instead.
    #[test]
    fn an_age_limit_takes_the_newest_revision_old_enough() -> Result<()> {
        let upstream = dated_repo(&[30, 20, 10, 2, 0]);
        let cache = TempDir::new().unwrap();
        let transport = upstream.path().to_string_lossy().to_string();

        // Ten days old is the newest at least seven days old.
        let expected = git::run(
            &[
                "-C",
                &transport,
                "rev-list",
                "-1",
                "--before",
                &cutoff_days_ago(7),
                "main",
            ],
            None,
        )?
        .trim()
        .to_string();
        let tip = git::run(&["-C", &transport, "rev-parse", "main"], None)?
            .trim()
            .to_string();
        assert_ne!(expected, tip, "the fixture must have a too-new tip");

        let fetched = fetch_git(
            cache.path(),
            "up",
            &SourceSpec::Git(transport.clone()),
            Some("main"),
            None,
            None,
            Some(&cutoff_days_ago(7)),
        )?;
        assert_eq!(fetched.revision, expected);
        assert!(fetched.root.join("SKILL.md").is_file());
        Ok(())
    }

    /// Without the limit, the tip is what you get — so the limit is doing the work
    /// rather than some accident of the fixture.
    #[test]
    fn without_a_limit_the_tip_is_taken() -> Result<()> {
        let upstream = dated_repo(&[30, 0]);
        let cache = TempDir::new().unwrap();
        let transport = upstream.path().to_string_lossy().to_string();
        let tip = git::run(&["-C", &transport, "rev-parse", "main"], None)?
            .trim()
            .to_string();

        let fetched = fetch_git(
            cache.path(),
            "up",
            &SourceSpec::Git(transport),
            Some("main"),
            None,
            None,
            None,
        )?;
        assert_eq!(fetched.revision, tip);
        Ok(())
    }

    /// A repository whose whole history is newer than the cutoff has no answer, and
    /// says so rather than silently falling back to the tip — falling back would make
    /// the setting look effective while doing nothing.
    #[test]
    fn a_repo_with_no_old_enough_revision_is_an_error() {
        let upstream = dated_repo(&[1, 0]);
        let cache = TempDir::new().unwrap();
        let error = fetch_git(
            cache.path(),
            "up",
            &SourceSpec::Git(upstream.path().to_string_lossy().to_string()),
            Some("main"),
            None,
            None,
            Some(&cutoff_days_ago(30)),
        )
        .unwrap_err();
        assert!(
            matches!(error, SkillenvError::NoRevisionOldEnough { .. }),
            "got: {error}"
        );
    }

    /// A pinned revision is fetched by sha, so the limit must not apply: the lock
    /// already named it, and re-judging its age would refuse to restore a machine
    /// from a lock that was fine when it was written.
    #[test]
    fn a_pinned_revision_ignores_the_age_limit() -> Result<()> {
        let upstream = dated_repo(&[30, 0]);
        let cache = TempDir::new().unwrap();
        let transport = upstream.path().to_string_lossy().to_string();
        let tip = git::run(&["-C", &transport, "rev-parse", "main"], None)?
            .trim()
            .to_string();

        let fetched = fetch_git(
            cache.path(),
            "up",
            &SourceSpec::Git(transport),
            Some("main"),
            None,
            Some(&tip),
            Some(&cutoff_days_ago(7)),
        )?;
        assert_eq!(fetched.revision, tip, "the pin wins");
        Ok(())
    }

    fn write(dir: &Path, relative: &str, body: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn skill_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "SKILL.md", "---\nname: x\n---\n\nBody\n");
        write(dir.path(), "assets/template.md", "asset\n");
        dir
    }

    #[test]
    fn accepting_a_skill_copies_it_and_records_a_digest() -> Result<()> {
        let source = skill_dir();
        let target = TempDir::new().unwrap();
        let accepted = accept_skill(
            source.path(),
            &target.path().join("out"),
            Some("abc123".to_string()),
        )?;

        assert!(accepted.dir.join("SKILL.md").is_file());
        assert!(accepted.dir.join("assets/template.md").is_file());
        assert!(accepted.content_digest.starts_with("sha256:"));
        assert_eq!(accepted.revision.as_deref(), Some("abc123"));
        assert!(accepted.notes.is_empty());
        Ok(())
    }

    #[test]
    fn a_directory_without_a_skill_file_is_refused_by_name() {
        let dir = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let error = accept_skill(dir.path(), target.path(), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("SKILL.md"), "unexpected: {error}");
    }

    /// A `.git` directory is not part of the skill, and copying it would also
    /// perturb the content digest.
    #[test]
    fn git_and_ds_store_are_not_copied() -> Result<()> {
        let source = skill_dir();
        write(source.path(), ".git/config", "[core]\n");
        write(source.path(), ".DS_Store", "junk");
        write(source.path(), "assets/.DS_Store", "junk");

        let target = TempDir::new().unwrap();
        let accepted = accept_skill(source.path(), &target.path().join("out"), None)?;
        assert!(!accepted.dir.join(".git").exists());
        assert!(!accepted.dir.join(".DS_Store").exists());
        assert!(!accepted.dir.join("assets/.DS_Store").exists());
        Ok(())
    }

    /// The obvious way to smuggle a reference out of the tree.
    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_a_skill_is_refused() {
        let source = skill_dir();
        std::os::unix::fs::symlink("/etc/passwd", source.path().join("secrets")).unwrap();

        let target = TempDir::new().unwrap();
        let error = accept_skill(source.path(), &target.path().join("out"), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("symlink"), "unexpected: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_directory_is_refused_rather_than_followed() {
        let source = skill_dir();
        let elsewhere = TempDir::new().unwrap();
        write(elsewhere.path(), "leaked.md", "secret\n");
        std::os::unix::fs::symlink(elsewhere.path(), source.path().join("linked")).unwrap();

        let target = TempDir::new().unwrap();
        assert!(accept_skill(source.path(), &target.path().join("out"), None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_asset_is_accepted_but_reported() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let source = skill_dir();
        let script = source.path().join("run.sh");
        fs::write(&script, "echo hi\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let target = TempDir::new().unwrap();
        let accepted = accept_skill(source.path(), &target.path().join("out"), None)?;
        assert!(
            accepted.notes.iter().any(|note| note.contains("run.sh")),
            "expected a note about the executable: {:?}",
            accepted.notes
        );
        Ok(())
    }

    #[test]
    fn an_oversized_file_is_refused_and_the_error_names_the_limit() {
        let source = skill_dir();
        fs::write(
            source.path().join("big.bin"),
            vec![0u8; (MAX_FILE_BYTES + 1) as usize],
        )
        .unwrap();

        let target = TempDir::new().unwrap();
        let error = accept_skill(source.path(), &target.path().join("out"), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("per file"), "unexpected: {error}");
    }

    #[test]
    fn too_many_files_is_refused() {
        let source = skill_dir();
        for index in 0..=MAX_FILE_COUNT {
            write(source.path(), &format!("many/f{index}.md"), "x");
        }
        let target = TempDir::new().unwrap();
        let error = accept_skill(source.path(), &target.path().join("out"), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("files"), "unexpected: {error}");
    }

    #[test]
    fn the_cache_path_is_keyed_by_revision() {
        let root = Path::new("/work/dotfiles");
        assert_eq!(
            cache_dir(root, "igtm-skills", "abc123"),
            root.join(".skillenv/cache/igtm-skills/abc123")
        );
    }

    /// A source name or revision reaching us from a manifest or from git is not
    /// trusted to be a usable filename.
    #[test]
    fn cache_components_are_sanitized() {
        let root = Path::new("/work");
        // The separator becomes a hyphen and the leading dots are stripped, so
        // this can only ever name a child of the cache.
        assert_eq!(
            cache_dir(root, "../escape", "rev"),
            root.join(".skillenv/cache/-escape/rev")
        );
        assert_eq!(
            cache_dir(root, "..", "..").to_string_lossy(),
            "/work/.skillenv/cache/unnamed/unnamed"
        );
        assert_eq!(
            cache_dir(root, "a/b", "c:d"),
            root.join(".skillenv/cache/a-b/c-d")
        );
    }

    #[test]
    fn locating_a_skill_accepts_the_layouts_that_occur_in_practice() {
        // The tree is itself one skill, which is what a gist looks like.
        let one = TempDir::new().unwrap();
        write(one.path(), "SKILL.md", "body\n");
        assert_eq!(
            locate_skill(one.path(), "anything"),
            Some(one.path().to_path_buf())
        );

        // A repository holding several under skills/.
        let many = TempDir::new().unwrap();
        write(many.path(), "skills/kinko/SKILL.md", "body\n");
        assert_eq!(
            locate_skill(many.path(), "kinko"),
            Some(many.path().join("skills/kinko"))
        );

        // Or directly at the top level.
        let flat = TempDir::new().unwrap();
        write(flat.path(), "kinko/SKILL.md", "body\n");
        assert_eq!(
            locate_skill(flat.path(), "kinko"),
            Some(flat.path().join("kinko"))
        );

        let empty = TempDir::new().unwrap();
        assert_eq!(locate_skill(empty.path(), "kinko"), None);
    }

    #[test]
    fn containment_is_judged_lexically() {
        assert!(contains(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(!contains(Path::new("/a/b"), Path::new("/a/c")));
        assert!(!contains(Path::new("/a/b"), Path::new("/a/b/../c")));
    }

    #[test]
    fn peeking_a_local_source_has_no_revision() -> Result<()> {
        assert_eq!(peek_revision(&SourceSpec::Local, None)?, None);
        Ok(())
    }
}
