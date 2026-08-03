use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_yaml::{Mapping, Value};
use thiserror::Error;

mod catalog;
mod inventory;
mod lock;
mod manifest;
mod paths;
mod provider;
mod remote;
mod render;
mod safeguard;

pub use inventory::format_skill_inventory_report;
pub use safeguard::{Finding, Severity};

/// Scan one `SKILL.md` for hidden instructions and unsafe patterns.
///
/// Frontmatter is included on purpose: `description` is loaded eagerly into agent
/// context while the body is not, which makes it the most valuable place to hide
/// an instruction.
pub fn scan_skill_text(text: &str) -> Vec<Finding> {
    safeguard::scan_text(text)
}
use inventory::skill_inventory_with_config;
use paths::{
    create_symlink, ensure_dir, ensure_layout_dir, ensure_unmanaged_target_absent,
    marker_source_matches_known_root, normalize_path, repo_slug, short_path_digest, slugify_or,
    stable_global_repo_root, symlink_targets_known_root,
};
use render::{copy_source_tree, parse_frontmatter, render_skill_markdown};

const GENERATED_MARKER_FILE: &str = ".skillenv-generated.json";
const REPO_LAYOUT_DIR: &str = "skillenv";
const DEFAULT_SCOPE_DIR: &str = "default";
const LOCAL_SCOPE_DIR: &str = "local";
const PROFILES_SCOPE_DIR: &str = "profiles";

pub type Result<T> = std::result::Result<T, SkillenvError>;

