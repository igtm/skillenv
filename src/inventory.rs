//! Read-only discovery of the custom skills each agent tool can see.
//!
//! This is deliberately separate from deployment: it reports every skill
//! directory a tool would read, including ones skillenv does not manage, so it
//! answers "what does this tool actually see here" rather than "what did we put
//! here". Deployment answers the latter.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_yaml::Value;
use walkdir::WalkDir;

use crate::{
    Config, DEFAULT_SCOPE_DIR, GENERATED_MARKER_FILE, GeneratedMarker, LOCAL_SCOPE_DIR,
    PROFILES_SCOPE_DIR, REPO_LAYOUT_DIR, Result, SkillDiscoveryState, SkillInventoryEntry,
    SkillInventoryMode, SkillInventoryOptions, SkillInventoryReport, SkillInventoryStatus,
    SkillInventoryTool, SkillenvError, all_source_roots, detect_repo_root, load_config,
    normalize_path, parse_frontmatter, repo_slug, slugify_or,
};

#[derive(Debug, Clone)]
struct InventorySourceRoot {
    name: String,
    root: PathBuf,
}

#[derive(Debug, Clone)]
struct InventoryRoot {
    scope: String,
    discovery_state: SkillDiscoveryState,
    path: PathBuf,
    precedence: usize,
    legacy: bool,
}

#[derive(Debug, Clone)]
struct InventoryEntryCandidate {
    entry: SkillInventoryEntry,
    precedence: usize,
}

#[derive(Debug, Clone, Default)]
struct RepoTreeRoots {
    agents: Vec<PathBuf>,
    claude: Vec<PathBuf>,
    opencode: Vec<PathBuf>,
    legacy_agents: Vec<PathBuf>,
}

pub fn format_skill_inventory_report(report: &SkillInventoryReport) -> String {
    let mut lines = Vec::new();
    match &report.repo_root {
        Some(repo_root) => lines.push(format!("repo root: {}", repo_root.display())),
        None => lines.push("repo root: not detected".to_string()),
    }

    lines.push(format!(
        "mode: {}",
        match report.mode {
            SkillInventoryMode::Current => "current",
            SkillInventoryMode::CurrentAndRepoTree => "current-and-repo-tree",
        }
    ));

    for tool in &report.tools {
        lines.push(String::new());
        lines.push(format!("tool: {}", tool.label()));

        let mut scope_names = Vec::new();
        for entry in report.entries.iter().filter(|entry| entry.tool == *tool) {
            if !scope_names.contains(&entry.scope) {
                scope_names.push(entry.scope.clone());
            }
        }

        for scope in scope_names {
            lines.push(format!("  scope: {scope}"));
            for entry in report
                .entries
                .iter()
                .filter(|entry| entry.tool == *tool && entry.scope == scope)
            {
                let mut details = vec![
                    format!(
                        "state={}",
                        match entry.discovery_state {
                            SkillDiscoveryState::Current => "current",
                            SkillDiscoveryState::RepoTreeOnly => "repo-tree-only",
                            SkillDiscoveryState::NestedOnDemand => "nested-on-demand",
                        }
                    ),
                    format!("name={}", entry.name),
                    format!("path={}", entry.skill_dir.display()),
                    format!(
                        "skillenv-managed={}",
                        if entry.skillenv_managed { "yes" } else { "no" }
                    ),
                    format!("origin={}", entry.skillenv_origin),
                ];
                if !entry.status.is_empty() {
                    details.push(format!(
                        "status={}",
                        entry
                            .status
                            .iter()
                            .map(|status| match status {
                                SkillInventoryStatus::Shadowed => "shadowed",
                                SkillInventoryStatus::DuplicateVisible => "duplicate-visible",
                                SkillInventoryStatus::Invalid => "invalid",
                                SkillInventoryStatus::Legacy => "legacy",
                            })
                            .collect::<Vec<_>>()
                            .join("|")
                    ));
                }
                lines.push(format!("    - {}", details.join(" ")));
            }
        }

        let tool_notes: Vec<_> = report
            .notes
            .iter()
            .filter(|note| note.starts_with(tool.label()))
            .collect();
        if !tool_notes.is_empty() {
            lines.push("  notes:".to_string());
            for note in tool_notes {
                lines.push(format!("    - {}", trim_tool_prefix(tool, note)));
            }
        }

        let tool_warnings: Vec<_> = report
            .warnings
            .iter()
            .filter(|warning| warning.starts_with(tool.label()))
            .collect();
        if !tool_warnings.is_empty() {
            lines.push("  warnings:".to_string());
            for warning in tool_warnings {
                lines.push(format!("    - {}", trim_tool_prefix(tool, warning)));
            }
        }
    }

    lines.join("\n")
}

