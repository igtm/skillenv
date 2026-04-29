use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use thiserror::Error;
use walkdir::WalkDir;

mod remote;

const GENERATED_MARKER_FILE: &str = ".skillenv-generated.json";
const REPO_LAYOUT_DIR: &str = "skillenv";
const DEFAULT_SCOPE_DIR: &str = "default";
const LOCAL_SCOPE_DIR: &str = "local";
const PROFILES_SCOPE_DIR: &str = "profiles";

pub type Result<T> = std::result::Result<T, SkillenvError>;

pub use remote::{
    AddSourceOptions, AddSourceReport, UpdateSourcesOptions, UpdateSourcesReport, add_source,
    format_add_source_report, format_update_sources_report, update_sources,
};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub targets: TargetsConfig,
    // Legacy no-op field retained for backward-compatible config parsing.
    pub gitignore: GitignoreConfig,
    pub defaults: DefaultsConfig,
    pub external_sources: Vec<ExternalSourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TargetsConfig {
    pub agents: bool,
    pub claude: bool,
}

impl Default for TargetsConfig {
    fn default() -> Self {
        Self {
            agents: true,
            claude: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
// Legacy no-op config retained for backward-compatible parsing.
pub struct GitignoreConfig {
    pub auto_update: bool,
}

impl Default for GitignoreConfig {
    fn default() -> Self {
        Self { auto_update: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DefaultsConfig {
    pub strategy: Strategy,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            strategy: Strategy::Render,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalSourceConfig {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Render,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeSelector {
    DefaultLocal,
    Profiles(Vec<String>),
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetOverride {
    #[default]
    UseConfig,
    ForceEnabled,
    ForceDisabled,
}

#[derive(Debug, Clone)]
pub struct LinkOptions {
    pub selector: ScopeSelector,
    pub claude: TargetOverride,
    pub quiet: bool,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            selector: ScopeSelector::DefaultLocal,
            claude: TargetOverride::UseConfig,
            quiet: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnlinkOptions {
    pub selector: ScopeSelector,
    pub claude: TargetOverride,
    pub quiet: bool,
}

impl Default for UnlinkOptions {
    fn default() -> Self {
        Self {
            selector: ScopeSelector::DefaultLocal,
            claude: TargetOverride::UseConfig,
            quiet: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusOptions {
    pub claude: TargetOverride,
}

#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub claude: TargetOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub repo_root: Option<PathBuf>,
    pub repo_slug: Option<String>,
    pub strategy: Option<Strategy>,
    pub touched_scopes: Vec<String>,
    pub target_reports: Vec<TargetReport>,
    pub gitignore_updated: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TargetReport {
    pub kind: TargetKind,
    pub path: Option<PathBuf>,
    pub linked: usize,
    pub removed: usize,
}

#[derive(Debug, Clone)]
pub struct StatusReport {
    pub repo_root: Option<PathBuf>,
    pub repo_slug: Option<String>,
    pub target_statuses: Vec<TargetStatus>,
}

#[derive(Debug, Clone)]
pub struct InitReport {
    pub repo_root: PathBuf,
    pub created_dirs: Vec<PathBuf>,
    pub gitignore_updated: bool,
}

#[derive(Debug, Clone)]
pub struct TargetStatus {
    pub kind: TargetKind,
    pub path: Option<PathBuf>,
    pub state: LinkState,
    pub managed_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Linked,
    NotLinked,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Agents,
    Claude,
}

impl TargetKind {
    fn label(self) -> &'static str {
        match self {
            Self::Agents => ".agents/skills",
            Self::Claude => ".claude/skills",
        }
    }
}

#[derive(Debug, Error)]
pub enum SkillenvError {
    #[error("failed to read {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("failed to create directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("invalid config at {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid frontmatter in {path}: {source}")]
    ParseFrontmatter {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("invalid metadata field in {path}: expected a mapping")]
    InvalidMetadataField { path: PathBuf },
    #[error("invalid lock file at {path}: {source}")]
    ParseLock {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to serialize lock file at {path}: {source}")]
    SerializeLock {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("duplicate skill '{skill}' in scope '{scope}' from '{first}' and '{second}'")]
    DuplicateSkill {
        scope: String,
        skill: String,
        first: String,
        second: String,
    },
    #[error("repo root not detected; this command requires a git repository")]
    RepoRequired,
    #[error("HOME is not set; global skill targets require a home directory")]
    HomeNotSet,
    #[error(
        "repo is not initialized for skillenv outputs; run `skillenv init` with the desired target flags first"
    )]
    RepoNotInitialized,
    #[error("invalid source '{input}': {message}")]
    InvalidSource { input: String, message: String },
    #[error("refusing to overwrite unmanaged target {path}")]
    TargetCollision { path: PathBuf },
    #[error("managed source collision at {path}")]
    ManagedSourceCollision { path: PathBuf },
    #[error("unknown managed source '{name}'")]
    UnknownManagedSource { name: String },
    #[error("failed to serialize marker for {path}: {source}")]
    SerializeMarker {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to run {program}: {source}")]
    RunCommand {
        program: String,
        cwd: Option<PathBuf>,
        source: io::Error,
    },
    #[error("command {program} failed in {cwd:?}: {stderr}")]
    CommandFailed {
        program: String,
        cwd: Option<PathBuf>,
        stderr: String,
    },
    #[error("unsupported shell '{0}'")]
    UnsupportedShell(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ScopeKey {
    Default,
    Local,
    Profile(String),
}

impl ScopeKey {
    fn selector_names(selector: &ScopeSelector) -> Vec<Self> {
        match selector {
            ScopeSelector::DefaultLocal => vec![Self::Default, Self::Local],
            ScopeSelector::Profiles(names) => names
                .iter()
                .map(|name| Self::Profile(slugify_or(name, "profile")))
                .collect(),
            ScopeSelector::All => Vec::new(),
        }
    }

    fn display_name(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Local => "local".to_string(),
            Self::Profile(name) => format!("profile:{name}"),
        }
    }

    fn context_path(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Local => "local".to_string(),
            Self::Profile(name) => format!("profiles/{name}"),
        }
    }

    fn generated_segment(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Local => "local".to_string(),
            Self::Profile(name) => format!("profile-{name}"),
        }
    }
}

#[derive(Debug, Clone)]
struct Discovery {
    sources: BTreeMap<ScopeKey, Vec<SkillSource>>,
    known_source_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct SkillSource {
    scope_origin: PathBuf,
    skill_slug: String,
    dir: PathBuf,
    #[cfg_attr(not(test), allow(dead_code))]
    origin_label: String,
}

#[derive(Debug, Clone)]
struct TargetSpec {
    kind: TargetKind,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetRootMode {
    RepoLocal,
    Global,
}

#[derive(Debug, Clone)]
struct GeneratedNameLayout {
    repo_slug: String,
    prefix: String,
}

impl GeneratedNameLayout {
    fn for_mode(repo_root: &Path, mode: TargetRootMode) -> Self {
        match mode {
            TargetRootMode::RepoLocal => {
                let repo_slug = repo_slug(repo_root);
                let prefix = format!("skillenv-{repo_slug}-");
                Self { repo_slug, prefix }
            }
            TargetRootMode::Global => {
                let stable_root = stable_global_repo_root(repo_root);
                let repo_slug = repo_slug(&stable_root);
                let hash = short_path_digest(&stable_root);
                let prefix = format!("skillenv-{repo_slug}-g{hash}-");
                Self { repo_slug, prefix }
            }
        }
    }

    fn generated_name(&self, scope: &ScopeKey, skill_slug: &str) -> String {
        format!("{}{}-{skill_slug}", self.prefix, scope.generated_segment())
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    fn scope_prefix(&self, scope: &str) -> String {
        format!("{}{}-", self.prefix, scope_segment_from_display(scope))
    }
}

#[derive(Debug, Clone)]
struct LoadedConfig {
    config: Config,
    base_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeneratedMarker {
    repo: String,
    scope: String,
    skill: String,
    generated_name: String,
    source: String,
    strategy: Strategy,
}

#[derive(Debug, Clone)]
enum ScopeFilter {
    AllCurrentRepo,
    Exact(BTreeSet<String>),
}

pub fn link_repo(cwd: impl AsRef<Path>, options: LinkOptions) -> Result<Report> {
    link_with_config(cwd.as_ref(), &options, None, TargetRootMode::RepoLocal)
}

pub fn link_global(cwd: impl AsRef<Path>, options: LinkOptions) -> Result<Report> {
    link_with_config(cwd.as_ref(), &options, None, TargetRootMode::Global)
}

pub fn unlink_repo(cwd: impl AsRef<Path>, options: UnlinkOptions) -> Result<Report> {
    unlink_with_config(cwd.as_ref(), &options, None, TargetRootMode::RepoLocal)
}

pub fn unlink_global(cwd: impl AsRef<Path>, options: UnlinkOptions) -> Result<Report> {
    unlink_with_config(cwd.as_ref(), &options, None, TargetRootMode::Global)
}

pub fn status_repo(cwd: impl AsRef<Path>, options: StatusOptions) -> Result<StatusReport> {
    status_with_config(cwd.as_ref(), &options, None, TargetRootMode::RepoLocal)
}

pub fn status_global(cwd: impl AsRef<Path>, options: StatusOptions) -> Result<StatusReport> {
    status_with_config(cwd.as_ref(), &options, None, TargetRootMode::Global)
}

pub fn init_repo(cwd: impl AsRef<Path>, options: InitOptions) -> Result<InitReport> {
    init_repo_with_config(cwd.as_ref(), &options, None)
}

pub fn hook_script(shell: Shell) -> String {
    match shell {
        Shell::Zsh => r#"autoload -Uz add-zsh-hook
_skillenv_chpwd() {
  command skillenv link --quiet
}
add-zsh-hook chpwd _skillenv_chpwd
_skillenv_chpwd
"#
        .to_string(),
        Shell::Bash => r#"_skillenv_last_repo_root=""
_skillenv_prompt_hook() {
  local current_repo_root
  current_repo_root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
  if [ "$current_repo_root" = "$_skillenv_last_repo_root" ]; then
    return
  fi
  _skillenv_last_repo_root="$current_repo_root"
  command skillenv link --quiet
}
case ";${PROMPT_COMMAND};" in
  *";_skillenv_prompt_hook;"*) ;;
  *)
    if [ -n "${PROMPT_COMMAND:-}" ]; then
      PROMPT_COMMAND="_skillenv_prompt_hook;${PROMPT_COMMAND}"
    else
      PROMPT_COMMAND="_skillenv_prompt_hook"
    fi
    ;;
esac
"#
        .to_string(),
    }
}

pub fn format_link_report(report: &Report, action: &str) -> String {
    if let Some(message) = &report.message {
        return message.clone();
    }

    let mut lines = Vec::new();
    if let Some(repo_root) = &report.repo_root {
        lines.push(format!("repo root: {}", repo_root.display()));
    }

    for target in &report.target_reports {
        let path = target
            .path
            .as_ref()
            .map(|value| value.display().to_string())
            .unwrap_or_else(|| target.kind.label().to_string());
        lines.push(format!(
            "{action} {} skill(s) in {} ({} removed)",
            target.linked, path, target.removed
        ));
    }

    if report.gitignore_updated {
        lines.push(".gitignore updated".to_string());
    }

    lines.join("\n")
}

pub fn format_status_report(report: &StatusReport) -> String {
    let mut lines = Vec::new();
    match &report.repo_root {
        Some(repo_root) => lines.push(format!("repo root: {}", repo_root.display())),
        None => lines.push("repo root: not detected".to_string()),
    }

    for status in &report.target_statuses {
        let label = status
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| status.kind.label().to_string());
        let state = match status.state {
            LinkState::Linked => "linked",
            LinkState::NotLinked => "not linked",
            LinkState::Disabled => "disabled",
        };
        if matches!(status.state, LinkState::Disabled) {
            lines.push(format!("{label}: {state}"));
        } else {
            lines.push(format!(
                "{label}: {state} ({} managed skill(s))",
                status.managed_count
            ));
        }
    }

    lines.join("\n")
}

pub fn format_init_report(report: &InitReport) -> String {
    let mut lines = vec![format!("repo root: {}", report.repo_root.display())];

    if report.created_dirs.is_empty() {
        lines.push("skillenv layout already present".to_string());
    } else {
        for dir in &report.created_dirs {
            let display = dir
                .strip_prefix(&report.repo_root)
                .unwrap_or(dir)
                .display()
                .to_string();
            lines.push(format!("created {display}"));
        }
    }

    if report.gitignore_updated {
        lines.push(".gitignore updated".to_string());
    } else {
        lines.push(".gitignore already up to date".to_string());
    }

    lines.join("\n")
}

fn init_repo_with_config(
    cwd: &Path,
    options: &InitOptions,
    config_override: Option<&Path>,
) -> Result<InitReport> {
    let loaded = load_config(config_override)?;
    let config = &loaded.config;
    let repo_root = detect_repo_root(cwd).ok_or(SkillenvError::RepoRequired)?;

    let layout_root = repo_root.join(REPO_LAYOUT_DIR);
    ensure_dir(&layout_root)?;

    let mut created_dirs = Vec::new();
    for scope_dir in [DEFAULT_SCOPE_DIR, LOCAL_SCOPE_DIR, PROFILES_SCOPE_DIR] {
        ensure_layout_dir(&layout_root.join(scope_dir), &mut created_dirs)?;
    }

    let gitignore_updated =
        update_gitignore(&repo_root, include_claude_target(config, options.claude))?;

    Ok(InitReport {
        repo_root,
        created_dirs,
        gitignore_updated,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn link_repo_with_config(
    cwd: &Path,
    options: &LinkOptions,
    config_override: Option<&Path>,
) -> Result<Report> {
    link_with_config(cwd, options, config_override, TargetRootMode::RepoLocal)
}

fn link_with_config(
    cwd: &Path,
    options: &LinkOptions,
    config_override: Option<&Path>,
    mode: TargetRootMode,
) -> Result<Report> {
    let loaded = load_config(config_override)?;
    let config = &loaded.config;
    let repo_root = detect_repo_root(cwd);
    let Some(repo_root) = repo_root else {
        return match mode {
            TargetRootMode::RepoLocal => Ok(Report {
                repo_root: None,
                repo_slug: None,
                strategy: None,
                touched_scopes: Vec::new(),
                target_reports: Vec::new(),
                gitignore_updated: false,
                message: if options.quiet {
                    None
                } else {
                    Some("repo root not detected; nothing linked".to_string())
                },
            }),
            TargetRootMode::Global => Err(SkillenvError::RepoRequired),
        };
    };

    let generated_names = GeneratedNameLayout::for_mode(&repo_root, mode);
    let discovery_root = source_repo_root(mode, &repo_root);
    if matches!(mode, TargetRootMode::RepoLocal) {
        match require_repo_initialized(&repo_root, include_claude_target(config, options.claude)) {
            Ok(()) => {}
            Err(SkillenvError::RepoNotInitialized) if options.quiet => {
                return Ok(Report {
                    repo_root: Some(repo_root),
                    repo_slug: Some(generated_names.repo_slug.clone()),
                    strategy: Some(config.defaults.strategy),
                    touched_scopes: Vec::new(),
                    target_reports: Vec::new(),
                    gitignore_updated: false,
                    message: None,
                });
            }
            Err(error) => return Err(error),
        }
    }

    let discovery = discover_sources(
        &discovery_root,
        &generated_names.repo_slug,
        config,
        loaded.base_dir.as_deref(),
    )?;
    let desired_sources = desired_sources(&discovery, &options.selector);
    let targets = resolve_targets(mode, &repo_root, config, options.claude)?;
    let filter = removal_filter(&options.selector);
    let touched_scopes = touched_scope_names(&options.selector, &desired_sources);

    let mut target_reports = Vec::new();
    for target in &targets {
        ensure_dir(&target.path)?;
        let removed = remove_managed_entries(
            &target.path,
            &generated_names,
            &filter,
            &discovery.known_source_roots,
        )?;
        let linked = reconcile_target(
            &target.path,
            &generated_names,
            config.defaults.strategy,
            &desired_sources,
        )?;
        target_reports.push(TargetReport {
            kind: target.kind,
            path: Some(target.path.clone()),
            linked,
            removed,
        });
    }

    Ok(Report {
        repo_root: Some(repo_root),
        repo_slug: Some(generated_names.repo_slug),
        strategy: Some(config.defaults.strategy),
        touched_scopes,
        target_reports,
        gitignore_updated: false,
        message: None,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn unlink_repo_with_config(
    cwd: &Path,
    options: &UnlinkOptions,
    config_override: Option<&Path>,
) -> Result<Report> {
    unlink_with_config(cwd, options, config_override, TargetRootMode::RepoLocal)
}

fn unlink_with_config(
    cwd: &Path,
    options: &UnlinkOptions,
    config_override: Option<&Path>,
    mode: TargetRootMode,
) -> Result<Report> {
    let loaded = load_config(config_override)?;
    let config = &loaded.config;
    let repo_root = detect_repo_root(cwd);
    let Some(repo_root) = repo_root else {
        return match mode {
            TargetRootMode::RepoLocal => Ok(Report {
                repo_root: None,
                repo_slug: None,
                strategy: None,
                touched_scopes: Vec::new(),
                target_reports: Vec::new(),
                gitignore_updated: false,
                message: if options.quiet {
                    None
                } else {
                    Some("repo root not detected; nothing unlinked".to_string())
                },
            }),
            TargetRootMode::Global => Err(SkillenvError::RepoRequired),
        };
    };

    let generated_names = GeneratedNameLayout::for_mode(&repo_root, mode);
    let discovery_root = source_repo_root(mode, &repo_root);
    let targets = resolve_targets(mode, &repo_root, config, options.claude)?;
    let filter = removal_filter(&options.selector);
    let touched_scopes = touched_scope_names(&options.selector, &BTreeMap::new());
    let known_source_roots = known_source_roots(
        &discovery_root,
        &generated_names.repo_slug,
        config,
        loaded.base_dir.as_deref(),
    )?;

    let mut target_reports = Vec::new();
    for target in &targets {
        let removed = if target.path.exists() {
            remove_managed_entries(&target.path, &generated_names, &filter, &known_source_roots)?
        } else {
            0
        };
        target_reports.push(TargetReport {
            kind: target.kind,
            path: Some(target.path.clone()),
            linked: 0,
            removed,
        });
    }

    Ok(Report {
        repo_root: Some(repo_root),
        repo_slug: Some(generated_names.repo_slug),
        strategy: Some(config.defaults.strategy),
        touched_scopes,
        target_reports,
        gitignore_updated: false,
        message: None,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn status_repo_with_config(
    cwd: &Path,
    options: &StatusOptions,
    config_override: Option<&Path>,
) -> Result<StatusReport> {
    status_with_config(cwd, options, config_override, TargetRootMode::RepoLocal)
}

fn status_with_config(
    cwd: &Path,
    options: &StatusOptions,
    config_override: Option<&Path>,
    mode: TargetRootMode,
) -> Result<StatusReport> {
    let loaded = load_config(config_override)?;
    let config = &loaded.config;
    let enabled = resolve_target_kinds(config, options.claude);
    let repo_root = detect_repo_root(cwd);
    let Some(repo_root) = repo_root else {
        return match mode {
            TargetRootMode::RepoLocal => Ok(StatusReport {
                repo_root: None,
                repo_slug: None,
                target_statuses: all_target_statuses(None, &enabled),
            }),
            TargetRootMode::Global => Err(SkillenvError::RepoRequired),
        };
    };

    let generated_names = GeneratedNameLayout::for_mode(&repo_root, mode);
    let discovery_root = source_repo_root(mode, &repo_root);
    let discovery = discover_sources(
        &discovery_root,
        &generated_names.repo_slug,
        config,
        loaded.base_dir.as_deref(),
    )?;
    let target_root = target_root(mode, &repo_root)?;
    let mut statuses = Vec::new();
    for kind in [TargetKind::Agents, TargetKind::Claude] {
        let path = target_root.join(kind.label());
        if enabled.contains(&kind) {
            let managed_count =
                count_managed_entries(&path, &generated_names, &discovery.known_source_roots)?;
            statuses.push(TargetStatus {
                kind,
                path: Some(path.clone()),
                state: if managed_count > 0 {
                    LinkState::Linked
                } else {
                    LinkState::NotLinked
                },
                managed_count,
            });
        } else {
            statuses.push(TargetStatus {
                kind,
                path: Some(path),
                state: LinkState::Disabled,
                managed_count: 0,
            });
        }
    }

    Ok(StatusReport {
        repo_root: Some(repo_root),
        repo_slug: Some(generated_names.repo_slug),
        target_statuses: statuses,
    })
}

fn all_target_statuses(repo_root: Option<PathBuf>, enabled: &[TargetKind]) -> Vec<TargetStatus> {
    [TargetKind::Agents, TargetKind::Claude]
        .into_iter()
        .map(|kind| {
            let state = if enabled.contains(&kind) {
                LinkState::NotLinked
            } else {
                LinkState::Disabled
            };
            let path = repo_root.as_ref().map(|root| root.join(kind.label()));
            TargetStatus {
                kind,
                path,
                state,
                managed_count: 0,
            }
        })
        .collect()
}

fn load_config(config_override: Option<&Path>) -> Result<LoadedConfig> {
    let Some(config_path) = config_override
        .map(PathBuf::from)
        .or_else(default_config_path)
    else {
        return Ok(LoadedConfig {
            config: Config::default(),
            base_dir: None,
        });
    };
    let base_dir = config_path.parent().map(Path::to_path_buf);

    if !config_path.exists() {
        return Ok(LoadedConfig {
            config: Config::default(),
            base_dir,
        });
    }

    let raw = fs::read_to_string(&config_path).map_err(|source| SkillenvError::ReadFile {
        path: config_path.clone(),
        source,
    })?;
    let config = toml::from_str(&raw).map_err(|source| SkillenvError::ParseConfig {
        path: config_path,
        source,
    })?;
    Ok(LoadedConfig { config, base_dir })
}

fn default_config_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/skillenv/config.toml"))
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(SkillenvError::HomeNotSet)
}

fn detect_repo_root(cwd: &Path) -> Option<PathBuf> {
    let start = if cwd.is_absolute() {
        normalize_path(cwd)
    } else {
        normalize_path(&env::current_dir().ok()?.join(cwd))
    };

    for candidate in start.ancestors() {
        let git_path = candidate.join(".git");
        if git_path.exists() {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn resolve_targets(
    mode: TargetRootMode,
    repo_root: &Path,
    config: &Config,
    override_value: TargetOverride,
) -> Result<Vec<TargetSpec>> {
    let target_root = target_root(mode, repo_root)?;
    Ok(build_target_specs(
        &target_root,
        &resolve_target_kinds(config, override_value),
    ))
}

fn target_root(mode: TargetRootMode, repo_root: &Path) -> Result<PathBuf> {
    match mode {
        TargetRootMode::RepoLocal => Ok(repo_root.to_path_buf()),
        TargetRootMode::Global => home_dir(),
    }
}

fn source_repo_root(mode: TargetRootMode, repo_root: &Path) -> PathBuf {
    match mode {
        TargetRootMode::RepoLocal => repo_root.to_path_buf(),
        TargetRootMode::Global => stable_global_repo_root(repo_root),
    }
}

fn build_target_specs(target_root: &Path, kinds: &[TargetKind]) -> Vec<TargetSpec> {
    kinds
        .iter()
        .copied()
        .map(|kind| TargetSpec {
            kind,
            path: target_root.join(kind.label()),
        })
        .collect()
}

fn resolve_target_kinds(config: &Config, override_value: TargetOverride) -> Vec<TargetKind> {
    let claude = match override_value {
        TargetOverride::UseConfig => config.targets.claude,
        TargetOverride::ForceEnabled => true,
        TargetOverride::ForceDisabled => false,
    };

    let mut targets = Vec::new();
    if config.targets.agents {
        targets.push(TargetKind::Agents);
    }
    if claude {
        targets.push(TargetKind::Claude);
    }
    targets
}

fn include_claude_target(config: &Config, override_value: TargetOverride) -> bool {
    resolve_target_kinds(config, override_value).contains(&TargetKind::Claude)
}

fn discover_sources(
    repo_root: &Path,
    repo_slug: &str,
    config: &Config,
    config_base_dir: Option<&Path>,
) -> Result<Discovery> {
    let all_roots = all_source_roots(repo_root, repo_slug, config, config_base_dir)?;
    let known_source_roots = all_roots.iter().map(|(_, root)| root.clone()).collect();

    let mut by_scope: BTreeMap<ScopeKey, Vec<SkillSource>> = BTreeMap::new();
    let mut seen: BTreeMap<(ScopeKey, String), String> = BTreeMap::new();

    for (origin_name, root) in all_roots {
        collect_scope_sources(
            &mut by_scope,
            &mut seen,
            ScopeKey::Default,
            root.join(DEFAULT_SCOPE_DIR),
            &origin_name,
        )?;
        collect_scope_sources(
            &mut by_scope,
            &mut seen,
            ScopeKey::Local,
            root.join(LOCAL_SCOPE_DIR),
            &origin_name,
        )?;

        let profiles_root = root.join(PROFILES_SCOPE_DIR);
        if profiles_root.is_dir() {
            for entry in fs::read_dir(&profiles_root).map_err(|source| SkillenvError::ReadFile {
                path: profiles_root.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| SkillenvError::ReadFile {
                    path: profiles_root.clone(),
                    source,
                })?;
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let profile_name = slugify_or(
                    path.file_name()
                        .and_then(OsStr::to_str)
                        .unwrap_or("profile"),
                    "profile",
                );
                collect_scope_sources(
                    &mut by_scope,
                    &mut seen,
                    ScopeKey::Profile(profile_name),
                    path,
                    &origin_name,
                )?;
            }
        }
    }

    Ok(Discovery {
        sources: by_scope,
        known_source_roots,
    })
}

fn known_source_roots(
    repo_root: &Path,
    repo_slug: &str,
    config: &Config,
    config_base_dir: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    Ok(
        all_source_roots(repo_root, repo_slug, config, config_base_dir)?
            .into_iter()
            .map(|(_, root)| root)
            .collect(),
    )
}

fn resolve_external_root(
    external: &ExternalSourceConfig,
    repo_slug: &str,
    config_base_dir: Option<&Path>,
) -> PathBuf {
    let replaced = external.path.replace("{repo}", repo_slug);
    let path = PathBuf::from(replaced);
    if path.is_absolute() {
        path
    } else if let Some(base_dir) = config_base_dir {
        base_dir.join(path)
    } else {
        path
    }
}

fn all_source_roots(
    repo_root: &Path,
    repo_slug: &str,
    config: &Config,
    config_base_dir: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>> {
    let mut roots = vec![("repo".to_string(), repo_root.join(REPO_LAYOUT_DIR))];
    for external in &config.external_sources {
        roots.push((
            external.name.clone(),
            resolve_external_root(external, repo_slug, config_base_dir),
        ));
    }
    for managed in remote::installed_source_roots(repo_root)? {
        roots.push((managed.name, managed.root));
    }
    Ok(roots)
}

fn collect_scope_sources(
    by_scope: &mut BTreeMap<ScopeKey, Vec<SkillSource>>,
    seen: &mut BTreeMap<(ScopeKey, String), String>,
    scope: ScopeKey,
    scope_dir: PathBuf,
    origin_name: &str,
) -> Result<()> {
    if !scope_dir.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(&scope_dir).map_err(|source| SkillenvError::ReadFile {
        path: scope_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| SkillenvError::ReadFile {
            path: scope_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("SKILL.md").is_file() {
            continue;
        }

        let skill_slug = slugify_or(
            path.file_name().and_then(OsStr::to_str).unwrap_or("skill"),
            "skill",
        );
        let key = (scope.clone(), skill_slug.clone());
        let origin_label = format!("{origin_name}:{}", path.display());
        if let Some(existing) = seen.insert(key.clone(), origin_label.clone()) {
            return Err(SkillenvError::DuplicateSkill {
                scope: key.0.display_name(),
                skill: key.1,
                first: existing,
                second: origin_label,
            });
        }
        by_scope
            .entry(scope.clone())
            .or_default()
            .push(SkillSource {
                scope_origin: scope_dir.clone(),
                skill_slug,
                dir: path,
                origin_label,
            });
    }

    Ok(())
}

fn desired_sources(
    discovery: &Discovery,
    selector: &ScopeSelector,
) -> BTreeMap<ScopeKey, Vec<SkillSource>> {
    match selector {
        ScopeSelector::DefaultLocal => discovery
            .sources
            .iter()
            .filter(|(scope, _)| matches!(scope, ScopeKey::Default | ScopeKey::Local))
            .map(|(scope, sources)| (scope.clone(), sources.clone()))
            .collect(),
        ScopeSelector::Profiles(names) => {
            let requested: BTreeSet<_> = names
                .iter()
                .map(|name| ScopeKey::Profile(slugify_or(name, "profile")))
                .collect();
            discovery
                .sources
                .iter()
                .filter(|(scope, _)| requested.contains(*scope))
                .map(|(scope, sources)| (scope.clone(), sources.clone()))
                .collect()
        }
        ScopeSelector::All => discovery.sources.clone(),
    }
}

fn touched_scope_names(
    selector: &ScopeSelector,
    desired_sources: &BTreeMap<ScopeKey, Vec<SkillSource>>,
) -> Vec<String> {
    match selector {
        ScopeSelector::All => desired_sources.keys().map(ScopeKey::display_name).collect(),
        _ => ScopeKey::selector_names(selector)
            .into_iter()
            .map(|scope| scope.display_name())
            .collect(),
    }
}

fn removal_filter(selector: &ScopeSelector) -> ScopeFilter {
    match selector {
        ScopeSelector::All => ScopeFilter::AllCurrentRepo,
        _ => ScopeFilter::Exact(
            ScopeKey::selector_names(selector)
                .into_iter()
                .map(|scope| scope.display_name())
                .collect(),
        ),
    }
}

fn remove_managed_entries(
    target_dir: &Path,
    generated_names: &GeneratedNameLayout,
    filter: &ScopeFilter,
    known_source_roots: &[PathBuf],
) -> Result<usize> {
    if !target_dir.exists() {
        return Ok(0);
    }

    let mut removed = 0usize;
    let prefix = generated_names.prefix();
    for entry in fs::read_dir(target_dir).map_err(|source| SkillenvError::ReadFile {
        path: target_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| SkillenvError::ReadFile {
            path: target_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(prefix) {
            continue;
        }

        let metadata = fs::symlink_metadata(&path).map_err(|source| SkillenvError::ReadFile {
            path: path.clone(),
            source,
        })?;

        if metadata.file_type().is_symlink() {
            if generated_name_matches_scope(&name, generated_names, filter)
                && symlink_targets_known_root(&path, known_source_roots)?
            {
                fs::remove_file(&path).map_err(|source| SkillenvError::WriteFile {
                    path: path.clone(),
                    source,
                })?;
                removed += 1;
            }
            continue;
        }

        if metadata.is_dir() {
            let marker_path = path.join(GENERATED_MARKER_FILE);
            if !marker_path.is_file() {
                continue;
            }

            let marker_raw =
                fs::read_to_string(&marker_path).map_err(|source| SkillenvError::ReadFile {
                    path: marker_path.clone(),
                    source,
                })?;
            let marker: GeneratedMarker = serde_json::from_str(&marker_raw).map_err(|source| {
                SkillenvError::SerializeMarker {
                    path: marker_path.clone(),
                    source,
                }
            })?;

            if marker.generated_name.starts_with(generated_names.prefix())
                && filter.matches_scope(&marker.scope)
                && marker_source_matches_known_root(&marker.source, known_source_roots)
            {
                fs::remove_dir_all(&path).map_err(|source| SkillenvError::WriteFile {
                    path: path.clone(),
                    source,
                })?;
                removed += 1;
            }
        }
    }

    Ok(removed)
}

fn reconcile_target(
    target_dir: &Path,
    generated_names: &GeneratedNameLayout,
    strategy: Strategy,
    desired_sources: &BTreeMap<ScopeKey, Vec<SkillSource>>,
) -> Result<usize> {
    let mut linked = 0usize;
    for (scope, sources) in desired_sources {
        for source in sources {
            let generated_name = generated_names.generated_name(scope, &source.skill_slug);
            let generated_path = target_dir.join(&generated_name);
            ensure_unmanaged_target_absent(&generated_path)?;
            match strategy {
                Strategy::Render => render_source(
                    &generated_names.repo_slug,
                    scope,
                    source,
                    &generated_name,
                    &generated_path,
                )?,
                Strategy::Symlink => symlink_source(source, &generated_path)?,
            }
            linked += 1;
        }
    }

    Ok(linked)
}

fn render_source(
    repo_slug: &str,
    scope: &ScopeKey,
    source: &SkillSource,
    generated_name: &str,
    generated_path: &Path,
) -> Result<()> {
    ensure_dir(generated_path)?;
    copy_source_tree(&source.dir, generated_path)?;

    let skill_md_path = source.dir.join("SKILL.md");
    let rendered = render_skill_markdown(repo_slug, scope, source, generated_name, &skill_md_path)?;
    let target_skill_md = generated_path.join("SKILL.md");
    fs::write(&target_skill_md, rendered).map_err(|source| SkillenvError::WriteFile {
        path: target_skill_md,
        source,
    })?;

    let marker = GeneratedMarker {
        repo: repo_slug.to_string(),
        scope: scope.display_name(),
        skill: source.skill_slug.clone(),
        generated_name: generated_name.to_string(),
        source: source.dir.display().to_string(),
        strategy: Strategy::Render,
    };
    let marker_path = generated_path.join(GENERATED_MARKER_FILE);
    let marker_json =
        serde_json::to_string_pretty(&marker).map_err(|source| SkillenvError::SerializeMarker {
            path: marker_path.clone(),
            source,
        })?;
    fs::write(&marker_path, marker_json).map_err(|source| SkillenvError::WriteFile {
        path: marker_path,
        source,
    })?;
    Ok(())
}

fn symlink_source(source: &SkillSource, generated_path: &Path) -> Result<()> {
    create_symlink(&source.dir, generated_path).map_err(|source| SkillenvError::WriteFile {
        path: generated_path.to_path_buf(),
        source,
    })
}

fn render_skill_markdown(
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

fn summarize_markdown_body(body: &str) -> Option<String> {
    let mut in_code_block = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block
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

fn parse_frontmatter(path: &Path, raw: &str) -> Result<(Mapping, String)> {
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

fn copy_source_tree(source_dir: &Path, target_dir: &Path) -> Result<()> {
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

fn gitignore_patterns(include_claude: bool) -> Vec<&'static str> {
    let mut patterns = vec![".agents/skills/skillenv-*", "skillenv/local/"];
    if include_claude {
        patterns.push(".claude/skills/skillenv-*");
    }
    patterns
}

fn repo_is_initialized(repo_root: &Path, include_claude: bool) -> Result<bool> {
    for scope_dir in [DEFAULT_SCOPE_DIR, LOCAL_SCOPE_DIR, PROFILES_SCOPE_DIR] {
        if !repo_root.join(REPO_LAYOUT_DIR).join(scope_dir).is_dir() {
            return Ok(false);
        }
    }

    let gitignore_path = repo_root.join(".gitignore");
    if !gitignore_path.is_file() {
        return Ok(false);
    }

    let contents =
        fs::read_to_string(&gitignore_path).map_err(|source| SkillenvError::ReadFile {
            path: gitignore_path,
            source,
        })?;
    let existing: BTreeSet<String> = contents
        .lines()
        .map(|line| line.trim().to_string())
        .collect();

    Ok(gitignore_patterns(include_claude)
        .into_iter()
        .all(|pattern| existing.contains(pattern)))
}

fn require_repo_initialized(repo_root: &Path, include_claude: bool) -> Result<()> {
    if repo_is_initialized(repo_root, include_claude)? {
        Ok(())
    } else {
        Err(SkillenvError::RepoNotInitialized)
    }
}

fn update_gitignore(repo_root: &Path, include_claude: bool) -> Result<bool> {
    let gitignore_path = repo_root.join(".gitignore");
    let mut contents = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path).map_err(|source| SkillenvError::ReadFile {
            path: gitignore_path.clone(),
            source,
        })?
    } else {
        String::new()
    };

    let existing: BTreeSet<String> = contents
        .lines()
        .map(|line| line.trim().to_string())
        .collect();
    let mut changed = false;
    for pattern in gitignore_patterns(include_claude) {
        if existing.contains(pattern) {
            continue;
        }
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(pattern);
        contents.push('\n');
        changed = true;
    }

    if changed {
        fs::write(&gitignore_path, contents).map_err(|source| SkillenvError::WriteFile {
            path: gitignore_path,
            source,
        })?;
    }

    Ok(changed)
}

fn count_managed_entries(
    target_dir: &Path,
    generated_names: &GeneratedNameLayout,
    known_source_roots: &[PathBuf],
) -> Result<usize> {
    if !target_dir.is_dir() {
        return Ok(0);
    }

    let mut count = 0usize;
    let prefix = generated_names.prefix();
    for entry in fs::read_dir(target_dir).map_err(|source| SkillenvError::ReadFile {
        path: target_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| SkillenvError::ReadFile {
            path: target_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(prefix) {
            continue;
        }

        let metadata = fs::symlink_metadata(&path).map_err(|source| SkillenvError::ReadFile {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            if symlink_targets_known_root(&path, known_source_roots)? {
                count += 1;
            }
            continue;
        }

        if metadata.is_dir() {
            let marker_path = path.join(GENERATED_MARKER_FILE);
            if !marker_path.is_file() {
                continue;
            }
            let marker_raw =
                fs::read_to_string(&marker_path).map_err(|source| SkillenvError::ReadFile {
                    path: marker_path.clone(),
                    source,
                })?;
            let marker: GeneratedMarker = serde_json::from_str(&marker_raw).map_err(|source| {
                SkillenvError::SerializeMarker {
                    path: marker_path.clone(),
                    source,
                }
            })?;
            if marker.generated_name.starts_with(generated_names.prefix())
                && marker_source_matches_known_root(&marker.source, known_source_roots)
            {
                count += 1;
            }
        }
    }

    Ok(count)
}

fn repo_slug(repo_root: &Path) -> String {
    slugify_or(
        repo_root
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("repo"),
        "repo",
    )
}

fn stable_global_repo_root(repo_root: &Path) -> PathBuf {
    fs::canonicalize(repo_root)
        .map(|path| normalize_path(&path))
        .unwrap_or_else(|_| normalize_path(repo_root))
}

fn short_path_digest(path: &Path) -> String {
    let normalized = normalize_path(path);
    let mut hash = 0xcbf29ce484222325u64;
    for byte in normalized.display().to_string().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let digest = format!("{hash:016x}");
    digest[..12].to_string()
}

fn slugify_or(input: &str, fallback: &str) -> String {
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

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| SkillenvError::CreateDir {
        path: path.to_path_buf(),
        source,
    })
}

fn ensure_layout_dir(path: &Path, created_dirs: &mut Vec<PathBuf>) -> Result<()> {
    let existed = path.is_dir();
    ensure_dir(path)?;
    if !existed {
        created_dirs.push(path.to_path_buf());
    }
    Ok(())
}

fn ensure_unmanaged_target_absent(path: &Path) -> Result<()> {
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

fn generated_name_matches_scope(
    name: &str,
    generated_names: &GeneratedNameLayout,
    filter: &ScopeFilter,
) -> bool {
    match filter {
        ScopeFilter::AllCurrentRepo => name.starts_with(generated_names.prefix()),
        ScopeFilter::Exact(scopes) => scopes
            .iter()
            .any(|scope| name.starts_with(&generated_names.scope_prefix(scope))),
    }
}

fn scope_segment_from_display(scope: &str) -> String {
    if let Some(profile) = scope.strip_prefix("profile:") {
        format!("profile-{profile}")
    } else {
        scope.to_string()
    }
}

fn symlink_targets_known_root(path: &Path, known_source_roots: &[PathBuf]) -> Result<bool> {
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

fn marker_source_matches_known_root(source: &str, known_source_roots: &[PathBuf]) -> bool {
    let source_path = normalize_path(Path::new(source));
    known_source_roots
        .iter()
        .map(|root| normalize_path(root))
        .any(|root| source_path.starts_with(&root))
}

fn normalize_path(path: &Path) -> PathBuf {
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

impl ScopeFilter {
    fn matches_scope(&self, scope: &str) -> bool {
        match self {
            Self::AllCurrentRepo => true,
            Self::Exact(scopes) => scopes.contains(scope),
        }
    }
}

#[cfg(unix)]
fn create_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn create_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(source, destination)
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zsh => write!(f, "zsh"),
            Self::Bash => write!(f, "bash"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn frontmatter_string<'a>(metadata: &'a Mapping, key: &str) -> Option<&'a str> {
        metadata
            .get(Value::String(key.to_string()))
            .and_then(Value::as_str)
    }

    fn frontmatter_mapping<'a>(metadata: &'a Mapping, key: &str) -> Option<&'a Mapping> {
        metadata
            .get(Value::String(key.to_string()))
            .and_then(Value::as_mapping)
    }

    #[test]
    fn slugify_normalizes_values() {
        assert_eq!(slugify_or("My Repo", "repo"), "my-repo");
        assert_eq!(slugify_or("---A__B---", "repo"), "a-b");
        assert_eq!(slugify_or("Already-kebab", "repo"), "already-kebab");
        assert_eq!(slugify_or("!!!", "repo"), "repo");
    }

    #[test]
    fn config_defaults_apply_without_file() -> Result<()> {
        let root = TempDir::new().unwrap();
        let loaded = load_config(Some(&root.path().join("missing.toml")))?;
        assert!(loaded.config.targets.agents);
        assert!(!loaded.config.targets.claude);
        assert_eq!(loaded.config.defaults.strategy, Strategy::Render);
        Ok(())
    }

    #[test]
    fn external_source_root_resolves_repo_placeholder() {
        let external = ExternalSourceConfig {
            name: "shared".to_string(),
            path: "/tmp/skills/{repo}".to_string(),
        };
        assert_eq!(
            resolve_external_root(&external, "demo-repo", None),
            PathBuf::from("/tmp/skills/demo-repo")
        );

        let raw = ExternalSourceConfig {
            name: "raw".to_string(),
            path: "/tmp/skills".to_string(),
        };
        assert_eq!(
            resolve_external_root(&raw, "demo-repo", None),
            PathBuf::from("/tmp/skills")
        );
    }

    #[test]
    fn relative_external_paths_resolve_from_config_directory() {
        let config_dir = PathBuf::from("/tmp/skillenv-config");
        let external = ExternalSourceConfig {
            name: "shared".to_string(),
            path: "../shared/{repo}".to_string(),
        };
        assert_eq!(
            resolve_external_root(&external, "demo-repo", Some(&config_dir)),
            PathBuf::from("/tmp/skillenv-config/../shared/demo-repo")
        );
    }

    #[test]
    fn claude_cli_override_wins_over_config() {
        let mut config = Config::default();
        config.targets.claude = false;
        assert_eq!(
            resolve_target_kinds(&config, TargetOverride::ForceEnabled),
            vec![TargetKind::Agents, TargetKind::Claude]
        );
        assert_eq!(
            resolve_target_kinds(&config, TargetOverride::ForceDisabled),
            vec![TargetKind::Agents]
        );
    }

    #[test]
    fn detect_repo_root_normalizes_dot_segments() -> Result<()> {
        let repo = repo_fixture()?;
        let detected = detect_repo_root(&repo.path().join(".")).unwrap();
        assert_eq!(detected, repo.path());
        Ok(())
    }

    #[test]
    fn discover_sources_from_repo_only() -> Result<()> {
        let repo = repo_fixture()?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            None,
            "repo default",
        )?;
        let discovery = discover_sources(repo.path(), "repo", &Config::default(), None)?;
        assert_eq!(discovery.sources.len(), 1);
        assert_eq!(
            discovery.sources.get(&ScopeKey::Default).unwrap()[0].skill_slug,
            "research"
        );
        Ok(())
    }

    #[test]
    fn discover_sources_from_external_only() -> Result<()> {
        let repo = repo_fixture()?;
        let external = TempDir::new().unwrap();
        write_skill(
            external.path(),
            "default/research",
            None,
            "external default",
        )?;

        let mut config = Config::default();
        config.external_sources.push(ExternalSourceConfig {
            name: "shared".to_string(),
            path: external.path().display().to_string(),
        });

        let discovery = discover_sources(repo.path(), "repo", &config, None)?;
        assert_eq!(discovery.sources.get(&ScopeKey::Default).unwrap().len(), 1);
        assert_eq!(
            discovery.sources.get(&ScopeKey::Default).unwrap()[0].origin_label,
            format!("shared:{}/default/research", external.path().display())
        );
        Ok(())
    }

    #[test]
    fn discover_sources_from_repo_and_external() -> Result<()> {
        let repo = repo_fixture()?;
        let external = TempDir::new().unwrap();
        write_skill(
            repo.path(),
            "skillenv/default/research",
            None,
            "repo default",
        )?;
        write_skill(
            external.path(),
            "profiles/review/lint",
            None,
            "external profile",
        )?;

        let mut config = Config::default();
        config.external_sources.push(ExternalSourceConfig {
            name: "shared".to_string(),
            path: external.path().display().to_string(),
        });

        let discovery = discover_sources(repo.path(), "repo", &config, None)?;
        assert!(discovery.sources.contains_key(&ScopeKey::Default));
        assert!(
            discovery
                .sources
                .contains_key(&ScopeKey::Profile("review".to_string()))
        );
        Ok(())
    }

    #[test]
    fn duplicate_skills_are_rejected() -> Result<()> {
        let repo = repo_fixture()?;
        let external = TempDir::new().unwrap();
        write_skill(
            repo.path(),
            "skillenv/default/research",
            None,
            "repo default",
        )?;
        write_skill(
            external.path(),
            "default/research",
            None,
            "external default",
        )?;

        let mut config = Config::default();
        config.external_sources.push(ExternalSourceConfig {
            name: "shared".to_string(),
            path: external.path().display().to_string(),
        });

        let error = discover_sources(repo.path(), "repo", &config, None).unwrap_err();
        assert!(matches!(error, SkillenvError::DuplicateSkill { .. }));
        Ok(())
    }

    #[test]
    fn render_rewrites_frontmatter_and_copies_extra_files() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let scope_origin = repo.path().join("skillenv/default");
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        let skill_md = format!(
            r#"---
name: original-name
description: 'Original description [skillenv: {repo_slug}/default/research] repo={}'
color: blue
metadata:
  owner: team-skillenv
---

Body text
"#,
            scope_origin.display()
        );
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some(skill_md.as_str()),
            "Body text",
        )?;
        let extra_file = repo
            .path()
            .join("skillenv/default/research/assets/info.txt");
        ensure_dir(extra_file.parent().unwrap())?;
        fs::write(&extra_file, "extra").unwrap();
        init_test_repo(repo.path(), &config_path)?;

        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let generated_name = format!("skillenv-{repo_slug}-default-research");
        let expected_source = format!("{repo_slug}/default/research");
        let expected_scope_origin = scope_origin.display().to_string();
        let generated = repo.path().join(".agents/skills").join(&generated_name);
        let rendered = fs::read_to_string(generated.join("SKILL.md")).unwrap();
        let (metadata, body) = parse_frontmatter(Path::new("generated"), &rendered)?;
        assert_eq!(
            frontmatter_string(&metadata, "name"),
            Some(generated_name.as_str())
        );
        assert_eq!(
            frontmatter_string(&metadata, "description"),
            Some("Original description")
        );
        assert_eq!(frontmatter_string(&metadata, "color"), Some("blue"));
        let extra_metadata = frontmatter_mapping(&metadata, "metadata").unwrap();
        assert_eq!(
            frontmatter_string(extra_metadata, "owner"),
            Some("team-skillenv")
        );
        assert_eq!(
            frontmatter_string(extra_metadata, "skillenv.source"),
            Some(expected_source.as_str())
        );
        assert_eq!(
            frontmatter_string(extra_metadata, "skillenv.scope_origin"),
            Some(expected_scope_origin.as_str())
        );
        assert_eq!(body, "\nBody text\n");
        assert_eq!(
            fs::read_to_string(generated.join("assets/info.txt")).unwrap(),
            "extra"
        );
        assert!(generated.join(GENERATED_MARKER_FILE).is_file());
        Ok(())
    }

    #[test]
    fn render_creates_frontmatter_when_missing() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some("Body only\n"),
            "Body only",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let generated_name = format!("skillenv-{repo_slug}-default-research");
        let expected_source = format!("{repo_slug}/default/research");
        let expected_scope_origin = repo.path().join("skillenv/default").display().to_string();
        let generated = repo
            .path()
            .join(".agents/skills")
            .join(&generated_name)
            .join("SKILL.md");
        let rendered = fs::read_to_string(generated).unwrap();
        let (metadata, body) = parse_frontmatter(Path::new("generated"), &rendered)?;
        assert_eq!(
            frontmatter_string(&metadata, "name"),
            Some(generated_name.as_str())
        );
        assert_eq!(
            frontmatter_string(&metadata, "description"),
            Some("Body only")
        );
        let extra_metadata = frontmatter_mapping(&metadata, "metadata").unwrap();
        assert_eq!(
            frontmatter_string(extra_metadata, "skillenv.source"),
            Some(expected_source.as_str())
        );
        assert_eq!(
            frontmatter_string(extra_metadata, "skillenv.scope_origin"),
            Some(expected_scope_origin.as_str())
        );
        assert_eq!(body, "\nBody only\n");
        Ok(())
    }

    #[test]
    fn render_replaces_legacy_description_with_non_provenance_fallback() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let scope_origin = repo.path().join("skillenv/default");
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        let skill_md = format!(
            r#"---
description: "[skillenv: {repo_slug}/default/research] repo={}"
---

# Research Skill

```bash
echo test
```
"#,
            scope_origin.display()
        );
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some(skill_md.as_str()),
            "ignored",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let expected_source = format!("{repo_slug}/default/research");
        let expected_scope_origin = scope_origin.display().to_string();
        let rendered = fs::read_to_string(
            repo.path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-default-research"))
                .join("SKILL.md"),
        )
        .unwrap();
        let (metadata, body) = parse_frontmatter(Path::new("generated"), &rendered)?;
        assert_eq!(
            frontmatter_string(&metadata, "description"),
            Some("Instructions for the research skill.")
        );
        let extra_metadata = frontmatter_mapping(&metadata, "metadata").unwrap();
        assert_eq!(
            frontmatter_string(extra_metadata, "skillenv.source"),
            Some(expected_source.as_str())
        );
        assert_eq!(
            frontmatter_string(extra_metadata, "skillenv.scope_origin"),
            Some(expected_scope_origin.as_str())
        );
        assert_eq!(body, "\n# Research Skill\n\n```bash\necho test\n```\n");
        Ok(())
    }

    #[test]
    fn render_rejects_non_mapping_metadata_field() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some(
                r#"---
metadata: legacy-value
---

Body text
"#,
            ),
            "Body text",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        let error = link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))
            .unwrap_err();

        assert!(matches!(error, SkillenvError::InvalidMetadataField { .. }));
        Ok(())
    }

    #[test]
    fn symlink_strategy_creates_direct_symlink_and_leaves_source_unchanged() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "symlink"
"#,
        )?;
        let source_path = write_skill(
            repo.path(),
            "skillenv/default/research",
            Some("plain body\n"),
            "plain body",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let generated = repo
            .path()
            .join(".agents/skills")
            .join(format!("skillenv-{repo_slug}-default-research"));
        let metadata = fs::symlink_metadata(&generated).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(
            normalize_path(&fs::read_link(&generated).unwrap()),
            normalize_path(&source_path)
        );
        assert_eq!(
            fs::read_to_string(source_path.join("SKILL.md")).unwrap(),
            "plain body\n"
        );
        Ok(())
    }

    #[test]
    fn unlink_only_removes_safe_symlinks() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "symlink"
"#,
        )?;
        let source_path = write_skill(
            repo.path(),
            "skillenv/default/research",
            Some("plain body\n"),
            "plain body",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let target_dir = repo.path().join(".agents/skills");
        let unsafe_target = TempDir::new().unwrap();
        let unsafe_name = target_dir.join(format!("skillenv-{repo_slug}-default-unsafe"));
        create_symlink(unsafe_target.path(), &unsafe_name).unwrap();
        let handwritten = target_dir.join("handmade");
        ensure_dir(&handwritten)?;

        unlink_repo_with_config(repo.path(), &UnlinkOptions::default(), Some(&config_path))?;

        assert!(
            !target_dir
                .join(format!("skillenv-{repo_slug}-default-research"))
                .exists()
        );
        assert!(unsafe_name.exists());
        assert!(handwritten.exists());
        assert!(source_path.exists());
        Ok(())
    }

    #[test]
    fn unlink_still_works_when_source_discovery_would_fail() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            None,
            "primary source",
        )?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        write_skill(
            repo.path(),
            "skillenv/default/Research",
            None,
            "duplicate source",
        )?;

        unlink_repo_with_config(repo.path(), &UnlinkOptions::default(), Some(&config_path))?;

        assert!(
            !repo
                .path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-default-research"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn unlink_removes_symlinks_after_source_root_is_deleted() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "symlink"
"#,
        )?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some("plain body\n"),
            "plain body",
        )?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        fs::remove_dir_all(repo.path().join("skillenv")).unwrap();
        unlink_repo_with_config(repo.path(), &UnlinkOptions::default(), Some(&config_path))?;

        assert!(
            !repo
                .path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-default-research"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn link_refuses_to_overwrite_unmanaged_target() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(repo.path(), "")?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            None,
            "primary source",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        let collision = repo
            .path()
            .join(".agents/skills")
            .join(format!("skillenv-{repo_slug}-default-research"));
        ensure_dir(&collision)?;
        fs::write(collision.join("README.md"), "manual content").unwrap();

        let error = link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))
            .unwrap_err();
        assert!(matches!(error, SkillenvError::TargetCollision { .. }));
        assert_eq!(
            fs::read_to_string(collision.join("README.md")).unwrap(),
            "manual content"
        );
        Ok(())
    }

    #[test]
    fn link_default_local_only_leaves_profiles_in_place() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            None,
            "repo default",
        )?;
        write_skill(
            repo.path(),
            "skillenv/profiles/review/lint",
            None,
            "profile",
        )?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(
            repo.path(),
            &LinkOptions {
                selector: ScopeSelector::All,
                claude: TargetOverride::UseConfig,
                quiet: false,
            },
            Some(&config_path),
        )?;

        fs::remove_dir_all(repo.path().join("skillenv/default/research")).unwrap();
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        assert!(
            !repo
                .path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-default-research"))
                .exists()
        );
        assert!(
            repo.path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-profile-review-lint"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn link_profile_targets_only_requested_profile() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        write_skill(repo.path(), "skillenv/profiles/review/lint", None, "review")?;
        write_skill(
            repo.path(),
            "skillenv/profiles/migration/check",
            None,
            "migration",
        )?;
        init_test_repo(repo.path(), &config_path)?;

        link_repo_with_config(
            repo.path(),
            &LinkOptions {
                selector: ScopeSelector::Profiles(vec!["review".to_string()]),
                claude: TargetOverride::UseConfig,
                quiet: false,
            },
            Some(&config_path),
        )?;

        assert!(
            repo.path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-profile-review-lint"))
                .exists()
        );
        assert!(
            !repo
                .path()
                .join(".agents/skills")
                .join(format!("skillenv-{repo_slug}-profile-migration-check"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn link_all_cleans_current_repo_stale_entries_only() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let target_dir = repo.path().join(".agents/skills");
        let stale_current = target_dir.join(format!("skillenv-{repo_slug}-local-stale"));
        ensure_dir(&stale_current)?;
        let stale_marker = GeneratedMarker {
            repo: repo_slug.clone(),
            scope: "local".to_string(),
            skill: "stale".to_string(),
            generated_name: format!("skillenv-{repo_slug}-local-stale"),
            source: repo
                .path()
                .join("skillenv/local/stale")
                .display()
                .to_string(),
            strategy: Strategy::Render,
        };
        fs::write(
            stale_current.join(GENERATED_MARKER_FILE),
            serde_json::to_string_pretty(&stale_marker).unwrap(),
        )
        .unwrap();

        let other_repo = target_dir.join("skillenv-other-default-keep");
        ensure_dir(&other_repo)?;
        fs::write(
            other_repo.join(GENERATED_MARKER_FILE),
            serde_json::to_string_pretty(&GeneratedMarker {
                repo: "other".to_string(),
                scope: "default".to_string(),
                skill: "keep".to_string(),
                generated_name: "skillenv-other-default-keep".to_string(),
                source: repo.path().display().to_string(),
                strategy: Strategy::Render,
            })
            .unwrap(),
        )
        .unwrap();
        let manual = target_dir.join("manual-skill");
        ensure_dir(&manual)?;

        link_repo_with_config(
            repo.path(),
            &LinkOptions {
                selector: ScopeSelector::All,
                claude: TargetOverride::UseConfig,
                quiet: false,
            },
            Some(&config_path),
        )?;

        assert!(!stale_current.exists());
        assert!(other_repo.exists());
        assert!(manual.exists());
        Ok(())
    }

    #[test]
    fn cleanup_skips_rendered_directory_outside_known_roots() -> Result<()> {
        let repo = repo_fixture()?;
        let repo_slug = test_repo_slug(repo.path());
        let config_path = write_config(repo.path(), "")?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;
        init_test_repo(repo.path(), &config_path)?;

        let target_dir = repo.path().join(".agents/skills");
        let copied = target_dir.join(format!("skillenv-{repo_slug}-default-copied"));
        ensure_dir(&copied)?;
        fs::write(
            copied.join(GENERATED_MARKER_FILE),
            serde_json::to_string_pretty(&GeneratedMarker {
                repo: repo_slug.clone(),
                scope: "default".to_string(),
                skill: "copied".to_string(),
                generated_name: format!("skillenv-{repo_slug}-default-copied"),
                source: "/tmp/not-managed/copied".to_string(),
                strategy: Strategy::Render,
            })
            .unwrap(),
        )
        .unwrap();

        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        assert!(copied.exists());
        Ok(())
    }

    #[test]
    fn init_creates_layout_and_updates_gitignore() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;

        let report =
            init_repo_with_config(repo.path(), &InitOptions::default(), Some(&config_path))?;
        let gitignore = fs::read_to_string(repo.path().join(".gitignore")).unwrap();

        assert_eq!(report.created_dirs.len(), 3);
        assert!(repo.path().join("skillenv/default").is_dir());
        assert!(repo.path().join("skillenv/local").is_dir());
        assert!(repo.path().join("skillenv/profiles").is_dir());
        assert!(report.gitignore_updated);
        assert!(gitignore.contains(".agents/skills/skillenv-*"));
        assert!(gitignore.contains("skillenv/local/"));
        assert!(!gitignore.contains(".claude/skills/skillenv-*"));
        Ok(())
    }

    #[test]
    fn init_is_idempotent_and_uses_claude_target_config() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(
            repo.path(),
            r#"
[targets]
claude = true
"#,
        )?;

        let first =
            init_repo_with_config(repo.path(), &InitOptions::default(), Some(&config_path))?;
        let once = fs::read_to_string(repo.path().join(".gitignore")).unwrap();
        let second =
            init_repo_with_config(repo.path(), &InitOptions::default(), Some(&config_path))?;
        let twice = fs::read_to_string(repo.path().join(".gitignore")).unwrap();

        assert_eq!(once, twice);
        assert_eq!(first.created_dirs.len(), 3);
        assert!(second.created_dirs.is_empty());
        assert!(first.gitignore_updated);
        assert!(!second.gitignore_updated);
        assert!(twice.contains(".agents/skills/skillenv-*"));
        assert!(twice.contains(".claude/skills/skillenv-*"));
        assert!(twice.contains("skillenv/local/"));
        Ok(())
    }

    #[test]
    fn link_does_not_change_gitignore_after_init() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;
        init_test_repo(repo.path(), &config_path)?;
        let before = fs::read_to_string(repo.path().join(".gitignore")).unwrap();

        let report =
            link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;
        let after = fs::read_to_string(repo.path().join(".gitignore")).unwrap();

        assert!(!report.gitignore_updated);
        assert_eq!(before, after);
        Ok(())
    }

    #[test]
    fn status_uses_linked_and_not_linked() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let before =
            status_repo_with_config(repo.path(), &StatusOptions::default(), Some(&config_path))?;
        let before_text = format_status_report(&before);
        assert!(before_text.contains(".agents/skills: not linked"));

        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;
        let after =
            status_repo_with_config(repo.path(), &StatusOptions::default(), Some(&config_path))?;
        let after_text = format_status_report(&after);
        assert!(after_text.contains(".agents/skills: linked"));
        Ok(())
    }

    #[test]
    fn repo_local_link_requires_init_but_quiet_is_noop() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let error = link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))
            .unwrap_err();
        assert!(matches!(error, SkillenvError::RepoNotInitialized));
        assert!(!repo.path().join(".agents/skills").exists());

        let report = link_repo_with_config(
            repo.path(),
            &LinkOptions {
                quiet: true,
                ..LinkOptions::default()
            },
            Some(&config_path),
        )?;
        assert!(report.message.is_none());
        assert!(report.target_reports.is_empty());
        assert!(!repo.path().join(".agents/skills").exists());
        assert!(!repo.path().join(".gitignore").exists());
        Ok(())
    }

    #[test]
    fn repo_local_unlink_still_works_before_init() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let source_dir = write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let generated_name = GeneratedNameLayout::for_mode(repo.path(), TargetRootMode::RepoLocal)
            .generated_name(&ScopeKey::Default, "research");
        let generated_dir = repo.path().join(".agents/skills").join(generated_name);
        ensure_dir(&generated_dir)?;
        fs::write(
            generated_dir.join(GENERATED_MARKER_FILE),
            serde_json::to_string_pretty(&GeneratedMarker {
                repo: test_repo_slug(repo.path()),
                scope: "default".to_string(),
                skill: "research".to_string(),
                generated_name: generated_dir
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("generated")
                    .to_string(),
                source: source_dir.display().to_string(),
                strategy: Strategy::Render,
            })
            .unwrap(),
        )
        .unwrap();

        let report =
            unlink_repo_with_config(repo.path(), &UnlinkOptions::default(), Some(&config_path))?;
        assert_eq!(report.target_reports[0].removed, 1);
        assert!(!generated_dir.exists());
        assert!(!repo.path().join(".gitignore").exists());
        Ok(())
    }

    #[test]
    fn global_link_uses_home_targets_without_repo_init() -> Result<()> {
        let repo = repo_fixture()?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let report = link_global(repo.path(), LinkOptions::default())?;

        let generated_name = GeneratedNameLayout::for_mode(repo.path(), TargetRootMode::Global)
            .generated_name(&ScopeKey::Default, "research");
        let target_dir = home.path().join(".agents/skills");
        assert_eq!(report.target_reports.len(), 1);
        assert_eq!(report.target_reports[0].path, Some(target_dir.clone()));
        assert!(target_dir.join(generated_name).exists());
        assert!(!repo.path().join(".gitignore").exists());
        assert!(!repo.path().join("skillenv/local").exists());
        assert!(!repo.path().join("skillenv/profiles").exists());
        Ok(())
    }

    #[test]
    fn global_link_supports_claude_target() -> Result<()> {
        let repo = repo_fixture()?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let report = link_global(
            repo.path(),
            LinkOptions {
                claude: TargetOverride::ForceEnabled,
                ..LinkOptions::default()
            },
        )?;

        assert_eq!(report.target_reports.len(), 2);
        assert!(home.path().join(".agents/skills").is_dir());
        assert!(home.path().join(".claude/skills").is_dir());
        Ok(())
    }

    #[test]
    fn global_unlink_and_status_use_home_targets() -> Result<()> {
        let repo = repo_fixture()?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        link_global(repo.path(), LinkOptions::default())?;

        let target_dir = home.path().join(".agents/skills");
        ensure_dir(&target_dir.join("manual-skill"))?;

        let before = status_global(repo.path(), StatusOptions::default())?;
        assert_eq!(before.target_statuses[0].path, Some(target_dir.clone()));
        assert_eq!(before.target_statuses[0].state, LinkState::Linked);
        assert_eq!(before.target_statuses[0].managed_count, 1);
        assert!(format_status_report(&before).contains(&target_dir.display().to_string()));

        unlink_global(repo.path(), UnlinkOptions::default())?;

        assert!(
            !target_dir
                .join(
                    GeneratedNameLayout::for_mode(repo.path(), TargetRootMode::Global)
                        .generated_name(&ScopeKey::Default, "research")
                )
                .exists()
        );
        assert!(target_dir.join("manual-skill").exists());

        let after = status_global(repo.path(), StatusOptions::default())?;
        assert_eq!(after.target_statuses[0].state, LinkState::NotLinked);
        assert_eq!(after.target_statuses[0].managed_count, 0);
        Ok(())
    }

    #[test]
    fn global_names_include_repo_path_hash_to_avoid_collisions() -> Result<()> {
        let parent_one = TempDir::new().unwrap();
        let parent_two = TempDir::new().unwrap();
        let repo_one = repo_root_fixture(parent_one.path(), "demo")?;
        let repo_two = repo_root_fixture(parent_two.path(), "demo")?;
        write_skill(&repo_one, "skillenv/default/research", None, "default")?;
        write_skill(&repo_two, "skillenv/default/research", None, "default")?;

        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        link_global(&repo_one, LinkOptions::default())?;
        link_global(&repo_two, LinkOptions::default())?;

        let name_one = GeneratedNameLayout::for_mode(&repo_one, TargetRootMode::Global)
            .generated_name(&ScopeKey::Default, "research");
        let name_two = GeneratedNameLayout::for_mode(&repo_two, TargetRootMode::Global)
            .generated_name(&ScopeKey::Default, "research");

        assert_ne!(name_one, name_two);
        assert!(home.path().join(".agents/skills").join(name_one).exists());
        assert!(home.path().join(".agents/skills").join(name_two).exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn global_alias_path_reuses_same_managed_entries() -> Result<()> {
        let repo = repo_fixture()?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let alias_parent = TempDir::new().unwrap();
        let alias_path = alias_parent.path().join("repo-alias");
        create_symlink(repo.path(), &alias_path).unwrap();

        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        link_global(repo.path(), LinkOptions::default())?;

        let status_via_alias = status_global(&alias_path, StatusOptions::default())?;
        assert_eq!(status_via_alias.target_statuses[0].managed_count, 1);

        unlink_global(&alias_path, UnlinkOptions::default())?;

        let status_after_unlink = status_global(repo.path(), StatusOptions::default())?;
        assert_eq!(status_after_unlink.target_statuses[0].managed_count, 0);
        Ok(())
    }

    #[test]
    fn global_link_fails_without_home() -> Result<()> {
        let repo = repo_fixture()?;
        write_skill(repo.path(), "skillenv/default/research", None, "default")?;

        let _home = set_home_for_test(None);
        let error = link_global(repo.path(), LinkOptions::default()).unwrap_err();
        assert!(matches!(error, SkillenvError::HomeNotSet));
        Ok(())
    }

    #[test]
    fn hook_scripts_call_quiet_link() {
        assert!(hook_script(Shell::Zsh).contains("skillenv link --quiet"));
        assert!(hook_script(Shell::Bash).contains("skillenv link --quiet"));
    }

    fn repo_fixture() -> Result<TempDir> {
        let dir = TempDir::new().unwrap();
        ensure_dir(&dir.path().join(".git"))?;
        Ok(dir)
    }

    fn repo_root_fixture(parent: &Path, name: &str) -> Result<PathBuf> {
        let repo_root = parent.join(name);
        ensure_dir(&repo_root)?;
        ensure_dir(&repo_root.join(".git"))?;
        Ok(repo_root)
    }

    fn init_test_repo(repo_root: &Path, config_path: &Path) -> Result<()> {
        init_repo_with_config(repo_root, &InitOptions::default(), Some(config_path)).map(|_| ())
    }

    fn write_config(repo_root: &Path, body: &str) -> Result<PathBuf> {
        let config_dir = repo_root.join("test-home/.config/skillenv");
        ensure_dir(&config_dir)?;
        let path = config_dir.join("config.toml");
        fs::write(&path, body).map_err(|source| SkillenvError::WriteFile {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    fn write_skill(
        repo_root: &Path,
        relative: &str,
        skill_md: Option<&str>,
        fallback_body: &str,
    ) -> Result<PathBuf> {
        let dir = repo_root.join(relative);
        ensure_dir(&dir)?;
        let content = skill_md.unwrap_or(fallback_body);
        let skill_md_path = dir.join("SKILL.md");
        fs::write(&skill_md_path, content).map_err(|source| SkillenvError::WriteFile {
            path: skill_md_path,
            source,
        })?;
        Ok(dir)
    }

    fn test_repo_slug(repo_root: &Path) -> String {
        slugify_or(
            repo_root
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("repo"),
            "repo",
        )
    }

    fn set_home_for_test(home: Option<&Path>) -> HomeEnvGuard {
        let _lock = home_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = env::var_os("HOME");
        match home {
            Some(path) => unsafe {
                env::set_var("HOME", path);
            },
            None => unsafe {
                env::remove_var("HOME");
            },
        }
        HomeEnvGuard { previous, _lock }
    }

    fn home_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct HomeEnvGuard {
        previous: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for HomeEnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe {
                    env::set_var("HOME", value);
                },
                None => unsafe {
                    env::remove_var("HOME");
                },
            }
        }
    }
}
