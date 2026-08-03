//! Turning a source skill directory into the bytes a provider consumes.
//!
//! Frontmatter is parsed, adjusted, and re-emitted; the body is passed through
//! verbatim. Nothing here touches the filesystem layout of a target directory —
//! that is deployment's job.

use std::path::{Path, PathBuf};

use serde_yaml::Mapping;

use crate::{Result, SkillenvError};

/// Which lines sit inside a fenced code block.
///
/// Shared with `crate::safeguard` deliberately: if the summarizer and the scanner
/// disagreed about what counts as code, the same text could be prose to one and
/// code to the other, and an instruction could hide in the gap. The fence line
/// itself counts as code so a crafted info string is not read as prose.
pub(crate) fn fenced_lines(text: &str) -> Vec<bool> {
    let mut flags = Vec::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
            flags.push(true);
            continue;
        }
        flags.push(in_code_block);
    }
    flags
}

pub(crate) fn parse_frontmatter(path: &Path, raw: &str) -> Result<(Mapping, String)> {
    if !(raw.starts_with("---\n") || raw.starts_with("---\r\n")) {
        return Ok((Mapping::new(), raw.to_string()));
    }

    let start = if raw.starts_with("---\r\n") { 5 } else { 4 };
    let mut cursor = start;
    for segment in raw[start..].split_inclusive('\n') {
        let trimmed = segment.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            let yaml = &raw[start..cursor];
            let body = &raw[(cursor + segment.len())..];
            let mapping = if yaml.trim().is_empty() {
                Mapping::new()
            } else {
                serde_yaml::from_str::<Mapping>(yaml).map_err(|source| {
                    SkillenvError::ParseFrontmatter {
                        path: path.to_path_buf(),
                        source,
                    }
                })?
            };
            return Ok((mapping, body.to_string()));
        }
        cursor += segment.len();
    }

    Ok((Mapping::new(), raw.to_string()))
}

pub(crate) fn mapping_to_yaml(mapping: &Mapping) -> Result<String> {
    let mut yaml =
        serde_yaml::to_string(mapping).map_err(|source| SkillenvError::ParseFrontmatter {
            path: PathBuf::from("inline-frontmatter"),
            source,
        })?;
    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_string();
    }
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    Ok(yaml)
}
