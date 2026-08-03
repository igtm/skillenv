use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use walkdir::WalkDir;

use crate::{
    LinkOptions, Report, Result, ScopeSelector, SkillenvError, TargetOverride, detect_repo_root,
    ensure_dir, include_claude_target, link_repo, load_config, normalize_path,
    require_repo_initialized, slugify_or,
};

const LOCK_FILE_NAME: &str = "skillenv.lock.json";
const LOCK_FILE_VERSION: u32 = 1;
const MANAGED_SOURCE_MARKER_FILE: &str = ".skillenv-source.json";

#[derive(Debug, Clone)]
pub struct AddSourceOptions {
    pub source: String,
    pub into: Option<PathBuf>,
    pub skills: Vec<String>,
    pub ref_name: Option<String>,
    pub name: Option<String>,
    pub claude: TargetOverride,
}

#[derive(Debug, Clone)]
pub struct AddSourceReport {
    pub name: String,
    pub install_root: PathBuf,
    pub selected_skills: Vec<String>,
    pub resolved_revision: String,
    pub link_report: Report,
}

#[derive(Debug, Clone)]
pub struct FetchSourcesOptions {
    pub names: Vec<String>,
    pub claude: TargetOverride,
}

#[derive(Debug, Clone)]
pub struct FetchSourcesReport {
    pub fetched: Vec<FetchedLockedSource>,
    pub link_report: Option<Report>,
}