pub use remote::{
    AddSourceOptions, AddSourceReport, FetchSourcesOptions, FetchSourcesReport,
    FetchedLockedSource, UpdateSourcesOptions, UpdateSourcesReport, add_source, fetch_sources,
    format_add_source_report, format_fetch_sources_report, format_update_sources_report,
    update_sources,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillInventoryTool {
    Claude,
    Codex,
    Opencode,
    Antigravity,
}

impl SkillInventoryTool {
    fn all() -> [Self; 4] {
        [Self::Claude, Self::Codex, Self::Opencode, Self::Antigravity]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Antigravity => "antigravity",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SkillInventoryOptions {
    pub tools: Vec<SkillInventoryTool>,
    pub repo_tree: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DoctorOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillInventoryMode {
    #[serde(rename = "current")]
    Current,
    #[serde(rename = "current-and-repo-tree")]
    CurrentAndRepoTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SkillDiscoveryState {
    #[serde(rename = "current")]
    Current,
    #[serde(rename = "repo-tree-only")]
    RepoTreeOnly,
    #[serde(rename = "nested-on-demand")]
    NestedOnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillInventoryStatus {
    #[serde(rename = "shadowed")]
    Shadowed,
    #[serde(rename = "duplicate-visible")]
    DuplicateVisible,
    #[serde(rename = "invalid")]
    Invalid,
    #[serde(rename = "legacy")]
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInventoryReport {
    pub repo_root: Option<PathBuf>,
    pub mode: SkillInventoryMode,
    pub tools: Vec<SkillInventoryTool>,
    pub entries: Vec<SkillInventoryEntry>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInventoryEntry {
    pub tool: SkillInventoryTool,
    pub scope: String,
    pub discovery_state: SkillDiscoveryState,
    pub name: String,
    pub description: Option<String>,
    pub skill_dir: PathBuf,
    pub skill_md: Option<PathBuf>,
    pub skillenv_managed: bool,
    pub skillenv_origin: String,
    pub status: Vec<SkillInventoryStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub repo_root: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub config_exists: bool,
    pub repo_initialized: Option<bool>,
    pub config: DoctorConfigReport,
    pub source_roots: Vec<DoctorSourceRoot>,
    pub external_sources: Vec<DoctorExternalSource>,
    pub managed_sources: Vec<DoctorManagedSource>,
    pub repo_targets: Vec<DoctorTargetReport>,
    pub global_targets: Vec<DoctorTargetReport>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorConfigReport {
    pub targets_agents: bool,
    pub targets_claude: bool,
    pub strategy: Strategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorSourceRoot {
    pub origin: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorExternalSource {
    pub name: String,
    pub configured_path: String,
    pub resolved_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorManagedSource {
    pub name: String,
    pub kind: String,
    pub source: String,
    pub transport: String,
    pub requested_ref: Option<String>,
    pub subdir: Option<String>,
    pub install_root: PathBuf,
    pub selected_skills: Vec<String>,
    pub resolved_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DoctorTargetState {
    #[serde(rename = "linked")]
    Linked,
    #[serde(rename = "not-linked")]
    NotLinked,
    #[serde(rename = "disabled")]
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorTargetReport {
    pub label: String,
    pub path: Option<PathBuf>,
    pub state: DoctorTargetState,
    pub managed_count: usize,
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
        source: toml_edit::de::Error,
    },
    #[error("invalid manifest at {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: toml_edit::de::Error,
    },
    #[error("invalid manifest at {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },
    #[error("invalid skill id '{input}': {reason}")]
    InvalidSkillId { input: String, reason: String },
    #[error("unknown provider '{name}'; known providers are {known}")]
    UnknownProvider { name: String, known: String },
    #[error(
        "skill id '{id}' is declared twice, by {first} and {second}; ids are unique across every source"
    )]
    DuplicateSkillId {
        id: String,
        first: String,
        second: String,
    },
    #[error(
        "lock file at {path} is version {found}, but this build understands only version {supported}; upgrade skillenv"
    )]
    UnsupportedLockVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
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
    path: Option<PathBuf>,
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

pub fn skill_inventory(
    cwd: impl AsRef<Path>,
    options: SkillInventoryOptions,
) -> Result<SkillInventoryReport> {
    skill_inventory_with_config(cwd.as_ref(), &options, None)
}

pub fn doctor(cwd: impl AsRef<Path>, options: DoctorOptions) -> Result<DoctorReport> {
    doctor_with_config(cwd.as_ref(), &options, None)
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

pub fn format_doctor_report(report: &DoctorReport) -> String {
    let mut lines = Vec::new();
    match &report.repo_root {
        Some(repo_root) => lines.push(format!("repo root: {}", repo_root.display())),
        None => lines.push("repo root: not detected".to_string()),
    }
    match &report.home_dir {
        Some(home_dir) => lines.push(format!("home: {}", home_dir.display())),
        None => lines.push("home: not set".to_string()),
    }
    match &report.config_path {
        Some(config_path) => lines.push(format!(
            "config path: {} ({})",
            config_path.display(),
            if report.config_exists {
                "exists"
            } else {
                "missing"
            }
        )),
        None => lines.push("config path: unavailable".to_string()),
    }
    lines.push(format!(
        "repo initialized: {}",
        match report.repo_initialized {
            Some(true) => "yes",
            Some(false) => "no",
            None => "not applicable",
        }
    ));

    lines.push("config:".to_string());
    lines.push(format!(
        "  targets.agents: {}",
        enabled_label(report.config.targets_agents)
    ));
    lines.push(format!(
        "  targets.claude: {}",
        enabled_label(report.config.targets_claude)
    ));
    lines.push(format!(
        "  defaults.strategy: {}",
        strategy_label(report.config.strategy)
    ));

    lines.push("targets:".to_string());
    lines.push("  repo-local:".to_string());
    append_doctor_target_lines(&mut lines, &report.repo_targets);
    lines.push("  global:".to_string());
    append_doctor_target_lines(&mut lines, &report.global_targets);

    lines.push("source roots:".to_string());
    if report.source_roots.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for root in &report.source_roots {
            lines.push(format!(
                "  - origin={} path={}",
                root.origin,
                root.path.display()
            ));
        }
    }

    lines.push("external sources:".to_string());
    if report.external_sources.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for source in &report.external_sources {
            let resolved = source
                .resolved_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unresolved".to_string());
            lines.push(format!(
                "  - name={} configured={} resolved={}",
                source.name, source.configured_path, resolved
            ));
        }
    }

    lines.push("managed sources:".to_string());
    if report.managed_sources.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for source in &report.managed_sources {
            let skills = if source.selected_skills.is_empty() {
                "all".to_string()
            } else {
                source.selected_skills.join(",")
            };
            let mut parts = vec![
                format!("name={}", source.name),
                format!("kind={}", source.kind),
                format!("source={}", source.source),
                format!("transport={}", source.transport),
                format!("install_root={}", source.install_root.display()),
                format!("revision={}", source.resolved_revision),
                format!("skills={skills}"),
            ];
            if let Some(requested_ref) = &source.requested_ref {
                parts.push(format!("ref={requested_ref}"));
            }
            if let Some(subdir) = &source.subdir {
                parts.push(format!("subdir={subdir}"));
            }
            lines.push(format!("  - {}", parts.join(" ")));
        }
    }

    if !report.notes.is_empty() {
        lines.push("notes:".to_string());
        for note in &report.notes {
            lines.push(format!("  - {note}"));
        }
    }
    if !report.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.join("\n")
}

#[cfg_attr(not(test), allow(dead_code))]
fn doctor_with_config(
    cwd: &Path,
    _options: &DoctorOptions,
    config_override: Option<&Path>,
) -> Result<DoctorReport> {
    let loaded = load_config(config_override)?;
    let repo_root = detect_repo_root(cwd);
    let home_dir = env::var_os("HOME").map(PathBuf::from);
    let config_exists = loaded.path.as_ref().is_some_and(|path| path.exists());
    let mut notes = Vec::new();
    let mut warnings = Vec::new();

    if !config_exists {
        match &loaded.path {
            Some(path) => notes.push(format!(
                "config file not found at {}; using built-in defaults",
                path.display()
            )),
            None => notes.push("config path unavailable; using built-in defaults".to_string()),
        }
    }

    if repo_root.is_none() {
        notes.push("repo root not detected; repo-local diagnostics are limited".to_string());
    }
    if home_dir.is_none() {
        notes.push("HOME not set; global target diagnostics are unavailable".to_string());
    }

    let repo_initialized = match repo_root.as_deref() {
        Some(repo_root) => Some(repo_is_initialized(
            repo_root,
            include_claude_target(&loaded.config, TargetOverride::UseConfig),
        )?),
        None => None,
    };

    let external_sources = doctor_external_sources(
        repo_root.as_deref(),
        &loaded.config,
        loaded.base_dir.as_deref(),
        &mut notes,
        &mut warnings,
    );

    let managed_sources = match repo_root.as_deref() {
        Some(repo_root) => remote::managed_source_details(repo_root)?
            .into_iter()
            .map(|source| {
                if !source.install_root.exists() {
                    warnings.push(format!(
                        "managed source '{}' install root is missing: {}",
                        source.name,
                        source.install_root.display()
                    ));
                }
                DoctorManagedSource {
                    name: source.name,
                    kind: source.kind,
                    source: source.source,
                    transport: source.transport,
                    requested_ref: source.requested_ref,
                    subdir: source.subdir,
                    install_root: source.install_root,
                    selected_skills: source.selected_skills,
                    resolved_revision: source.resolved_revision,
                }
            })
            .collect(),
        None => Vec::new(),
    };

    let source_roots = match repo_root.as_deref() {
        Some(repo_root) => {
            let repo_slug = repo_slug(repo_root);
            all_source_roots(
                repo_root,
                &repo_slug,
                &loaded.config,
                loaded.base_dir.as_deref(),
            )?
            .into_iter()
            .map(|(origin, path)| DoctorSourceRoot { origin, path })
            .collect()
        }
        None => Vec::new(),
    };

    let enabled_targets = resolve_target_kinds(&loaded.config, TargetOverride::UseConfig);
    let repo_targets = match repo_root.as_deref() {
        Some(repo_root) => doctor_targets_from_status_report(status_with_config(
            repo_root,
            &StatusOptions::default(),
            config_override,
            TargetRootMode::RepoLocal,
        )?),
        None => doctor_targets_from_statuses(all_target_statuses(None, &enabled_targets)),
    };
    let global_targets = match (home_dir.as_deref(), repo_root.as_deref()) {
        (Some(_), Some(repo_root)) => doctor_targets_from_status_report(status_with_config(
            repo_root,
            &StatusOptions::default(),
            config_override,
            TargetRootMode::Global,
        )?),
        (Some(home_dir), None) => {
            notes.push(
                "repo root not detected; global managed counts for the current repository are unavailable"
                    .to_string(),
            );
            doctor_default_global_targets(home_dir, &enabled_targets)
        }
        (None, _) => Vec::new(),
    };

    Ok(DoctorReport {
        repo_root,
        home_dir,
        config_path: loaded.path,
        config_exists,
        repo_initialized,
        config: DoctorConfigReport {
            targets_agents: loaded.config.targets.agents,
            targets_claude: loaded.config.targets.claude,
            strategy: loaded.config.defaults.strategy,
        },
        source_roots,
        external_sources,
        managed_sources,
        repo_targets,
        global_targets,
        notes,
        warnings,
    })
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

fn strategy_label(strategy: Strategy) -> &'static str {
    match strategy {
        Strategy::Render => "render",
        Strategy::Symlink => "symlink",
    }
}

fn append_doctor_target_lines(lines: &mut Vec<String>, targets: &[DoctorTargetReport]) {
    if targets.is_empty() {
        lines.push("    - none".to_string());
        return;
    }

    for target in targets {
        let path = target
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| target.label.clone());
        let state = match target.state {
            DoctorTargetState::Linked => "linked",
            DoctorTargetState::NotLinked => "not linked",
            DoctorTargetState::Disabled => "disabled",
        };
        if matches!(target.state, DoctorTargetState::Disabled) {
            lines.push(format!("    - {path}: {state}"));
        } else {
            lines.push(format!(
                "    - {path}: {state} ({} managed skill(s))",
                target.managed_count
            ));
        }
    }
}

fn doctor_external_sources(
    repo_root: Option<&Path>,
    config: &Config,
    config_base_dir: Option<&Path>,
    notes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Vec<DoctorExternalSource> {
    let repo_slug = repo_root.map(repo_slug);
    config
        .external_sources
        .iter()
        .map(|external| {
            let resolved_path = if repo_slug.is_none() && external.path.contains("{repo}") {
                notes.push(format!(
                    "external source '{}' uses {{repo}} and cannot be fully resolved without a repo root",
                    external.name
                ));
                None
            } else {
                Some(resolve_external_root(
                    external,
                    repo_slug.as_deref().unwrap_or("repo"),
                    config_base_dir,
                ))
            };
            if let Some(path) = resolved_path.as_ref()
                && !path.is_dir()
            {
                warnings.push(format!(
                    "external source '{}' resolved path is missing: {}",
                    external.name,
                    path.display()
                ));
            }
            DoctorExternalSource {
                name: external.name.clone(),
                configured_path: external.path.clone(),
                resolved_path,
            }
        })
        .collect()
}

fn doctor_targets_from_status_report(report: StatusReport) -> Vec<DoctorTargetReport> {
    doctor_targets_from_statuses(report.target_statuses)
}

fn doctor_targets_from_statuses(statuses: Vec<TargetStatus>) -> Vec<DoctorTargetReport> {
    statuses
        .into_iter()
        .map(|status| DoctorTargetReport {
            label: status.kind.label().to_string(),
            path: status.path,
            state: match status.state {
                LinkState::Linked => DoctorTargetState::Linked,
                LinkState::NotLinked => DoctorTargetState::NotLinked,
                LinkState::Disabled => DoctorTargetState::Disabled,
            },
            managed_count: status.managed_count,
        })
        .collect()
}

fn doctor_default_global_targets(
    home_dir: &Path,
    enabled_targets: &[TargetKind],
) -> Vec<DoctorTargetReport> {
    [TargetKind::Agents, TargetKind::Claude]
        .into_iter()
        .map(|kind| DoctorTargetReport {
            label: kind.label().to_string(),
            path: Some(home_dir.join(kind.label())),
            state: if enabled_targets.contains(&kind) {
                DoctorTargetState::NotLinked
            } else {
                DoctorTargetState::Disabled
            },
            managed_count: 0,
        })
        .collect()
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
            path: None,
            base_dir: None,
        });
    };
    let base_dir = config_path.parent().map(Path::to_path_buf);

    if !config_path.exists() {
        return Ok(LoadedConfig {
            config: Config::default(),
            path: Some(config_path),
            base_dir,
        });
    }

    let raw = fs::read_to_string(&config_path).map_err(|source| SkillenvError::ReadFile {
        path: config_path.clone(),
        source,
    })?;
    let config = toml_edit::de::from_str(&raw).map_err(|source| SkillenvError::ParseConfig {
        path: config_path.clone(),
        source,
    })?;
    Ok(LoadedConfig {
        config,
        path: Some(config_path),
        base_dir,
    })
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

impl ScopeFilter {
    fn matches_scope(&self, scope: &str) -> bool {
        match self {
            Self::AllCurrentRepo => true,
            Self::Exact(scopes) => scopes.contains(scope),
        }
    }
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

    #[test]
    fn skill_inventory_lists_repo_local_tool_directories() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/research",
            Some(
                r#"---
name: research
description: repo agent
---
"#,
            ),
            "repo agent",
        )?;
        write_skill(
            repo.path(),
            ".claude/skills/review",
            Some(
                r#"---
name: review
description: repo claude
---
"#,
            ),
            "repo claude",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![
                    SkillInventoryTool::Codex,
                    SkillInventoryTool::Claude,
                    SkillInventoryTool::Opencode,
                    SkillInventoryTool::Antigravity,
                ],
                repo_tree: false,
            },
            Some(&config_path),
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.scope == "repository"
                && entry.name == "research"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Opencode
                && entry.scope == "repository"
                && entry.name == "research"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Antigravity
                && entry.scope == "workspace"
                && entry.name == "research"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Claude
                && entry.scope == "project"
                && entry.name == "review"
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_lists_user_and_global_roots() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            home.path(),
            ".agents/skills/user-agent",
            Some(
                r#"---
name: user-agent
---
"#,
            ),
            "user agent",
        )?;
        write_skill(
            home.path(),
            ".claude/skills/personal-review",
            Some(
                r#"---
name: personal-review
---
"#,
            ),
            "personal review",
        )?;
        write_skill(
            home.path(),
            ".config/opencode/skills/global-open",
            Some(
                r#"---
name: global-open
---
"#,
            ),
            "global open",
        )?;
        write_skill(
            home.path(),
            ".gemini/antigravity/skills/gravity",
            Some(
                r#"---
name: gravity
---
"#,
            ),
            "gravity",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![
                    SkillInventoryTool::Codex,
                    SkillInventoryTool::Claude,
                    SkillInventoryTool::Opencode,
                    SkillInventoryTool::Antigravity,
                ],
                repo_tree: false,
            },
            Some(&config_path),
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.scope == "user"
                && entry.name == "user-agent"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Claude
                && entry.scope == "user"
                && entry.name == "personal-review"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Opencode
                && entry.scope == "global"
                && entry.name == "global-open"
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Antigravity
                && entry.scope == "global"
                && entry.name == "gravity"
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_marks_rendered_skillenv_entries() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "render"
"#,
        )?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some(
                r#"---
description: rendered
---
"#,
            ),
            "rendered",
        )?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let generated_name = GeneratedNameLayout::for_mode(repo.path(), TargetRootMode::RepoLocal)
            .generated_name(&ScopeKey::Default, "research");
        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
            Some(&config_path),
        )?;
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == generated_name)
            .unwrap();
        assert!(entry.skillenv_managed);
        assert_eq!(entry.skillenv_origin, "repo:default");
        Ok(())
    }

    #[test]
    fn skill_inventory_marks_symlinked_skillenv_entries() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(
            repo.path(),
            r#"
[defaults]
strategy = "symlink"
"#,
        )?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some("plain body\n"),
            "plain",
        )?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let generated_name = GeneratedNameLayout::for_mode(repo.path(), TargetRootMode::RepoLocal)
            .generated_name(&ScopeKey::Default, "research");
        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
            Some(&config_path),
        )?;
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == generated_name)
            .unwrap();
        assert!(entry.skillenv_managed);
        assert_eq!(entry.skillenv_origin, "repo:default");
        Ok(())
    }

    #[test]
    fn skill_inventory_marks_codex_duplicates_as_visible() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_skill = r#"---
