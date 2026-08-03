//! Path normalization, slugification, and the small filesystem primitives the
//! rest of the crate builds on.
//!
//! `normalize_path` is deliberately lexical: it never touches the filesystem, so
//! it is safe to call on paths that do not exist and cannot follow a symlink out
//! of a tree being validated.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{Result, SkillenvError};

pub(crate) fn repo_slug(repo_root: &Path) -> String {
    slugify_or(
        repo_root
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("repo"),
        "repo",
    )
}

pub(crate) fn stable_global_repo_root(repo_root: &Path) -> PathBuf {
    fs::canonicalize(repo_root)
        .map(|path| normalize_path(&path))
        .unwrap_or_else(|_| normalize_path(repo_root))
}

pub(crate) fn short_path_digest(path: &Path) -> String {
    let normalized = normalize_path(path);
    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalized.display().to_string().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let digest = format!("{hash:016x}");
    digest[..12].to_string()
}

pub(crate) fn slugify_or(input: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        let is_allowed = lower.is_ascii_lowercase() || lower.is_ascii_digit();
        if is_allowed {
            slug.push(lower);
            last_dash = false;
            continue;
        }

        if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }

    while slug.starts_with('-') {
        slug.remove(0);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

pub(crate) fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| SkillenvError::CreateDir {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn ensure_layout_dir(path: &Path, created_dirs: &mut Vec<PathBuf>) -> Result<()> {
    let existed = path.is_dir();
    ensure_dir(path)?;
    if !existed {
        created_dirs.push(path.to_path_buf());
    }
    Ok(())
}

pub(crate) fn ensure_unmanaged_target_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(SkillenvError::TargetCollision {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SkillenvError::ReadFile {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(crate) fn symlink_targets_known_root(
    path: &Path,
    known_source_roots: &[PathBuf],
) -> Result<bool> {
    let target = fs::read_link(path).map_err(|source| SkillenvError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let resolved = if target.is_absolute() {
        normalize_path(&target)
    } else {
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        normalize_path(&base.join(target))
    };
    Ok(known_source_roots
        .iter()
        .map(|root| normalize_path(root))
        .any(|root| resolved.starts_with(&root)))
}

pub(crate) fn marker_source_matches_known_root(
    source: &str,
    known_source_roots: &[PathBuf],
) -> bool {
    let source_path = normalize_path(Path::new(source));
    known_source_roots
        .iter()
        .map(|root| normalize_path(root))
        .any(|root| source_path.starts_with(&root))
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[cfg(unix)]
pub(crate) fn create_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
pub(crate) fn create_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(source, destination)
}