pub(crate) fn skill_inventory_with_config(
    cwd: &Path,
    options: &SkillInventoryOptions,
    config_override: Option<&Path>,
) -> Result<SkillInventoryReport> {
    let loaded = load_config(config_override)?;
    let repo_root = detect_repo_root(cwd);
    let cwd_path = resolve_cwd_path(cwd);
    let tools = normalized_inventory_tools(&options.tools);
    let mode = if options.repo_tree {
        SkillInventoryMode::CurrentAndRepoTree
    } else {
        SkillInventoryMode::Current
    };

    let mut notes = Vec::new();
    let mut warnings = Vec::new();
    let source_roots = inventory_source_roots(
        repo_root.as_deref(),
        &loaded.config,
        loaded.base_dir.as_deref(),
    )?;

    let mut entries = Vec::new();
    for tool in &tools {
        notes.extend(tool_inventory_notes(
            *tool,
            repo_root.as_deref(),
            options.repo_tree,
        ));
        let current_roots = tool_current_roots(*tool, cwd_path.as_deref(), repo_root.as_deref());
        let repo_tree_roots = if options.repo_tree {
            tool_repo_tree_roots(*tool, repo_root.as_deref(), &current_roots)?
        } else {
            Vec::new()
        };

        let mut tool_entries = collect_inventory_entries(
            *tool,
            current_roots.into_iter().chain(repo_tree_roots).collect(),
            &source_roots,
            &mut warnings,
        )?;
        annotate_tool_conflicts(*tool, &mut tool_entries, &mut warnings);
        sort_inventory_entries(*tool, &mut tool_entries);
        entries.extend(tool_entries.into_iter().map(|candidate| candidate.entry));
    }

    Ok(SkillInventoryReport {
        repo_root,
        mode,
        tools,
        entries,
        notes,
        warnings,
    })
}

fn normalized_inventory_tools(tools: &[SkillInventoryTool]) -> Vec<SkillInventoryTool> {
    let source = if tools.is_empty() {
        SkillInventoryTool::all().to_vec()
    } else {
        tools.to_vec()
    };

    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for tool in source {
        if seen.insert(tool) {
            normalized.push(tool);
        }
    }
    normalized
}

fn resolve_cwd_path(cwd: &Path) -> Option<PathBuf> {
    if cwd.is_absolute() {
        Some(normalize_path(cwd))
    } else {
        env::current_dir()
            .ok()
            .map(|current_dir| normalize_path(&current_dir.join(cwd)))
    }
}

fn repo_ancestor_dirs(cwd: Option<&Path>, repo_root: Option<&Path>) -> Vec<PathBuf> {
    let (Some(cwd), Some(repo_root)) = (cwd, repo_root) else {
        return Vec::new();
    };
    if !cwd.starts_with(repo_root) {
        return vec![repo_root.to_path_buf()];
    }

    let mut ancestors = Vec::new();
    for candidate in cwd.ancestors() {
        if !candidate.starts_with(repo_root) {
            break;
        }
        ancestors.push(candidate.to_path_buf());
        if candidate == repo_root {
            break;
        }
    }
    ancestors
}