name: shared-skill
description: duplicate
---
"#;
        write_skill(
            repo.path(),
            ".agents/skills/repo-shared",
            Some(shared_skill),
            "repo shared",
        )?;
        write_skill(
            home.path(),
            ".agents/skills/user-shared",
            Some(shared_skill),
            "user shared",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
            Some(&config_path),
        )?;

        let duplicate_entries: Vec<_> = report
            .entries
            .iter()
            .filter(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == "shared-skill")
            .collect();
        assert_eq!(duplicate_entries.len(), 2);
        assert!(duplicate_entries.iter().all(|entry| {
            entry
                .status
                .contains(&SkillInventoryStatus::DuplicateVisible)
        }));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate visible skill 'shared-skill'"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_marks_shadowed_claude_project_skills() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_skill = r#"---
name: review
description: duplicate
---
"#;
        write_skill(
            repo.path(),
            ".claude/skills/project-review",
            Some(shared_skill),
            "project review",
        )?;
        write_skill(
            home.path(),
            ".claude/skills/user-review",
            Some(shared_skill),
            "user review",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Claude],
                repo_tree: false,
            },
            Some(&config_path),
        )?;

        let project_entry = report
            .entries
            .iter()
            .find(|entry| {
                entry.tool == SkillInventoryTool::Claude
                    && entry.scope == "project"
                    && entry.name == "review"
            })
            .unwrap();
        let user_entry = report
            .entries
            .iter()
            .find(|entry| {
                entry.tool == SkillInventoryTool::Claude
                    && entry.scope == "user"
                    && entry.name == "review"
            })
            .unwrap();
        assert!(
            project_entry
                .status
                .contains(&SkillInventoryStatus::Shadowed)
        );
        assert!(!user_entry.status.contains(&SkillInventoryStatus::Shadowed));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("shadowed visible skill 'review'"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_reports_invalid_skills_without_panicking() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/bad-frontmatter",
            Some(
                r#"---
name: [broken
---
"#,
            ),
            "broken",
        )?;
        ensure_dir(&repo.path().join(".agents/skills/missing-skill-md"))?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
            Some(&config_path),
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.name == "bad-frontmatter" && entry.status.contains(&SkillInventoryStatus::Invalid)
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.name == "missing-skill-md"
                && entry.status.contains(&SkillInventoryStatus::Invalid)
        }));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("invalid frontmatter"))
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("missing SKILL.md"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_repo_tree_marks_nested_skill_dirs() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            "apps/demo/.claude/skills/nested-review",
            Some(
                r#"---
name: nested-review
---
"#,
            ),
            "nested review",
        )?;
        write_skill(
            repo.path(),
            "tools/.agents/skills/nested-agent",
            Some(
                r#"---
name: nested-agent
---
"#,
            ),
            "nested agent",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Claude, SkillInventoryTool::Codex],
                repo_tree: true,
            },
            Some(&config_path),
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Claude
                && entry.name == "nested-review"
                && entry.discovery_state == SkillDiscoveryState::NestedOnDemand
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.name == "nested-agent"
                && entry.discovery_state == SkillDiscoveryState::RepoTreeOnly
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_repo_tree_marks_duplicates_outside_current_scope() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_skill = r#"---
name: shared
---
"#;
        write_skill(
            repo.path(),
            ".agents/skills/current-shared",
            Some(shared_skill),
            "current",
        )?;
        write_skill(
            repo.path(),
            "apps/demo/.agents/skills/nested-shared",
            Some(shared_skill),
            "nested",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: true,
            },
            Some(&config_path),
        )?;

        let duplicate_entries: Vec<_> = report
            .entries
            .iter()
            .filter(|entry| entry.tool == SkillInventoryTool::Codex && entry.name == "shared")
            .collect();
        assert_eq!(duplicate_entries.len(), 2);
        assert!(duplicate_entries.iter().all(|entry| {
            entry
                .status
                .contains(&SkillInventoryStatus::DuplicateVisible)
        }));
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate visible skill 'shared'"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_repo_tree_discovers_symlinked_skill_roots() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let shared_root = repo.path().join("skill-roots/shared-root");
        write_skill(
            repo.path(),
            "skill-roots/shared-root/symlinked-agent",
            Some(
                r#"---
name: symlinked-agent
---
"#,
            ),
            "symlinked agent",
        )?;
        ensure_dir(&repo.path().join("apps/demo/.agents"))?;
        create_symlink(&shared_root, &repo.path().join("apps/demo/.agents/skills")).unwrap();

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: true,
            },
            Some(&config_path),
        )?;

        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.name == "symlinked-agent"
                && entry.discovery_state == SkillDiscoveryState::RepoTreeOnly
        }));
        Ok(())
    }

    #[test]
    fn skill_inventory_outside_repo_still_lists_user_skills() -> Result<()> {
        let outside = Path::new("/");
        let config_path = PathBuf::from("/tmp/skillenv-missing.toml");
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            home.path(),
            ".agents/skills/user-only",
            Some(
                r#"---
name: user-only
---
"#,
            ),
            "user only",
        )?;

        let report = skill_inventory_with_config(
            outside,
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
            Some(&config_path),
        )?;

        assert!(report.repo_root.is_none());
        assert!(report.entries.iter().any(|entry| {
            entry.tool == SkillInventoryTool::Codex
                && entry.scope == "user"
                && entry.name == "user-only"
        }));
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("repo root not detected"))
        );
        Ok(())
    }

    #[test]
    fn skill_inventory_report_serializes_stable_shape() -> Result<()> {
        let repo = repo_fixture()?;
        let config_path = write_config(repo.path(), "")?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        write_skill(
            repo.path(),
            ".agents/skills/research",
            Some(
                r#"---
name: research
description: repo agent
---
"#,
            ),
            "repo agent",
        )?;

        let report = skill_inventory_with_config(
            repo.path(),
            &SkillInventoryOptions {
                tools: vec![SkillInventoryTool::Codex],
                repo_tree: false,
            },
            Some(&config_path),
        )?;
        let json = serde_json::to_value(&report).unwrap();

        assert!(json.get("repo_root").is_some());
        assert!(json.get("mode").is_some());
        assert!(json.get("tools").is_some());
        assert!(json.get("entries").is_some());
        assert!(json.get("notes").is_some());
        assert!(json.get("warnings").is_some());

        let entry = json
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
            .unwrap();
        assert!(entry.get("tool").is_some());
        assert!(entry.get("scope").is_some());
        assert!(entry.get("discovery_state").is_some());
        assert!(entry.get("name").is_some());
        assert!(entry.get("description").is_some());
        assert!(entry.get("skill_dir").is_some());
        assert!(entry.get("skill_md").is_some());
        assert!(entry.get("skillenv_managed").is_some());
        assert!(entry.get("skillenv_origin").is_some());
        assert!(entry.get("status").is_some());
        Ok(())
    }

    #[test]
    fn doctor_reports_config_path_and_external_sources() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let config_path = write_config(
            repo.path(),
            r#"
[targets]
claude = true

[defaults]
strategy = "symlink"

[[external_sources]]
name = "shared"
path = "../shared/{repo}"
"#,
        )?;
        let expected_external_root = resolve_external_root(
            &ExternalSourceConfig {
                name: "shared".to_string(),
                path: "../shared/{repo}".to_string(),
            },
            &test_repo_slug(repo.path()),
            config_path.parent(),
        );
        ensure_dir(&expected_external_root)?;

        let report = doctor_with_config(repo.path(), &DoctorOptions, Some(&config_path))?;

        assert_eq!(report.config_path, Some(config_path));
        assert!(report.config_exists);
        assert_eq!(report.repo_root, Some(repo.path().to_path_buf()));
        assert_eq!(report.home_dir, Some(home.path().to_path_buf()));
        assert!(!report.repo_initialized.unwrap());
        assert!(report.config.targets_agents);
        assert!(report.config.targets_claude);
        assert_eq!(report.config.strategy, Strategy::Symlink);
        assert_eq!(report.external_sources.len(), 1);
        assert_eq!(report.external_sources[0].name, "shared");
        assert_eq!(
            report.external_sources[0].resolved_path,
            Some(expected_external_root)
        );
        assert!(
            report
                .repo_targets
                .iter()
                .any(|target| target.label == ".agents/skills")
        );
        assert!(
            report
                .global_targets
                .iter()
                .any(|target| target.path == Some(home.path().join(".agents/skills")))
        );
        Ok(())
    }

    #[test]
    fn doctor_reports_managed_sources_with_transport_metadata() -> Result<()> {
        let repo = repo_fixture()?;
        let home = TempDir::new().unwrap();
        let _home = set_home_for_test(Some(home.path()));
        let config_path = write_config(repo.path(), "")?;
        write_skill(
            repo.path(),
            "skillenv/default/research",
            Some("repo doctor\n"),
            "repo doctor",
        )?;
        init_test_repo(repo.path(), &config_path)?;
        link_repo_with_config(repo.path(), &LinkOptions::default(), Some(&config_path))?;

        let install_root = repo.path().join("skillenv/remote/vercel");
        ensure_dir(&install_root)?;
        write_lock_file(
            repo.path(),
            format!(
                r#"{{
  "version": 1,
  "sources": [
    {{
      "name": "vercel",
      "source": "vercel-labs/agent-skills",
      "kind": "git",
      "transport": "https://github.com/vercel-labs/agent-skills.git",
      "requested_ref": "main",
      "subdir": null,
      "install_root": "{}",
      "selected_skills": ["frontend-design"],
      "resolved_revision": "abc123"
    }}
  ]
}}"#,
                install_root.strip_prefix(repo.path()).unwrap().display()
            ),
        )?;

        let report = doctor_with_config(repo.path(), &DoctorOptions, Some(&config_path))?;

        assert_eq!(report.managed_sources.len(), 1);
        let source = &report.managed_sources[0];
        assert_eq!(source.name, "vercel");
        assert_eq!(source.kind, "git");
        assert_eq!(source.source, "vercel-labs/agent-skills");
        assert_eq!(
            source.transport,
            "https://github.com/vercel-labs/agent-skills.git"
        );
        assert_eq!(source.requested_ref.as_deref(), Some("main"));
        assert_eq!(source.install_root, install_root);
        assert_eq!(source.selected_skills, vec!["frontend-design"]);
        assert_eq!(source.resolved_revision, "abc123");
        assert!(
            report
                .source_roots
                .iter()
                .any(|root| root.origin == "managed:vercel")
        );

        let rendered = format_doctor_report(&report);
        assert!(rendered.contains("transport=https://github.com/vercel-labs/agent-skills.git"));
        assert!(rendered.contains("source=vercel-labs/agent-skills"));
        Ok(())
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

    fn write_lock_file(repo_root: &Path, body: String) -> Result<()> {
        let path = repo_root.join("skillenv.lock.json");
        fs::write(&path, body).map_err(|source| SkillenvError::WriteFile { path, source })
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
