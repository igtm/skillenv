//! `skillenv.lock` — what the manifest's intent actually resolved to.
//!
//! Three things v0's lock could not express, each of which caused a real
//! failure:
//!
//! * **Resolution separate from intent.** v0 expanded "every skill from this
//!   source" into a concrete `selected_skills` list at `add` time and then read
//!   that list back as if the user had typed it, so new upstream skills were
//!   never picked up and a renamed one made `update` fail outright.
//! * **Content identity.** v0 recorded only a git revision, so a source whose
//!   bytes changed without the pinned revision moving was undetectable — and
//!   there was no way to ask "is this stale?" without mutating the checkout.
//! * **Per-skill state.** v0 keyed everything by source, leaving nowhere to
//!   record that one skill is quarantined while its siblings are fine.
//!
//! Writes go through a temporary file and a rename. v0 wrote the lock with a
//! plain `fs::write` once, after installing every source, so a failure partway
//! through left the installed trees and the recorded revisions disagreeing with
//! no way back.
//!
//! Nothing reads a lock yet — `source` and `deploy` become the first consumers,
//! and this allow goes away with them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::manifest::SkillId;
use crate::{Result, SkillenvError};

pub(crate) const LOCK_FILE: &str = "skillenv.lock";
const LOCK_VERSION: u32 = 1;

/// Files that must not influence a skill's content digest.
///
/// `.git` because a fetched tree may carry one; the marker because we write it
/// ourselves; `.DS_Store` because macOS creates it in any directory a user
/// looks at, and letting it change the digest would make skills spuriously
/// dirty.
const DIGEST_EXCLUDED: &[&str] = &[".git", ".skillenv-generated.json", ".DS_Store"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockFile {
    pub version: u32,
    #[serde(default)]
    pub skills: Vec<LockedSkill>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            version: LOCK_VERSION,
            skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedSkill {
    pub id: SkillId,
    /// The manifest source this came from, as written by the user.
    pub source: String,
    /// Which `[[source]]` entry contributed it, if any. Absent for a
    /// directly-declared `[[skill]]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    /// The ref that was resolved, e.g. `main`. Absent for local sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ref: Option<String>,
    /// The revision the ref pointed at, or `None` for an unversioned local path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_revision: Option<String>,
    /// Digest of the skill tree as fetched. Lets `outdated` and `diff` notice a
    /// content change even when the revision has not moved.
    pub content_digest: String,
    #[serde(default)]
    pub safeguard: SafeguardState,
}

/// What the safeguard concluded about this skill, and for which bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeguardState {
    /// The digest the findings were produced from. When it differs from
    /// `content_digest` the findings are stale and the scan must be redone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<LockedFinding>,
    /// Set when a previously-deployed skill was refused an update. The old copy
    /// stays where it is rather than being deleted, so a compromised upstream
    /// cannot remove a skill by tripping the scanner on purpose.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quarantined: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedFinding {
    pub code: String,
    pub severity: String,
    pub message: String,
}

impl LockFile {
    pub fn path(root: &Path) -> PathBuf {
        root.join(LOCK_FILE)
    }

    /// Read the lock, treating a missing file as an empty one.
    ///
    /// A future version is refused rather than silently downgraded: v0 accepted
    /// any `version` above 1 and would have rewritten it, discarding fields it
    /// did not know about.
    pub fn load(root: &Path) -> Result<Self> {
        let path = Self::path(root);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => return Err(SkillenvError::ReadFile { path, source }),
        };

        let lock: Self = serde_json::from_str(&raw).map_err(|source| SkillenvError::ParseLock {
            path: path.clone(),
            source,
        })?;
        if lock.version > LOCK_VERSION {
            return Err(SkillenvError::UnsupportedLockVersion {
                path,
                found: lock.version,
                supported: LOCK_VERSION,
            });
        }
        Ok(lock)
    }

    /// Write the lock atomically.
    ///
    /// Entries are sorted by id so the file is stable across runs and a diff
    /// shows only real changes.
    pub fn save(&self, root: &Path) -> Result<()> {
        let mut ordered = self.clone();
        ordered.version = LOCK_VERSION;
        ordered.skills.sort_by(|a, b| a.id.cmp(&b.id));

        let mut body = serde_json::to_string_pretty(&ordered).map_err(|source| {
            SkillenvError::SerializeLock {
                path: Self::path(root),
                source,
            }
        })?;
        body.push('\n');

        write_atomically(&Self::path(root), body.as_bytes())
    }

    pub fn get(&self, id: &SkillId) -> Option<&LockedSkill> {
        self.skills.iter().find(|skill| &skill.id == id)
    }

    /// Insert or replace one skill's entry.
    ///
    /// Callers save after each source rather than once at the end, so an
    /// interrupted run leaves the lock describing exactly what is on disk.
    pub fn upsert(&mut self, entry: LockedSkill) {
        match self.skills.iter_mut().find(|skill| skill.id == entry.id) {
            Some(existing) => *existing = entry,
            None => self.skills.push(entry),
        }
    }

    pub fn remove(&mut self, id: &SkillId) -> Option<LockedSkill> {
        let index = self.skills.iter().position(|skill| &skill.id == id)?;
        Some(self.skills.remove(index))
    }
}