fn tool_inventory_notes(
    tool: SkillInventoryTool,
    repo_root: Option<&Path>,
    repo_tree: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    match tool {
        SkillInventoryTool::Claude => {
            notes.push(tool_note(
                tool,
                "plugin, enterprise, and bundled Claude skills are not enumerated",
            ));
            if repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "repo root not detected; project skills were not scanned",
                ));
            }
            if repo_tree && repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "--repo-tree requested without a repo root; nested project inventory is unavailable",
                ));
            }
        }
        SkillInventoryTool::Codex => {
            notes.push(tool_note(
                tool,
                "bundled/system Codex skills are not enumerated beyond /etc/codex/skills",
            ));
            if repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "repo root not detected; repository skills were not scanned",
                ));
            }
            if repo_tree && repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "--repo-tree requested without a repo root; repository inventory is unavailable",
                ));
            }
        }
        SkillInventoryTool::Opencode => {
            notes.push(tool_note(
                tool,
                "duplicate precedence is not collapsed; matching names remain listed as duplicate-visible",
            ));
            if repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "repo root not detected; repository skills were not scanned",
                ));
            }
            if repo_tree && repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "--repo-tree requested without a repo root; repository inventory is unavailable",
                ));
            }
        }
        SkillInventoryTool::Antigravity => {
            notes.push(tool_note(
                tool,
                "the built-in /skills UI is not enumerated by this command",
            ));
            if repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "repo root not detected; workspace skills were not scanned",
                ));
            }
            if repo_tree && repo_root.is_none() {
                notes.push(tool_note(
                    tool,
                    "--repo-tree requested without a repo root; workspace inventory is unavailable",
                ));
            }
        }
    }
    notes
}

fn tool_current_roots(
    tool: SkillInventoryTool,
    cwd: Option<&Path>,
    repo_root: Option<&Path>,
) -> Vec<InventoryRoot> {
    let ancestors = repo_ancestor_dirs(cwd, repo_root);
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut roots = Vec::new();

    match tool {
        SkillInventoryTool::Claude => {
            for (precedence, ancestor) in ancestors.iter().enumerate() {
                roots.push(InventoryRoot {
                    scope: "project".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: ancestor.join(".claude/skills"),
                    precedence: precedence + 1,
                    legacy: false,
                });
            }
            if let Some(home) = home {
                roots.push(InventoryRoot {
                    scope: "user".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: home.join(".claude/skills"),
                    precedence: 0,
                    legacy: false,
                });
            }
        }
        SkillInventoryTool::Codex => {
            for ancestor in ancestors {
                roots.push(InventoryRoot {
                    scope: "repository".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: ancestor.join(".agents/skills"),
                    precedence: usize::MAX,
                    legacy: false,
                });
            }
            if let Some(home) = home.as_deref() {
                roots.push(InventoryRoot {
                    scope: "user".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: home.join(".agents/skills"),
                    precedence: usize::MAX,
                    legacy: false,
                });
            }
            roots.push(InventoryRoot {
                scope: "admin".to_string(),
                discovery_state: SkillDiscoveryState::Current,
                path: PathBuf::from("/etc/codex/skills"),
                precedence: usize::MAX,
                legacy: false,
            });
        }
        SkillInventoryTool::Opencode => {
            for ancestor in ancestors {
                for relative in [".opencode/skills", ".claude/skills", ".agents/skills"] {
                    roots.push(InventoryRoot {
                        scope: "repository".to_string(),
                        discovery_state: SkillDiscoveryState::Current,
                        path: ancestor.join(relative),
                        precedence: usize::MAX,
                        legacy: false,
                    });
                }
            }
            if let Some(home) = home.as_deref() {
                for relative in [
                    ".config/opencode/skills",
                    ".claude/skills",
                    ".agents/skills",
                ] {
                    roots.push(InventoryRoot {
                        scope: "global".to_string(),
                        discovery_state: SkillDiscoveryState::Current,
                        path: home.join(relative),
                        precedence: usize::MAX,
                        legacy: false,
                    });
                }
            }
        }
        SkillInventoryTool::Antigravity => {
            if let Some(repo_root) = repo_root {
                roots.push(InventoryRoot {
                    scope: "workspace".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: repo_root.join(".agents/skills"),
                    precedence: usize::MAX,
                    legacy: false,
                });
                roots.push(InventoryRoot {
                    scope: "workspace".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: repo_root.join(".agent/skills"),
                    precedence: usize::MAX,
                    legacy: true,
                });
            }
            if let Some(home) = home {
                roots.push(InventoryRoot {
                    scope: "global".to_string(),
                    discovery_state: SkillDiscoveryState::Current,
                    path: home.join(".gemini/antigravity/skills"),
                    precedence: usize::MAX,
                    legacy: false,
                });
            }
        }
    }

    dedup_inventory_roots(roots)
}