#[derive(Debug, Clone)]
pub struct FetchedLockedSource {
    pub name: String,
    pub install_root: PathBuf,
    pub resolved_revision: String,
    pub selected_skills: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateSourcesOptions {
    pub names: Vec<String>,
    pub claude: TargetOverride,
}

#[derive(Debug, Clone)]
pub struct UpdateSourcesReport {
    pub updated: Vec<UpdatedSource>,
    pub unchanged: Vec<UpdatedSource>,
    pub link_report: Option<Report>,
}

#[derive(Debug, Clone)]
pub struct UpdatedSource {
    pub name: String,
    pub install_root: PathBuf,
    pub old_revision: String,
    pub new_revision: String,
    pub selected_skills: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct InstalledSourceRoot {
    pub(crate) name: String,
    pub(crate) root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedSourceDetails {
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) transport: String,
    pub(crate) kind: String,
    pub(crate) requested_ref: Option<String>,
    pub(crate) subdir: Option<String>,
    pub(crate) install_root: PathBuf,
    pub(crate) selected_skills: Vec<String>,
    pub(crate) resolved_revision: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LockFile {
    version: u32,
    #[serde(default)]
    sources: Vec<LockedSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockedSource {
    name: String,
    source: String,
    kind: LockedSourceKind,
    transport: String,
    requested_ref: Option<String>,
    subdir: Option<String>,
    install_root: String,
    selected_skills: Vec<String>,
    resolved_revision: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LockedSourceKind {
    Git,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedSourceMarker {
    name: String,
}

#[derive(Debug, Clone)]
struct ParsedSource {
    display_source: String,
    kind: LockedSourceKind,
    transport: String,
    requested_ref: Option<String>,
    subdir: Option<PathBuf>,
    default_name: String,
}

#[derive(Debug)]
struct PreparedSource {
    selected_skills: Vec<String>,
    resolved_revision: String,
}

#[derive(Debug)]
struct FetchedSource {
    _tempdir: Option<TempDir>,
    root: PathBuf,
    resolved_revision: String,
    versioned: bool,
}

#[derive(Debug)]
struct SourceTree {
    skills: Vec<TreeSkill>,
}

#[derive(Debug, Clone)]
struct TreeSkill {
    slug: String,
    source_dir: PathBuf,
    scope: InstalledScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstalledScope {
    Default,
    Local,
    Profile,
}

pub fn add_source(cwd: impl AsRef<Path>, options: AddSourceOptions) -> Result<AddSourceReport> {
    let cwd = cwd.as_ref();
    let repo_root = detect_repo_root(cwd).ok_or(SkillenvError::RepoRequired)?;
    let loaded = load_config(None)?;
    require_repo_initialized(
        &repo_root,
        include_claude_target(&loaded.config, options.claude),
    )?;
    let parsed = parse_source(&options.source, cwd, options.ref_name.as_deref())?;
    let name = options
        .name
        .as_deref()
        .map(|value| slugify_or(value, &parsed.default_name))
        .unwrap_or_else(|| parsed.default_name.clone());
    let install_root = resolve_install_root(&repo_root, &name, options.into.as_deref());
    let mut lock_file = load_lock_file(&repo_root)?;

    let occupied_install_roots = occupied_install_roots(&repo_root, &lock_file);
    ensure_install_root_available(&occupied_install_roots, &name, &install_root)?;
    let fetched = fetch_source(&parsed)?;
    let selected_skills = normalize_selected_skills(&options.skills);
    let prepared =
        install_fetched_source(&repo_root, &name, &install_root, &fetched, &selected_skills)?;

    let new_entry = LockedSource {
        name: name.clone(),
        source: parsed.display_source,
        kind: parsed.kind,
        transport: parsed.transport,
        requested_ref: parsed.requested_ref,
        subdir: parsed.subdir.map(|path| path.display().to_string()),
        install_root: store_path(&repo_root, &install_root),
        selected_skills: prepared.selected_skills.clone(),
        resolved_revision: prepared.resolved_revision.clone(),
    };

    let old_install_root = lock_file
        .sources
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| resolve_stored_path(&repo_root, &entry.install_root));
    upsert_lock_entry(&mut lock_file, new_entry);
    save_lock_file(&repo_root, &lock_file)?;

    if let Some(old_root) = old_install_root
        && old_root != install_root
    {
        remove_managed_install_root(&old_root, &name)?;
    }

    let link_report = link_repo(
        &repo_root,
        LinkOptions {
            selector: ScopeSelector::DefaultLocal,
            claude: options.claude,
            quiet: true,
        },
    )?;

    Ok(AddSourceReport {
        name,
        install_root,
        selected_skills: prepared.selected_skills,
        resolved_revision: prepared.resolved_revision,
        link_report,
    })
}

pub fn update_sources(
    cwd: impl AsRef<Path>,
    options: UpdateSourcesOptions,
) -> Result<UpdateSourcesReport> {
    let cwd = cwd.as_ref();
    let repo_root = detect_repo_root(cwd).ok_or(SkillenvError::RepoRequired)?;
    let loaded = load_config(None)?;
    require_repo_initialized(
        &repo_root,
        include_claude_target(&loaded.config, options.claude),
    )?;
    let mut lock_file = load_lock_file(&repo_root)?;
    let requested_names = normalize_selected_skills(&options.names);
    validate_requested_names(&lock_file, &requested_names)?;

    let mut updated = Vec::new();
    let mut unchanged = Vec::new();
    let mut changed_any = false;

    let occupied_install_roots = occupied_install_roots(&repo_root, &lock_file);
    for entry in &mut lock_file.sources {
        if !requested_names.is_empty() && !requested_names.contains(&entry.name) {
            continue;
        }

        let parsed = parsed_from_lock_entry(entry);
        let fetched = fetch_source(&parsed)?;
        let install_root = resolve_stored_path(&repo_root, &entry.install_root);
        let previous_revision = entry.resolved_revision.clone();

        let should_update =
            !fetched.versioned || fetched.resolved_revision != entry.resolved_revision;
        if should_update {
            ensure_install_root_available(&occupied_install_roots, &entry.name, &install_root)?;
            let prepared = install_fetched_source(
                &repo_root,
                &entry.name,
                &install_root,
                &fetched,
                &entry.selected_skills,
            )?;
            entry.resolved_revision = prepared.resolved_revision.clone();
            entry.selected_skills = prepared.selected_skills.clone();
            updated.push(UpdatedSource {
                name: entry.name.clone(),
                install_root,
                old_revision: previous_revision,
                new_revision: entry.resolved_revision.clone(),
                selected_skills: entry.selected_skills.clone(),
            });
            changed_any = true;
        } else {
            unchanged.push(UpdatedSource {
                name: entry.name.clone(),
                install_root,
                old_revision: previous_revision.clone(),
                new_revision: previous_revision,
                selected_skills: entry.selected_skills.clone(),
            });
        }
    }

    if changed_any {
        save_lock_file(&repo_root, &lock_file)?;
    }

    let link_report = if changed_any {
        Some(link_repo(
            &repo_root,
            LinkOptions {
                selector: ScopeSelector::DefaultLocal,
                claude: options.claude,
                quiet: true,
            },
        )?)
    } else {
        None
    };

    Ok(UpdateSourcesReport {
        updated,
        unchanged,
        link_report,
    })
}

pub fn fetch_sources(
    cwd: impl AsRef<Path>,
    options: FetchSourcesOptions,
) -> Result<FetchSourcesReport> {
    let cwd = cwd.as_ref();
    let repo_root = detect_repo_root(cwd).ok_or(SkillenvError::RepoRequired)?;
    let loaded = load_config(None)?;
    require_repo_initialized(
        &repo_root,
        include_claude_target(&loaded.config, options.claude),
    )?;
    let lock_file = load_lock_file(&repo_root)?;
    let requested_names = normalize_selected_skills(&options.names);
    validate_requested_names(&lock_file, &requested_names)?;

    let occupied_install_roots = occupied_install_roots(&repo_root, &lock_file);
    let mut fetched_sources = Vec::new();

    for entry in &lock_file.sources {
        if !requested_names.is_empty() && !requested_names.contains(&entry.name) {
            continue;
        }

        let fetched = fetch_locked_source(entry)?;
        let install_root = resolve_stored_path(&repo_root, &entry.install_root);
        ensure_install_root_available(&occupied_install_roots, &entry.name, &install_root)?;
        let prepared = install_fetched_source(
            &repo_root,
            &entry.name,
            &install_root,
            &fetched,
            &entry.selected_skills,
        )?;
        fetched_sources.push(FetchedLockedSource {
            name: entry.name.clone(),
            install_root,
            resolved_revision: prepared.resolved_revision,
            selected_skills: prepared.selected_skills,
        });
    }

    let link_report = if fetched_sources.is_empty() {
        None
    } else {
        Some(link_repo(
            &repo_root,
            LinkOptions {
                selector: ScopeSelector::DefaultLocal,
                claude: options.claude,
                quiet: true,
            },
        )?)
    };

    Ok(FetchSourcesReport {
        fetched: fetched_sources,
        link_report,
    })
}

pub fn format_add_source_report(report: &AddSourceReport) -> String {
    let mut lines = vec![
        format!("added source {}", report.name),
        format!("install root: {}", report.install_root.display()),
        format!("resolved revision: {}", report.resolved_revision),
    ];
    if !report.selected_skills.is_empty() {
        lines.push(format!("skills: {}", report.selected_skills.join(", ")));
    }
    lines.push(crate::format_link_report(&report.link_report, "linked"));
    lines.join("\n")
}

pub fn format_fetch_sources_report(report: &FetchSourcesReport) -> String {
    let mut lines = Vec::new();
    for source in &report.fetched {
        lines.push(format!(
            "fetched {} ({})",
            source.name, source.resolved_revision
        ));
    }
    if lines.is_empty() {
        lines.push("no managed sources found".to_string());
    }
    if let Some(link_report) = &report.link_report {
        lines.push(crate::format_link_report(link_report, "linked"));
    }
    lines.join("\n")
}

pub fn format_update_sources_report(report: &UpdateSourcesReport) -> String {
    let mut lines = Vec::new();
    for source in &report.updated {
        lines.push(format!(
            "updated {} {} -> {}",
            source.name, source.old_revision, source.new_revision
        ));
    }
    for source in &report.unchanged {
        lines.push(format!(
            "up to date {} ({})",
            source.name, source.new_revision
        ));
    }
    if lines.is_empty() {
        lines.push("no managed sources found".to_string());
    }
    if let Some(link_report) = &report.link_report {
        lines.push(crate::format_link_report(link_report, "linked"));
    }
    lines.join("\n")
}

pub(crate) fn installed_source_roots(repo_root: &Path) -> Result<Vec<InstalledSourceRoot>> {
    let lock_file = load_lock_file(repo_root)?;
    Ok(lock_file
        .sources
        .into_iter()
        .map(|entry| InstalledSourceRoot {
            name: format!("managed:{}", entry.name),
            root: resolve_stored_path(repo_root, &entry.install_root),
        })
        .collect())
}

pub(crate) fn managed_source_details(repo_root: &Path) -> Result<Vec<ManagedSourceDetails>> {
    let lock_file = load_lock_file(repo_root)?;
    Ok(lock_file
        .sources
        .into_iter()
        .map(|entry| ManagedSourceDetails {
            name: entry.name,
            source: entry.source,
            transport: entry.transport,
            kind: match entry.kind {
                LockedSourceKind::Git => "git".to_string(),
                LockedSourceKind::Local => "local".to_string(),
            },
            requested_ref: entry.requested_ref,
            subdir: entry.subdir,
            install_root: resolve_stored_path(repo_root, &entry.install_root),
            selected_skills: entry.selected_skills,
            resolved_revision: entry.resolved_revision,
        })
        .collect())
}

fn parse_source(source: &str, cwd: &Path, ref_override: Option<&str>) -> Result<ParsedSource> {
    let local_candidate = if Path::new(source).is_absolute() {
        PathBuf::from(source)
    } else {
        cwd.join(source)
    };
    if local_candidate.exists() {
        let name = slugify_or(
            local_candidate
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("source"),
            "source",
        );
        return Ok(ParsedSource {
            display_source: source.to_string(),
            kind: LockedSourceKind::Local,
            transport: normalize_local_source(&local_candidate),
            requested_ref: ref_override.map(str::to_string),
            subdir: None,
            default_name: name,
        });
    }

    if let Some(rest) = source.strip_prefix("https://github.com/") {
        return parse_github_http_source(source, rest, ref_override);
    }

    if is_github_shorthand(source) {
        return parse_github_shorthand_source(source, ref_override);
    }

    if looks_like_git_remote(source) {
        let default_name = default_name_from_remote(source);
        return Ok(ParsedSource {
            display_source: source.to_string(),
            kind: LockedSourceKind::Git,
            transport: source.to_string(),
            requested_ref: ref_override.map(str::to_string),
            subdir: None,
            default_name,
        });
    }

    Err(SkillenvError::InvalidSource {
        input: source.to_string(),
        message: "unsupported source format".to_string(),
    })
}

fn parse_github_http_source(
    source: &str,
    rest: &str,
    ref_override: Option<&str>,
) -> Result<ParsedSource> {
    let path = rest.trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 {
        return Err(SkillenvError::InvalidSource {
            input: source.to_string(),
            message: "expected https://github.com/<owner>/<repo>".to_string(),
        });
    }

    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");
    let mut requested_ref = ref_override.map(str::to_string);
    let mut subdir = None;
    if segments.len() >= 4 && (segments[2] == "tree" || segments[2] == "blob") {
        if requested_ref.is_none() {
            requested_ref = Some(segments[3].to_string());
        }
        if segments.len() > 4 {
            subdir = Some(PathBuf::from(segments[4..].join("/")));
        }
    }

    Ok(ParsedSource {
        display_source: source.to_string(),
        kind: LockedSourceKind::Git,
        transport: format!("https://github.com/{owner}/{repo}.git"),
        requested_ref,
        subdir,
        default_name: slugify_or(repo, "source"),
    })
}

fn parse_github_shorthand_source(source: &str, ref_override: Option<&str>) -> Result<ParsedSource> {
    let (repo_spec, source_ref) = source
        .rsplit_once('@')
        .filter(|(candidate, _)| candidate.matches('/').count() == 1)
        .unwrap_or((source, ""));
    let segments: Vec<&str> = repo_spec.split('/').collect();
    if segments.len() != 2 {
        return Err(SkillenvError::InvalidSource {
            input: source.to_string(),
            message: "expected owner/repo".to_string(),
        });
    }
    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");
    let requested_ref = ref_override
        .map(str::to_string)
        .or_else(|| (!source_ref.is_empty()).then(|| source_ref.to_string()));

    Ok(ParsedSource {
        display_source: source.to_string(),
        kind: LockedSourceKind::Git,
        transport: format!("https://github.com/{owner}/{repo}.git"),
        requested_ref,
        subdir: None,
        default_name: slugify_or(repo, "source"),
    })
}

fn is_github_shorthand(source: &str) -> bool {
    !source.contains("://")
        && !source.starts_with("git@")
        && source.matches('/').count() == 1
        && source.split('/').all(|part| !part.is_empty())
}

fn looks_like_git_remote(source: &str) -> bool {
    source.starts_with("git@")
        || source.starts_with("ssh://")
        || source.ends_with(".git")
        || source.starts_with("https://")
}

fn default_name_from_remote(source: &str) -> String {
    let trimmed = source.trim_end_matches('/');
    let candidate = trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("source")
        .trim_end_matches(".git");
    slugify_or(candidate, "source")
}

fn normalize_local_source(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| normalize_path(path))
        .display()
        .to_string()
}

fn git_source_root_and_subdir(path: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    let repo_root = run_git(
        &[
            "-C".to_string(),
            path.display().to_string(),
            "rev-parse".to_string(),
            "--show-toplevel".to_string(),
        ],
        None,
    )
    .ok()?;
    let prefix = run_git(
        &[
            "-C".to_string(),
            path.display().to_string(),
            "rev-parse".to_string(),
            "--show-prefix".to_string(),
        ],
        None,
    )
    .ok()?;

    let repo_root = PathBuf::from(repo_root.trim());
    let prefix = prefix.trim().trim_end_matches('/');
    let subdir = if prefix.is_empty() {
        None
    } else {
        Some(PathBuf::from(prefix))
    };
    Some((repo_root, subdir))
}

fn combine_subdirs(left: Option<&Path>, right: Option<&Path>) -> Option<PathBuf> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.join(right)),
        (Some(left), None) => Some(left.to_path_buf()),
        (None, Some(right)) => Some(right.to_path_buf()),
        (None, None) => None,
    }
}

fn fetch_source(parsed: &ParsedSource) -> Result<FetchedSource> {
    match parsed.kind {
        LockedSourceKind::Git => fetch_git_source(
            &parsed.transport,
            parsed.requested_ref.as_deref(),
            parsed.subdir.as_deref(),
        ),
        LockedSourceKind::Local => fetch_local_source(
            &parsed.transport,
            parsed.subdir.as_deref(),
            parsed.requested_ref.as_deref(),
        ),
    }
}

fn fetch_locked_source(entry: &LockedSource) -> Result<FetchedSource> {
    match entry.kind {
        LockedSourceKind::Git => fetch_git_source(
            &entry.transport,
            Some(&entry.resolved_revision),
            entry.subdir.as_deref().map(Path::new),
        ),
        LockedSourceKind::Local => fetch_locked_local_source(entry),
    }
}

fn fetch_locked_local_source(entry: &LockedSource) -> Result<FetchedSource> {
    let transport_path = Path::new(&entry.transport);
    if entry.resolved_revision != "unversioned"
        && transport_path.exists()
        && let Some((repo_root, repo_subdir)) = git_source_root_and_subdir(transport_path)
    {
        let subdir = combine_subdirs(
            repo_subdir.as_deref(),
            entry.subdir.as_deref().map(Path::new),
        );
        return fetch_git_source(
            repo_root.to_string_lossy().as_ref(),
            Some(&entry.resolved_revision),
            subdir.as_deref(),
        );
    }

    fetch_local_source(
        &entry.transport,
        entry.subdir.as_deref().map(Path::new),
        entry.requested_ref.as_deref(),
    )
}

fn fetch_git_source(
    transport: &str,
    requested_ref: Option<&str>,
    subdir: Option<&Path>,
) -> Result<FetchedSource> {
    let tempdir = tempfile::tempdir().map_err(|source| SkillenvError::CreateDir {
        path: std::env::temp_dir(),
        source,
    })?;
    let checkout_root = tempdir.path().join("checkout");
    ensure_dir(&checkout_root)?;

    run_git(
        &["init".to_string(), checkout_root.display().to_string()],
        None,
    )?;
    run_git(
        &[
            "-C".to_string(),
            checkout_root.display().to_string(),
            "remote".to_string(),
            "add".to_string(),
            "origin".to_string(),
            transport.to_string(),
        ],
        None,
    )?;
    run_git(
        &[
            "-C".to_string(),
            checkout_root.display().to_string(),
            "fetch".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "origin".to_string(),
            requested_ref.unwrap_or("HEAD").to_string(),
        ],
        None,
    )?;
    run_git(
        &[
            "-C".to_string(),
            checkout_root.display().to_string(),
            "checkout".to_string(),
            "--detach".to_string(),
            "FETCH_HEAD".to_string(),
        ],
        None,
    )?;
    let resolved_revision = run_git(
        &[
            "-C".to_string(),
            checkout_root.display().to_string(),
            "rev-parse".to_string(),
            "HEAD".to_string(),
        ],
        None,
    )?;
    let root = resolve_subdir(&checkout_root, subdir)?;

    Ok(FetchedSource {
        _tempdir: Some(tempdir),
        root,
        resolved_revision: resolved_revision.trim().to_string(),
        versioned: true,
    })
}

fn fetch_local_source(
    transport: &str,
    subdir: Option<&Path>,
    requested_ref: Option<&str>,
) -> Result<FetchedSource> {
    let root = resolve_subdir(Path::new(transport), subdir)?;
    let versioned = is_git_repository(Path::new(transport));
    let resolved_revision = if let Some(reference) = requested_ref {
        git_revision_at(Path::new(transport), Some(reference))?
    } else if versioned {
        git_revision_at(Path::new(transport), None)?
    } else {
        "unversioned".to_string()
    };

    Ok(FetchedSource {
        _tempdir: None,
        root,
        resolved_revision,
        versioned,
    })
}

fn resolve_subdir(root: &Path, subdir: Option<&Path>) -> Result<PathBuf> {
    let path = if let Some(subdir) = subdir {
        root.join(subdir)
    } else {
        root.to_path_buf()
    };
    if path.exists() {
        Ok(path)
    } else {
        Err(SkillenvError::InvalidSource {
            input: root.display().to_string(),
            message: format!("subdir {} does not exist", path.display()),
        })
    }
}

fn is_git_repository(path: &Path) -> bool {
    git_revision_at(path, None).is_ok()
}

fn git_revision_at(path: &Path, reference: Option<&str>) -> Result<String> {
    let mut args = vec![
        "-C".to_string(),
        path.display().to_string(),
        "rev-parse".to_string(),
    ];
    args.push(reference.unwrap_or("HEAD").to_string());
    Ok(run_git(&args, None)?.trim().to_string())
}

fn install_fetched_source(
    repo_root: &Path,
    name: &str,
    install_root: &Path,
    fetched: &FetchedSource,
    selected_skills: &[String],
) -> Result<PreparedSource> {
    let source_tree = analyze_source_tree(&fetched.root)?;
    let selected = select_skills(&source_tree, &fetched.root, selected_skills)?;
    reset_managed_install_root(repo_root, name, install_root)?;
    write_managed_source_marker(install_root, name)?;

    for skill in &selected {
        let destination = match skill.scope {
            InstalledScope::Default => install_root.join("default").join(&skill.slug),
            InstalledScope::Local => install_root.join("local").join(&skill.slug),
            InstalledScope::Profile => install_root
                .join("profiles")
                .join(
                    skill
                        .source_dir
                        .parent()
                        .and_then(|parent| parent.file_name())
                        .and_then(OsStr::to_str)
                        .unwrap_or("profile"),
                )
                .join(&skill.slug),
        };
        copy_dir_all(&skill.source_dir, &destination)?;
    }

    let selected_skills = selected.into_iter().map(|skill| skill.slug).collect();
    Ok(PreparedSource {
        selected_skills,
        resolved_revision: fetched.resolved_revision.clone(),
    })
}

fn analyze_source_tree(root: &Path) -> Result<SourceTree> {
    if root.join("SKILL.md").is_file() {
        return Ok(SourceTree {
            skills: vec![TreeSkill {
                slug: slugify_or(
                    root.file_name().and_then(OsStr::to_str).unwrap_or("skill"),
                    "skill",
                ),
                source_dir: root.to_path_buf(),
                scope: InstalledScope::Default,
            }],
        });
    }

    let skills_root = root.join("skills");
    if skills_root.is_dir() {
        let skills = collect_flat_skills(&skills_root, InstalledScope::Default)?;
        if !skills.is_empty() {
            return Ok(SourceTree { skills });
        }
    }

    if has_skillenv_layout(root) {
        return Ok(SourceTree {
            skills: collect_skillenv_skills(root)?,
        });
    }

    let nested_root = root.join("skillenv");
    if has_skillenv_layout(&nested_root) {
        return Ok(SourceTree {
            skills: collect_skillenv_skills(&nested_root)?,
        });
    }

    Err(SkillenvError::InvalidSource {
        input: root.display().to_string(),
        message: "no supported skill layout found".to_string(),
    })
}

fn has_skillenv_layout(root: &Path) -> bool {
    root.join("default").is_dir() || root.join("local").is_dir() || root.join("profiles").is_dir()
}

fn collect_flat_skills(root: &Path, scope: InstalledScope) -> Result<Vec<TreeSkill>> {
    let mut skills = Vec::new();
    for entry in fs::read_dir(root).map_err(|source| SkillenvError::ReadFile {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| SkillenvError::ReadFile {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            skills.push(TreeSkill {
                slug: slugify_or(
                    path.file_name().and_then(OsStr::to_str).unwrap_or("skill"),
                    "skill",
                ),
                source_dir: path,
                scope,
            });
        }
    }
    Ok(skills)
}

fn collect_skillenv_skills(root: &Path) -> Result<Vec<TreeSkill>> {
    let mut skills = Vec::new();
    skills.extend(collect_flat_skills(
        &root.join("default"),
        InstalledScope::Default,
    )?);
    skills.extend(collect_flat_skills(
        &root.join("local"),
        InstalledScope::Local,
    )?);

    let profiles_root = root.join("profiles");
    if profiles_root.is_dir() {
        for entry in fs::read_dir(&profiles_root).map_err(|source| SkillenvError::ReadFile {
            path: profiles_root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| SkillenvError::ReadFile {
                path: profiles_root.clone(),
                source,
            })?;
            let profile_path = entry.path();
            if !profile_path.is_dir() {
                continue;
            }
            skills.extend(collect_flat_skills(&profile_path, InstalledScope::Profile)?);
        }
    }

    Ok(skills)
}

fn select_skills(
    source_tree: &SourceTree,
    source_root: &Path,
    selected_skills: &[String],
) -> Result<Vec<TreeSkill>> {
    if selected_skills.is_empty() {
        return Ok(source_tree.skills.clone());
    }

    let requested: BTreeSet<_> = selected_skills.iter().cloned().collect();
    let available: BTreeMap<_, _> = source_tree
        .skills
        .iter()
        .map(|skill| (skill.slug.clone(), skill.clone()))
        .collect();
    let mut selected = Vec::new();
    for name in &requested {
        if let Some(skill) = available.get(name) {
            selected.push(skill.clone());
        } else {
            return Err(SkillenvError::InvalidSource {
                input: source_root.display().to_string(),
                message: format!("skill '{name}' not found"),
            });
        }
    }
    Ok(selected)
}

fn reset_managed_install_root(repo_root: &Path, name: &str, install_root: &Path) -> Result<()> {
    if install_root.exists() {
        remove_managed_install_root(install_root, name)?;
    } else if let Some(parent) = install_root.parent() {
        ensure_dir(parent)?;
    }

    if repo_root == install_root {
        return Err(SkillenvError::ManagedSourceCollision {
            path: install_root.to_path_buf(),
        });
    }

    ensure_dir(install_root)
}

fn ensure_install_root_available(
    occupied_install_roots: &[(String, PathBuf)],
    name: &str,
    install_root: &Path,
) -> Result<()> {
    if let Some(existing) = occupied_install_roots
        .iter()
        .find(|(_, path)| path == install_root)
        && existing.0 == name
    {
        return Ok(());
    }

    if !install_root.exists() {
        return Ok(());
    }

    if managed_source_marker_name(install_root)?.as_deref() == Some(name) {
        return Ok(());
    }

    Err(SkillenvError::ManagedSourceCollision {
        path: install_root.to_path_buf(),
    })
}

fn occupied_install_roots(repo_root: &Path, lock_file: &LockFile) -> Vec<(String, PathBuf)> {
    lock_file
        .sources
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                resolve_stored_path(repo_root, &entry.install_root),
            )
        })
        .collect()
}