impl LockedSkill {
    /// Whether the recorded safeguard findings describe the current bytes.
    pub fn safeguard_is_current(&self) -> bool {
        self.safeguard.scanned_digest.as_deref() == Some(self.content_digest.as_str())
    }
}

/// Replace `path`'s contents without ever leaving a partially-written file.
///
/// The temporary lives in the same directory so the rename is a same-filesystem
/// operation.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| SkillenvError::WriteFile {
        path: path.to_path_buf(),
        source: std::io::Error::other("path has no parent directory"),
    })?;

    let temp = tempfile::Builder::new()
        .prefix(".skillenv-lock-")
        .tempfile_in(parent)
        .map_err(|source| SkillenvError::WriteFile {
            path: parent.to_path_buf(),
            source,
        })?;

    fs::write(temp.path(), bytes).map_err(|source| SkillenvError::WriteFile {
        path: temp.path().to_path_buf(),
        source,
    })?;

    temp.persist(path)
        .map_err(|error| SkillenvError::WriteFile {
            path: path.to_path_buf(),
            source: error.error,
        })?;
    Ok(())
}

/// Digest of a skill directory: `sha256` over a sorted list of
/// `(relative path, executable bit, sha256(contents))`.
///
/// Sorted so filesystem enumeration order cannot change the result; the
/// executable bit is included because we report executables as a finding, so a
/// file gaining `+x` has to count as a content change; mtimes are excluded so
/// two checkouts of the same revision agree.
pub fn digest_tree(dir: &Path) -> Result<String> {
    let mut files: BTreeMap<String, (bool, String)> = BTreeMap::new();

    for entry in WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.map_err(|error| SkillenvError::ReadFile {
            path: dir.to_path_buf(),
            source: std::io::Error::other(error),
        })?;
        let relative = entry
            .path()
            .strip_prefix(dir)
            .map_err(|error| SkillenvError::ReadFile {
                path: dir.to_path_buf(),
                source: std::io::Error::other(error),
            })?;
        if relative.as_os_str().is_empty() || is_digest_excluded(relative) {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        let bytes = fs::read(entry.path()).map_err(|source| SkillenvError::ReadFile {
            path: entry.path().to_path_buf(),
            source,
        })?;
        // Forward slashes so a digest computed on Windows matches one from unix.
        let key = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.insert(key, (is_executable(entry.path())?, hex_digest(&bytes)));
    }

    let mut hasher = Sha256::new();
    for (path, (executable, digest)) in &files {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(if *executable { b"x" } else { b"-" });
        hasher.update([0]);
        hasher.update(digest.as_bytes());
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn is_digest_excluded(relative: &Path) -> bool {
    relative.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| DIGEST_EXCLUDED.contains(&name))
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|source| SkillenvError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn id(raw: &str) -> SkillId {
        SkillId::parse(raw).expect("test id should be valid")
    }

    fn entry(name: &str, digest: &str) -> LockedSkill {
        LockedSkill {
            id: id(name),
            source: "github:igtm/kinko".to_string(),
            source_name: None,
            resolved_ref: Some("main".to_string()),
            resolved_revision: Some("71947fdd".to_string()),
            content_digest: digest.to_string(),
            safeguard: SafeguardState::default(),
        }
    }

    fn write(dir: &Path, relative: &str, body: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn a_missing_lock_reads_as_empty() -> Result<()> {
        let root = TempDir::new().unwrap();
        let lock = LockFile::load(root.path())?;
        assert_eq!(lock.version, LOCK_VERSION);
        assert!(lock.skills.is_empty());
        Ok(())
    }

    #[test]
    fn round_trips_and_sorts_by_id() -> Result<()> {
        let root = TempDir::new().unwrap();
        let mut lock = LockFile::default();
        lock.upsert(entry("zeta", "sha256:z"));
        lock.upsert(entry("alpha", "sha256:a"));
        lock.save(root.path())?;

        let reloaded = LockFile::load(root.path())?;
        let ids: Vec<_> = reloaded
            .skills
            .iter()
            .map(|skill| skill.id.to_string())
            .collect();
        assert_eq!(ids, vec!["alpha", "zeta"]);
        Ok(())
    }

    #[test]
    fn upsert_replaces_rather_than_duplicating() {
        let mut lock = LockFile::default();
        lock.upsert(entry("kinko", "sha256:one"));
        lock.upsert(entry("kinko", "sha256:two"));
        assert_eq!(lock.skills.len(), 1);
        assert_eq!(lock.skills[0].content_digest, "sha256:two");
    }

    #[test]
    fn remove_returns_the_entry_it_took() {
        let mut lock = LockFile::default();
        lock.upsert(entry("kinko", "sha256:one"));
        assert!(lock.remove(&id("kinko")).is_some());
        assert!(lock.remove(&id("kinko")).is_none());
        assert!(lock.skills.is_empty());
    }

    /// Refusing a newer version protects fields this build does not know about;
    /// v0 accepted anything and would have rewritten the file without them.
    #[test]
    fn refuses_a_lock_from_a_newer_version() {
        let root = TempDir::new().unwrap();
        write(root.path(), LOCK_FILE, r#"{"version":2,"skills":[]}"#);
        let error = LockFile::load(root.path()).unwrap_err().to_string();
        assert!(error.contains("version 2"), "unexpected error: {error}");
    }

    #[test]
    fn a_saved_lock_omits_empty_optional_state() -> Result<()> {
        let root = TempDir::new().unwrap();
        let mut lock = LockFile::default();
        lock.upsert(entry("kinko", "sha256:one"));
        lock.save(root.path())?;

        let raw = fs::read_to_string(LockFile::path(root.path())).unwrap();
        assert!(!raw.contains("source_name"), "got: {raw}");
        assert!(!raw.contains("quarantined"), "got: {raw}");
        assert!(!raw.contains("findings"), "got: {raw}");
        assert!(raw.ends_with('\n'), "lock should end with a newline");
        Ok(())
    }

    /// The temporary must not survive, or the next `link` would see a stray file
    /// next to the lock.
    #[test]
    fn saving_leaves_no_temporary_behind() -> Result<()> {
        let root = TempDir::new().unwrap();
        LockFile::default().save(root.path())?;
        let strays: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name != LOCK_FILE)
            .collect();
        assert!(strays.is_empty(), "unexpected leftovers: {strays:?}");
        Ok(())
    }

    #[test]
    fn safeguard_findings_go_stale_when_content_changes() {
        let mut skill = entry("kinko", "sha256:new");
        skill.safeguard.scanned_digest = Some("sha256:old".to_string());
        assert!(!skill.safeguard_is_current());
        skill.safeguard.scanned_digest = Some("sha256:new".to_string());
        assert!(skill.safeguard_is_current());
    }

    #[test]
    fn digest_is_stable_and_content_sensitive() -> Result<()> {
        let one = TempDir::new().unwrap();
        write(one.path(), "SKILL.md", "body\n");
        write(one.path(), "assets/t.md", "asset\n");

        let two = TempDir::new().unwrap();
        write(two.path(), "assets/t.md", "asset\n");
        write(two.path(), "SKILL.md", "body\n");

        // Same content in a different creation order must agree.
        assert_eq!(digest_tree(one.path())?, digest_tree(two.path())?);

        write(two.path(), "SKILL.md", "different\n");
        assert_ne!(digest_tree(one.path())?, digest_tree(two.path())?);
        Ok(())
    }

    /// A `.git` directory, our own marker, and `.DS_Store` must not move the
    /// digest, or a skill would look dirty for reasons unrelated to its content.
    #[test]
    fn digest_ignores_bookkeeping_files() -> Result<()> {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "SKILL.md", "body\n");
        let before = digest_tree(dir.path())?;

        write(dir.path(), ".git/config", "[core]\n");
        write(dir.path(), ".skillenv-generated.json", "{}");
        write(dir.path(), ".DS_Store", "junk");
        write(dir.path(), "assets/.DS_Store", "junk");
        assert_eq!(before, digest_tree(dir.path())?);
        Ok(())
    }

    /// A file gaining the executable bit is a content change, because the
    /// safeguard reports executables and must not be able to miss one.
    #[cfg(unix)]
    #[test]
    fn digest_tracks_the_executable_bit() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        write(dir.path(), "run.sh", "echo hi\n");
        let before = digest_tree(dir.path())?;

        let path = dir.path().join("run.sh");
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();

        assert_ne!(before, digest_tree(dir.path())?);
        Ok(())
    }

    /// Renaming a file changes the tree even when the bytes are identical.
    #[test]
    fn digest_covers_paths_not_just_contents() -> Result<()> {
        let one = TempDir::new().unwrap();
        write(one.path(), "a.md", "same\n");
        let two = TempDir::new().unwrap();
        write(two.path(), "b.md", "same\n");
        assert_ne!(digest_tree(one.path())?, digest_tree(two.path())?);
        Ok(())
    }
}