fn tool_repo_tree_roots(
    tool: SkillInventoryTool,
    repo_root: Option<&Path>,
    current_roots: &[InventoryRoot],
) -> Result<Vec<InventoryRoot>> {
    let Some(repo_root) = repo_root else {
        return Ok(Vec::new());
    };
    let scanned = scan_repo_tree_skill_roots(repo_root)?;
    let current_paths: BTreeSet<_> = current_roots
        .iter()
        .map(|root| normalize_path(&root.path))
        .collect();

    let mut roots = Vec::new();
    match tool {
        SkillInventoryTool::Claude => {
            append_repo_tree_roots(
                &mut roots,
                "project",
                SkillDiscoveryState::NestedOnDemand,
                scanned.claude,
                &current_paths,
                false,
            );
        }
        SkillInventoryTool::Codex => {
            append_repo_tree_roots(
                &mut roots,
                "repository",
                SkillDiscoveryState::RepoTreeOnly,
                scanned.agents,
                &current_paths,
                false,
            );
        }
        SkillInventoryTool::Opencode => {
            append_repo_tree_roots(
                &mut roots,
                "repository",
                SkillDiscoveryState::RepoTreeOnly,
                scanned.opencode,
                &current_paths,
                false,
            );
            append_repo_tree_roots(
                &mut roots,
                "repository",
                SkillDiscoveryState::RepoTreeOnly,
                scanned.claude,
                &current_paths,
                false,
            );
            append_repo_tree_roots(
                &mut roots,
                "repository",
                SkillDiscoveryState::RepoTreeOnly,
                scanned.agents,
                &current_paths,
                false,
            );
        }
        SkillInventoryTool::Antigravity => {
            append_repo_tree_roots(
                &mut roots,
                "workspace",
                SkillDiscoveryState::RepoTreeOnly,
                scanned.agents,
                &current_paths,
                false,
            );
            append_repo_tree_roots(
                &mut roots,
                "workspace",
                SkillDiscoveryState::RepoTreeOnly,
                scanned.legacy_agents,
                &current_paths,
                true,
            );
        }
    }

    Ok(dedup_inventory_roots(roots))
}

fn append_repo_tree_roots(
    roots: &mut Vec<InventoryRoot>,
    scope: &str,
    discovery_state: SkillDiscoveryState,
    paths: Vec<PathBuf>,
    current_paths: &BTreeSet<PathBuf>,
    legacy: bool,
) {
    for path in paths {
        let normalized = normalize_path(&path);
        if current_paths.contains(&normalized) {
            continue;
        }
        roots.push(InventoryRoot {
            scope: scope.to_string(),
            discovery_state,
            path,
            precedence: usize::MAX,
            legacy,
        });
    }
}

fn dedup_inventory_roots(roots: Vec<InventoryRoot>) -> Vec<InventoryRoot> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for root in roots {
        let key = (
            root.scope.clone(),
            root.discovery_state,
            normalize_path(&root.path),
            root.legacy,
        );
        if seen.insert(key) {
            deduped.push(root);
        }
    }
    deduped
}

