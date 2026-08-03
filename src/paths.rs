//! Path normalization, slugification, and the small filesystem primitives the
//! rest of the crate builds on.
//!
//! `normalize_path` is deliberately lexical: it never touches the filesystem, so
//! it is safe to call on paths that do not exist and cannot follow a symlink out
//! of a tree being validated.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::{Result, SkillenvError};

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