fn remove_managed_install_root(install_root: &Path, name: &str) -> Result<()> {
    if !install_root.exists() {
        return Ok(());
    }

    if managed_source_marker_name(install_root)?.as_deref() != Some(name) {
        return Err(SkillenvError::ManagedSourceCollision {
            path: install_root.to_path_buf(),
        });
    }

    fs::remove_dir_all(install_root).map_err(|source| SkillenvError::WriteFile {
        path: install_root.to_path_buf(),
        source,
    })
}

fn write_managed_source_marker(install_root: &Path, name: &str) -> Result<()> {
    let marker_path = install_root.join(MANAGED_SOURCE_MARKER_FILE);
    let marker = ManagedSourceMarker {
        name: name.to_string(),
    };
    let body =
        serde_json::to_string_pretty(&marker).map_err(|source| SkillenvError::SerializeLock {
            path: marker_path.clone(),
            source,
        })?;
    fs::write(&marker_path, body).map_err(|source| SkillenvError::WriteFile {
        path: marker_path,
        source,
    })
}

fn managed_source_marker_name(install_root: &Path) -> Result<Option<String>> {
    let marker_path = install_root.join(MANAGED_SOURCE_MARKER_FILE);
    if !marker_path.is_file() {
        return Ok(None);
    }
    let body = fs::read_to_string(&marker_path).map_err(|source| SkillenvError::ReadFile {
        path: marker_path.clone(),
        source,
    })?;
    let marker: ManagedSourceMarker =
        serde_json::from_str(&body).map_err(|source| SkillenvError::ParseLock {
            path: marker_path,
            source,
        })?;
    Ok(Some(marker.name))
}