fn scan_repo_tree_skill_roots(repo_root: &Path) -> Result<RepoTreeRoots> {
    let mut roots = RepoTreeRoots::default();
    let walker = WalkDir::new(repo_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_walk_repo_tree_entry);

    for entry in walker {
        let entry = entry.map_err(|error| SkillenvError::ReadFile {
            path: repo_root.to_path_buf(),
            source: io::Error::other(error),
        })?;
        if !(entry.file_type().is_dir()
            || (entry.file_type().is_symlink() && entry.path().is_dir()))
        {
            continue;
        }
        if entry.file_name() != OsStr::new("skills") {
            continue;
        }
        let Some(parent) = entry.path().parent() else {
            continue;
        };
        match parent.file_name().and_then(OsStr::to_str) {
            Some(".agents") => roots.agents.push(entry.path().to_path_buf()),
            Some(".claude") => roots.claude.push(entry.path().to_path_buf()),
            Some(".opencode") => roots.opencode.push(entry.path().to_path_buf()),
            Some(".agent") => roots.legacy_agents.push(entry.path().to_path_buf()),
            _ => {}
        }
    }

    sort_dedup_paths(&mut roots.agents);
    sort_dedup_paths(&mut roots.claude);
    sort_dedup_paths(&mut roots.opencode);
    sort_dedup_paths(&mut roots.legacy_agents);
    Ok(roots)
}

fn should_walk_repo_tree_entry(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|name| !matches!(name, ".git" | "target"))
        .unwrap_or(true)
}

