//! Turning a source skill directory into the bytes a provider consumes.
//!
//! Frontmatter is parsed, adjusted, and re-emitted; the body is passed through
//! verbatim. Nothing here touches the filesystem layout of a target directory —
//! that is deployment's job.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};
use walkdir::WalkDir;

use crate::{Result, ScopeKey, SkillSource, SkillenvError, ensure_dir};

pub(crate) fn render_skill_markdown(
    repo_slug: &str,
    scope: &ScopeKey,
    source: &SkillSource,
    generated_name: &str,
    skill_md_path: &Path,
) -> Result<String> {
    let raw = fs::read_to_string(skill_md_path).map_err(|source| SkillenvError::ReadFile {
        path: skill_md_path.to_path_buf(),
        source,
    })?;
    let (mut metadata, body) = parse_frontmatter(skill_md_path, &raw)?;
    metadata.insert(
        Value::String("name".to_string()),
        Value::String(generated_name.to_string()),
    );
    ensure_render_description(&mut metadata, &body, source);
    merge_render_metadata(&mut metadata, repo_slug, scope, source, skill_md_path)?;

    let yaml = mapping_to_yaml(&metadata)?;
    let separator = if body.is_empty() || body.starts_with('\n') || body.starts_with("\r\n") {
        "\n"
    } else {
        "\n\n"
    };
    Ok(format!("---\n{yaml}---{separator}{body}"))
}

fn ensure_render_description(metadata: &mut Mapping, body: &str, source: &SkillSource) {
    let description_key = Value::String("description".to_string());
    let existing_description = metadata
        .get(&description_key)
        .and_then(Value::as_str)
        .map(sanitize_render_description);
    if let Some(description) = existing_description.filter(|value| !value.is_empty()) {
        metadata.insert(description_key, Value::String(description));
        return;
    }

    metadata.insert(
        description_key,
        Value::String(render_description_fallback(body, source)),
    );
}

fn render_description_fallback(body: &str, source: &SkillSource) -> String {
    if let Some(summary) = summarize_markdown_body(body) {
        return summary;
    }

    format!(
        "Instructions for the {} skill.",
        source.skill_slug.replace('-', " ")
    )
}

fn sanitize_render_description(value: &str) -> String {
    let trimmed = value.trim();
    if is_legacy_skillenv_description(trimmed) {
        return String::new();
    }

    if let Some(index) = trimmed.rfind(" [skillenv: ") {
        let suffix = &trimmed[(index + 1)..];
        if is_legacy_skillenv_description(suffix) {
            return trimmed[..index].trim_end().to_string();
        }
    }

    trimmed.to_string()
}

fn is_legacy_skillenv_description(value: &str) -> bool {
    value.starts_with("[skillenv: ") && value.contains("] repo=")
}

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

fn summarize_markdown_body(body: &str) -> Option<String> {
    let fenced = fenced_lines(body);
    for (index, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if fenced.get(index).copied().unwrap_or(false)
            || trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("<!--")
        {
            continue;
        }

        let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            continue;
        }

        return Some(truncate_description(&normalized));
    }

    None
}

fn truncate_description(value: &str) -> String {
    const MAX_DESCRIPTION_CHARS: usize = 1024;

    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(MAX_DESCRIPTION_CHARS).collect();
    if chars.next().is_none() {
        truncated
    } else {
        let mut shortened: String = truncated.chars().take(MAX_DESCRIPTION_CHARS - 3).collect();
        shortened.push_str("...");
        shortened
    }
}

fn merge_render_metadata(
    frontmatter: &mut Mapping,
    repo_slug: &str,
    scope: &ScopeKey,
    source: &SkillSource,
    skill_md_path: &Path,
) -> Result<()> {
    let metadata_key = Value::String("metadata".to_string());
    let mut metadata = match frontmatter.get(&metadata_key) {
        Some(Value::Mapping(mapping)) => mapping.clone(),
        Some(_) => {
            return Err(SkillenvError::InvalidMetadataField {
                path: skill_md_path.to_path_buf(),
            });
        }
        None => Mapping::new(),
    };
    metadata.insert(
        Value::String("skillenv.source".to_string()),
        Value::String(format!(
            "{}/{}/{}",
            repo_slug,
            scope.context_path(),
            source.skill_slug
        )),
    );
    metadata.insert(
        Value::String("skillenv.scope_origin".to_string()),
        Value::String(source.scope_origin.display().to_string()),
    );
    frontmatter.insert(metadata_key, Value::Mapping(metadata));
    Ok(())
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

fn mapping_to_yaml(mapping: &Mapping) -> Result<String> {
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

pub(crate) fn copy_source_tree(source_dir: &Path, target_dir: &Path) -> Result<()> {
    for entry in WalkDir::new(source_dir) {
        let entry = entry.map_err(|error| SkillenvError::ReadFile {
            path: source_dir.to_path_buf(),
            source: io::Error::other(error),
        })?;
        let relative =
            entry
                .path()
                .strip_prefix(source_dir)
                .map_err(|error| SkillenvError::ReadFile {
                    path: source_dir.to_path_buf(),
                    source: io::Error::other(error),
                })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if relative == Path::new("SKILL.md") {
            continue;
        }

        let destination = target_dir.join(relative);
        if entry.file_type().is_dir() {
            ensure_dir(&destination)?;
            continue;
        }

        if let Some(parent) = destination.parent() {
            ensure_dir(parent)?;
        }
        fs::copy(entry.path(), &destination).map_err(|source| SkillenvError::WriteFile {
            path: destination,
            source,
        })?;
    }
    Ok(())
}