fn validate_requested_names(lock_file: &LockFile, requested_names: &[String]) -> Result<()> {
    for name in requested_names {
        if !lock_file.sources.iter().any(|entry| &entry.name == name) {
            return Err(SkillenvError::UnknownManagedSource { name: name.clone() });
        }
    }
    Ok(())
}

fn parsed_from_lock_entry(entry: &LockedSource) -> ParsedSource {
    ParsedSource {
        display_source: entry.source.clone(),
        kind: entry.kind,
        transport: entry.transport.clone(),
        requested_ref: entry.requested_ref.clone(),
        subdir: entry.subdir.as_ref().map(PathBuf::from),
        default_name: entry.name.clone(),
    }
}

fn normalize_selected_skills(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| slugify_or(value, "skill"))
        .collect()
}

fn resolve_install_root(repo_root: &Path, name: &str, install_root: Option<&Path>) -> PathBuf {
    install_root
        .map(|path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                repo_root.join(path)
            }
        })
        .unwrap_or_else(|| repo_root.join("skillenv/remote").join(name))
}

fn upsert_lock_entry(lock_file: &mut LockFile, entry: LockedSource) {
    if lock_file.version == 0 {
        lock_file.version = LOCK_FILE_VERSION;
    }

    if let Some(slot) = lock_file
        .sources
        .iter_mut()
        .find(|candidate| candidate.name == entry.name)
    {
        *slot = entry;
    } else {
        lock_file.sources.push(entry);
        lock_file
            .sources
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

fn load_lock_file(repo_root: &Path) -> Result<LockFile> {
    let lock_path = repo_root.join(LOCK_FILE_NAME);
    if !lock_path.exists() {
        return Ok(LockFile {
            version: LOCK_FILE_VERSION,
            sources: Vec::new(),
        });
    }

    let body = fs::read_to_string(&lock_path).map_err(|source| SkillenvError::ReadFile {
        path: lock_path.clone(),
        source,
    })?;
    let mut lock_file: LockFile =
        serde_json::from_str(&body).map_err(|source| SkillenvError::ParseLock {
            path: lock_path,
            source,
        })?;
    if lock_file.version == 0 {
        lock_file.version = LOCK_FILE_VERSION;
    }
    Ok(lock_file)
}

fn save_lock_file(repo_root: &Path, lock_file: &LockFile) -> Result<()> {
    let lock_path = repo_root.join(LOCK_FILE_NAME);
    let body =
        serde_json::to_string_pretty(lock_file).map_err(|source| SkillenvError::SerializeLock {
            path: lock_path.clone(),
            source,
        })?;
    fs::write(&lock_path, body).map_err(|source| SkillenvError::WriteFile {
        path: lock_path,
        source,
    })
}

fn store_path(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn resolve_stored_path(repo_root: &Path, stored: &str) -> PathBuf {
    let path = PathBuf::from(stored);
    if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    }
}

fn copy_dir_all(source_dir: &Path, target_dir: &Path) -> Result<()> {
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

fn run_git(args: &[String], cwd: Option<&Path>) -> Result<String> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|source| SkillenvError::RunCommand {
            program: "git".to_string(),
            cwd: cwd.map(Path::to_path_buf),
            source,
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(SkillenvError::CommandFailed {
            program: "git".to_string(),
            cwd: cwd.map(Path::to_path_buf),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InitOptions, init_repo};

    #[test]
    fn add_source_installs_selected_skills_and_writes_lock() -> Result<()> {
        let repo = repo_fixture()?;
        let upstream = git_fixture("agent-skills")?;
        write_skill(upstream.path(), "skills/frontend-design", "frontend")?;
        write_skill(upstream.path(), "skills/testing", "testing")?;
        commit_all(upstream.path(), "initial")?;
        init_repo(repo.path(), InitOptions::default())?;

        let report = add_source(
            repo.path(),
            AddSourceOptions {
                source: upstream.path().display().to_string(),
                into: None,
                skills: vec!["frontend-design".to_string()],
                ref_name: None,
                name: Some("vercel".to_string()),
                claude: TargetOverride::UseConfig,
            },
        )?;

        assert_eq!(report.selected_skills, vec!["frontend-design"]);
        assert!(
            repo.path()
                .join("skillenv/remote/vercel/default/frontend-design/SKILL.md")
                .is_file()
        );
        assert!(
            !repo
                .path()
                .join("skillenv/remote/vercel/default/testing")
                .exists()
        );

        let lock_body = fs::read_to_string(repo.path().join(LOCK_FILE_NAME)).unwrap();
        assert!(lock_body.contains("\"name\": \"vercel\""));
        assert!(lock_body.contains("\"resolved_revision\""));
        assert!(
            repo.path()
                .join(".agents/skills")
                .read_dir()
                .unwrap()
                .next()
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn add_source_supports_direct_skill_directory() -> Result<()> {
        let repo = repo_fixture()?;
        let upstream = git_fixture("single-skill")?;
        write_skill(upstream.path(), "web-design-guidelines", "skill body")?;
        commit_all(upstream.path(), "initial")?;
        init_repo(repo.path(), InitOptions::default())?;

        let report = add_source(
            repo.path(),
            AddSourceOptions {
                source: upstream
                    .path()
                    .join("web-design-guidelines")
                    .display()
                    .to_string(),
                into: Some(PathBuf::from("vendor/ui")),
                skills: Vec::new(),
                ref_name: None,
                name: None,
                claude: TargetOverride::UseConfig,
            },
        )?;

        assert_eq!(report.selected_skills, vec!["web-design-guidelines"]);
        assert!(
            repo.path()
                .join("vendor/ui/default/web-design-guidelines/SKILL.md")
                .is_file()
        );
        Ok(())
    }

    #[test]
    fn update_sources_skips_unchanged_and_reinstalls_changed_source() -> Result<()> {
        let repo = repo_fixture()?;
        let upstream = git_fixture("agent-skills")?;
        write_skill(upstream.path(), "skills/frontend-design", "v1")?;
        commit_all(upstream.path(), "initial")?;
        init_repo(repo.path(), InitOptions::default())?;

        let first = add_source(
            repo.path(),
            AddSourceOptions {
                source: upstream.path().display().to_string(),
                into: None,
                skills: Vec::new(),
                ref_name: None,
                name: Some("shared".to_string()),
                claude: TargetOverride::UseConfig,
            },
        )?;

        let unchanged = update_sources(
            repo.path(),
            UpdateSourcesOptions {
                names: vec!["shared".to_string()],
                claude: TargetOverride::UseConfig,
            },
        )?;
        assert_eq!(unchanged.updated.len(), 0);
        assert_eq!(unchanged.unchanged.len(), 1);

        fs::write(
            upstream.path().join("skills/frontend-design/SKILL.md"),
            "v2\n",
        )
        .unwrap();
        commit_all(upstream.path(), "update")?;

        let changed = update_sources(
            repo.path(),
            UpdateSourcesOptions {
                names: vec!["shared".to_string()],
                claude: TargetOverride::UseConfig,
            },
        )?;
        assert_eq!(changed.updated.len(), 1);
        assert_ne!(changed.updated[0].new_revision, first.resolved_revision);
        assert!(
            fs::read_to_string(
                repo.path()
                    .join("skillenv/remote/shared/default/frontend-design/SKILL.md")
            )
            .unwrap()
            .contains("v2")
        );
        Ok(())
    }

    #[test]
    fn fetch_sources_restores_missing_install_root_at_locked_revision() -> Result<()> {
        let repo = repo_fixture()?;
        let upstream = git_fixture("agent-skills")?;
        write_skill(upstream.path(), "skills/frontend-design", "v1")?;
        commit_all(upstream.path(), "initial")?;
        init_repo(repo.path(), InitOptions::default())?;

        let first = add_source(
            repo.path(),
            AddSourceOptions {
                source: upstream.path().display().to_string(),
                into: None,
                skills: Vec::new(),
                ref_name: None,
                name: Some("shared".to_string()),
                claude: TargetOverride::UseConfig,
            },
        )?;
        let lock_before = fs::read_to_string(repo.path().join(LOCK_FILE_NAME)).unwrap();

        fs::write(
            upstream.path().join("skills/frontend-design/SKILL.md"),
            "v2\n",
        )
        .unwrap();
        commit_all(upstream.path(), "update")?;
        remove_managed_install_root(&repo.path().join("skillenv/remote/shared"), "shared")?;

        let fetched = fetch_sources(
            repo.path(),
            FetchSourcesOptions {
                names: vec!["shared".to_string()],
                claude: TargetOverride::UseConfig,
            },
        )?;

        assert_eq!(fetched.fetched.len(), 1);
        assert_eq!(
            fetched.fetched[0].resolved_revision,
            first.resolved_revision
        );
        assert!(
            fs::read_to_string(
                repo.path()
                    .join("skillenv/remote/shared/default/frontend-design/SKILL.md")
            )
            .unwrap()
            .contains("v1")
        );
        assert_eq!(
            fs::read_to_string(repo.path().join(LOCK_FILE_NAME)).unwrap(),
            lock_before
        );
        Ok(())
    }

    #[test]
    fn fetch_sources_restores_direct_skill_directory_from_git_subdir() -> Result<()> {
        let repo = repo_fixture()?;
        let upstream = git_fixture("single-skill")?;
        write_skill(upstream.path(), "web-design-guidelines", "v1")?;
        commit_all(upstream.path(), "initial")?;
        init_repo(repo.path(), InitOptions::default())?;

        let skill_dir = upstream.path().join("web-design-guidelines");
        let first = add_source(
            repo.path(),
            AddSourceOptions {
                source: skill_dir.display().to_string(),
                into: Some(PathBuf::from("vendor/ui")),
                skills: Vec::new(),
                ref_name: None,
                name: None,
                claude: TargetOverride::UseConfig,
            },
        )?;

        fs::write(skill_dir.join("SKILL.md"), "v2\n").unwrap();
        commit_all(upstream.path(), "update")?;
        remove_managed_install_root(&repo.path().join("vendor/ui"), "web-design-guidelines")?;

        let fetched = fetch_sources(
            repo.path(),
            FetchSourcesOptions {
                names: vec!["web-design-guidelines".to_string()],
                claude: TargetOverride::UseConfig,
            },
        )?;

        assert_eq!(fetched.fetched.len(), 1);
        assert_eq!(
            fetched.fetched[0].resolved_revision,
            first.resolved_revision
        );
        assert!(
            fs::read_to_string(
                repo.path()
                    .join("vendor/ui/default/web-design-guidelines/SKILL.md")
            )
            .unwrap()
            .contains("v1")
        );
        Ok(())
    }

    #[test]
    fn add_source_requires_initialized_repo() -> Result<()> {
        let repo = repo_fixture()?;
        let upstream = git_fixture("agent-skills")?;
        write_skill(upstream.path(), "skills/frontend-design", "frontend")?;
        commit_all(upstream.path(), "initial")?;

        let error = add_source(
            repo.path(),
            AddSourceOptions {
                source: upstream.path().display().to_string(),
                into: None,
                skills: Vec::new(),
                ref_name: None,
                name: None,
                claude: TargetOverride::UseConfig,
            },
        )
        .unwrap_err();

        assert!(matches!(error, SkillenvError::RepoNotInitialized));
        Ok(())
    }

    /// A repository fixture that also owns a private `HOME`.
    ///
    /// These tests reach `load_config`, which reads `HOME`, so they must hold the
    /// same guard as the tests that redirect it. Without this the suite passed
    /// serially and failed about two runs in three in parallel: one test would
    /// point `HOME` at its own temporary directory while another read that value
    /// and loaded a configuration that was not its own.
    struct RepoFixture {
        dir: TempDir,
        _home_dir: TempDir,
        _home: crate::test_support::HomeEnvGuard,
    }

    impl RepoFixture {
        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    fn repo_fixture() -> Result<RepoFixture> {
        let home_dir = TempDir::new().unwrap();
        let home = crate::test_support::set_home_for_test(Some(home_dir.path()));
        let dir = TempDir::new().unwrap();
        ensure_dir(&dir.path().join(".git"))?;
        Ok(RepoFixture {
            dir,
            _home_dir: home_dir,
            _home: home,
        })
    }

    fn git_fixture(name: &str) -> Result<TempDir> {
        let dir = TempDir::new().unwrap();
        run_git(
            &["init".to_string(), dir.path().display().to_string()],
            None,
        )?;
        run_git(
            &[
                "-C".to_string(),
                dir.path().display().to_string(),
                "config".to_string(),
                "user.email".to_string(),
                "skillenv@example.com".to_string(),
            ],
            None,
        )?;
        run_git(
            &[
                "-C".to_string(),
                dir.path().display().to_string(),
                "config".to_string(),
                "user.name".to_string(),
                "skillenv".to_string(),
            ],
            None,
        )?;
        let source_root = dir.path().join(name);
        ensure_dir(&source_root)?;
        Ok(dir)
    }

    fn write_skill(repo_root: &Path, relative: &str, body: &str) -> Result<()> {
        let skill_dir = repo_root.join(relative);
        ensure_dir(&skill_dir)?;
        fs::write(skill_dir.join("SKILL.md"), body).map_err(|source| SkillenvError::WriteFile {
            path: skill_dir.join("SKILL.md"),
            source,
        })
    }

    fn commit_all(repo_root: &Path, message: &str) -> Result<()> {
        run_git(
            &[
                "-C".to_string(),
                repo_root.display().to_string(),
                "add".to_string(),
                ".".to_string(),
            ],
            None,
        )?;
        run_git(
            &[
                "-C".to_string(),
                repo_root.display().to_string(),
                "commit".to_string(),
                "-m".to_string(),
                message.to_string(),
            ],
            None,
        )?;
        Ok(())
    }
}