fn sort_dedup_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn inventory_source_roots(
    repo_root: Option<&Path>,
    config: &Config,
    config_base_dir: Option<&Path>,
) -> Result<Vec<InventorySourceRoot>> {
    let Some(repo_root) = repo_root else {
        return Ok(Vec::new());
    };
    let repo_slug = repo_slug(repo_root);
    let mut roots: Vec<_> = all_source_roots(repo_root, &repo_slug, config, config_base_dir)?
        .into_iter()
        .map(|(name, root)| InventorySourceRoot { name, root })
        .collect();
    roots.sort_by(|left, right| {
        right
            .root
            .components()
            .count()
            .cmp(&left.root.components().count())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(roots)
}

fn collect_inventory_entries(
    tool: SkillInventoryTool,
    roots: Vec<InventoryRoot>,
    source_roots: &[InventorySourceRoot],
    warnings: &mut Vec<String>,
) -> Result<Vec<InventoryEntryCandidate>> {
    let mut entries = Vec::new();
    for root in roots {
        if !root.path.is_dir() {
            continue;
        }
        let dir_entries = fs::read_dir(&root.path).map_err(|source| SkillenvError::ReadFile {
            path: root.path.clone(),
            source,
        })?;
        for dir_entry in dir_entries {
            let dir_entry = dir_entry.map_err(|source| SkillenvError::ReadFile {
                path: root.path.clone(),
                source,
            })?;
            let path = dir_entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|source| SkillenvError::ReadFile {
                    path: path.clone(),
                    source,
                })?;
            if !metadata.is_dir() && !metadata.file_type().is_symlink() {
                continue;
            }

            let mut candidate =
                inspect_inventory_skill(tool, &root, &path, source_roots, warnings)?;
            candidate.precedence = root.precedence;
            entries.push(candidate);
        }
    }
    Ok(entries)
}

fn inspect_inventory_skill(
    tool: SkillInventoryTool,
    root: &InventoryRoot,
    skill_dir: &Path,
    source_roots: &[InventorySourceRoot],
    warnings: &mut Vec<String>,
) -> Result<InventoryEntryCandidate> {
    let skill_md_path = skill_dir.join("SKILL.md");
    let mut status = Vec::new();
    if root.legacy {
        status.push(SkillInventoryStatus::Legacy);
    }

    let metadata = resolve_inventory_skill_metadata(tool, skill_dir, &skill_md_path, warnings)?;
    if metadata.invalid {
        push_inventory_status(&mut status, SkillInventoryStatus::Invalid);
    }

    let managed_source = detect_skillenv_managed_source(skill_dir, source_roots)?;
    let (skillenv_managed, skillenv_origin) = match managed_source {
        Some(origin) => (true, origin),
        None => (false, "manual".to_string()),
    };

    Ok(InventoryEntryCandidate {
        precedence: root.precedence,
        entry: SkillInventoryEntry {
            tool,
            scope: root.scope.clone(),
            discovery_state: root.discovery_state,
            name: metadata.name,
            description: metadata.description,
            skill_dir: skill_dir.to_path_buf(),
            skill_md: metadata.skill_md,
            skillenv_managed,
            skillenv_origin,
            status,
        },
    })
}

struct InventorySkillMetadata {
    name: String,
    description: Option<String>,
    skill_md: Option<PathBuf>,
    invalid: bool,
}

fn resolve_inventory_skill_metadata(
    tool: SkillInventoryTool,
    skill_dir: &Path,
    skill_md_path: &Path,
    warnings: &mut Vec<String>,
) -> Result<InventorySkillMetadata> {
    let fallback_name = skill_dir
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("skill")
        .to_string();

    if !skill_md_path.is_file() {
        warnings.push(tool_warning(
            tool,
            format!("missing SKILL.md under {}", skill_dir.display()),
        ));
        return Ok(InventorySkillMetadata {
            name: fallback_name,
            description: None,
            skill_md: None,
            invalid: true,
        });
    }

    let raw = fs::read_to_string(skill_md_path).map_err(|source| SkillenvError::ReadFile {
        path: skill_md_path.to_path_buf(),
        source,
    })?;
    let (frontmatter, _) = match parse_frontmatter(skill_md_path, &raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            warnings.push(tool_warning(tool, error.to_string()));
            return Ok(InventorySkillMetadata {
                name: fallback_name,
                description: None,
                skill_md: Some(skill_md_path.to_path_buf()),
                invalid: true,
            });
        }
    };

    let mut invalid = false;
    let name = match frontmatter.get(Value::String("name".to_string())) {
        Some(Value::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
        Some(_) => {
            warnings.push(tool_warning(
                tool,
                format!(
                    "non-string or empty frontmatter name in {}",
                    skill_md_path.display()
                ),
            ));
            invalid = true;
            fallback_name
        }
        None => fallback_name,
    };

    let description = match frontmatter.get(Value::String("description".to_string())) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Some(_) => {
            warnings.push(tool_warning(
                tool,
                format!(
                    "non-string frontmatter description in {}",
                    skill_md_path.display()
                ),
            ));
            invalid = true;
            None
        }
        None => None,
    };

    Ok(InventorySkillMetadata {
        name,
        description,
        skill_md: Some(skill_md_path.to_path_buf()),
        invalid,
    })
}

fn detect_skillenv_managed_source(
    skill_dir: &Path,
    source_roots: &[InventorySourceRoot],
) -> Result<Option<String>> {
    match rendered_marker_origin(skill_dir) {
        // v1 says which manifest owns it directly, so no path inference is needed.
        Some(MarkerOrigin::Manifest(manifest)) => {
            return Ok(Some(format!("manifest:{manifest}")));
        }
        Some(MarkerOrigin::SourcePath(path)) => {
            return Ok(resolve_skillenv_origin(Path::new(&path), source_roots));
        }
        None => {}
    }

    let metadata = fs::symlink_metadata(skill_dir).map_err(|source| SkillenvError::ReadFile {
        path: skill_dir.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(skill_dir).map_err(|source| SkillenvError::ReadFile {
            path: skill_dir.to_path_buf(),
            source,
        })?;
        let resolved = if target.is_absolute() {
            normalize_path(&target)
        } else {
            let base = skill_dir.parent().unwrap_or_else(|| Path::new("."));
            normalize_path(&base.join(target))
        };
        return Ok(resolve_skillenv_origin(&resolved, source_roots));
    }

    Ok(None)
}

/// How a deployed directory identifies itself, if it does.
///
/// Two marker formats exist. The v1 marker records the manifest it belongs to and
/// carries no source path — deliberately, since v0 made removal conditional on that
/// path still resolving. The v0 marker records the path it was rendered from.
///
/// An unreadable or unrecognised marker yields `None` rather than an error. Listing
/// what a tool can see must not fail because one directory is in a format this
/// build does not know; before this, a v1 deployment made `skillenv skills` abort
/// with `missing field 'repo'`.
enum MarkerOrigin {
    /// v1: the manifest identifier.
    Manifest(String),
    /// v0: the path the skill was rendered from.
    SourcePath(String),
}

fn rendered_marker_origin(skill_dir: &Path) -> Option<MarkerOrigin> {
    let raw = fs::read_to_string(skill_dir.join(GENERATED_MARKER_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;

    // v1 first: it is the format being written now.
    if let Some(manifest) = value.get("manifest").and_then(serde_json::Value::as_str) {
        return Some(MarkerOrigin::Manifest(manifest.to_string()));
    }
    if let Ok(marker) = serde_json::from_value::<GeneratedMarker>(value) {
        return Some(MarkerOrigin::SourcePath(marker.source));
    }
    None
}

fn resolve_skillenv_origin(
    source_path: &Path,
    source_roots: &[InventorySourceRoot],
) -> Option<String> {
    let normalized = normalize_path(source_path);
    for source_root in source_roots {
        let root = normalize_path(&source_root.root);
        if !normalized.starts_with(&root) {
            continue;
        }
        if let Some(origin) = origin_from_source_root(&source_root.name, &root, &normalized) {
            return Some(origin);
        }
    }
    infer_skillenv_origin_from_path(&normalized)
}

fn origin_from_source_root(name: &str, root: &Path, source_path: &Path) -> Option<String> {
    let relative = source_path.strip_prefix(root).ok()?;
    if name == "repo" {
        repo_origin_from_relative_path(relative)
    } else if let Some(managed) = name.strip_prefix("managed:") {
        Some(format!("managed:{managed}"))
    } else {
        Some(format!("external:{}", slugify_or(name, "source")))
    }
}

fn repo_origin_from_relative_path(relative: &Path) -> Option<String> {
    let mut components = relative.components();
    match components.next()? {
        Component::Normal(part) if part == OsStr::new(DEFAULT_SCOPE_DIR) => {
            Some("repo:default".to_string())
        }
        Component::Normal(part) if part == OsStr::new(LOCAL_SCOPE_DIR) => {
            Some("repo:local".to_string())
        }
        Component::Normal(part) if part == OsStr::new(PROFILES_SCOPE_DIR) => {
            let profile = components
                .next()
                .and_then(|component| match component {
                    Component::Normal(name) => name.to_str(),
                    _ => None,
                })
                .map(|name| slugify_or(name, "profile"))?;
            Some(format!("repo:profile:{profile}"))
        }
        _ => None,
    }
}

fn infer_skillenv_origin_from_path(path: &Path) -> Option<String> {
    let components: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_string),
            _ => None,
        })
        .collect();

    for (index, component) in components.iter().enumerate() {
        if component != REPO_LAYOUT_DIR {
            continue;
        }
        match components.get(index + 1).map(String::as_str) {
            Some(DEFAULT_SCOPE_DIR) => return Some("repo:default".to_string()),
            Some(LOCAL_SCOPE_DIR) => return Some("repo:local".to_string()),
            Some(PROFILES_SCOPE_DIR) => {
                let profile = components.get(index + 2)?;
                return Some(format!("repo:profile:{}", slugify_or(profile, "profile")));
            }
            Some("remote") => {
                let managed_name = components.get(index + 2)?;
                return Some(format!("managed:{}", slugify_or(managed_name, "source")));
            }
            _ => {}
        }
    }
    None
}

fn annotate_tool_conflicts(
    tool: SkillInventoryTool,
    entries: &mut [InventoryEntryCandidate],
    warnings: &mut Vec<String>,
) {
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        by_name
            .entry(entry.entry.name.clone())
            .or_default()
            .push(index);
    }

    for (name, indices) in by_name {
        if indices.len() < 2 {
            continue;
        }

        match tool {
            SkillInventoryTool::Claude => {
                let mut ordered = indices;
                ordered.sort_by_key(|index| {
                    (
                        inventory_discovery_rank(entries[*index].entry.discovery_state),
                        entries[*index].precedence,
                        entries[*index].entry.skill_dir.clone(),
                    )
                });
                let visible = entries[ordered[0]].entry.skill_dir.display().to_string();
                let shadowed = ordered
                    .iter()
                    .skip(1)
                    .map(|index| {
                        push_inventory_status(
                            &mut entries[*index].entry.status,
                            SkillInventoryStatus::Shadowed,
                        );
                        entries[*index].entry.skill_dir.display().to_string()
                    })
                    .collect::<Vec<_>>();
                warnings.push(tool_warning(
                    tool,
                    format!(
                        "shadowed visible skill '{name}': active={} shadowed={}",
                        visible,
                        shadowed.join(", ")
                    ),
                ));
            }
            _ => {
                let paths = indices
                    .iter()
                    .map(|index| {
                        push_inventory_status(
                            &mut entries[*index].entry.status,
                            SkillInventoryStatus::DuplicateVisible,
                        );
                        entries[*index].entry.skill_dir.display().to_string()
                    })
                    .collect::<Vec<_>>();
                warnings.push(tool_warning(
                    tool,
                    format!("duplicate visible skill '{name}': {}", paths.join(", ")),
                ));
            }
        }
    }
}

fn sort_inventory_entries(tool: SkillInventoryTool, entries: &mut [InventoryEntryCandidate]) {
    entries.sort_by(|left, right| {
        inventory_discovery_rank(left.entry.discovery_state)
            .cmp(&inventory_discovery_rank(right.entry.discovery_state))
            .then_with(|| {
                inventory_scope_rank(tool, &left.entry.scope)
                    .cmp(&inventory_scope_rank(tool, &right.entry.scope))
            })
            .then_with(|| left.precedence.cmp(&right.precedence))
            .then_with(|| left.entry.name.cmp(&right.entry.name))
            .then_with(|| left.entry.skill_dir.cmp(&right.entry.skill_dir))
    });
}

fn inventory_discovery_rank(state: SkillDiscoveryState) -> usize {
    match state {
        SkillDiscoveryState::Current => 0,
        SkillDiscoveryState::NestedOnDemand => 1,
        SkillDiscoveryState::RepoTreeOnly => 2,
    }
}

fn inventory_scope_rank(tool: SkillInventoryTool, scope: &str) -> usize {
    match tool {
        SkillInventoryTool::Claude => match scope {
            "project" => 0,
            "user" => 1,
            _ => 2,
        },
        SkillInventoryTool::Codex => match scope {
            "repository" => 0,
            "user" => 1,
            "admin" => 2,
            _ => 3,
        },
        SkillInventoryTool::Opencode => match scope {
            "repository" => 0,
            "global" => 1,
            _ => 2,
        },
        SkillInventoryTool::Antigravity => match scope {
            "workspace" => 0,
            "global" => 1,
            _ => 2,
        },
    }
}

fn push_inventory_status(statuses: &mut Vec<SkillInventoryStatus>, status: SkillInventoryStatus) {
    if !statuses.contains(&status) {
        statuses.push(status);
    }
}

fn tool_note(tool: SkillInventoryTool, note: impl Into<String>) -> String {
    format!("{}: {}", tool.label(), note.into())
}

fn tool_warning(tool: SkillInventoryTool, warning: impl Into<String>) -> String {
    format!("{}: {}", tool.label(), warning.into())
}

fn trim_tool_prefix(tool: &SkillInventoryTool, value: &str) -> String {
    value
        .strip_prefix(&format!("{}: ", tool.label()))
        .unwrap_or(value)
        .to_string()
}
