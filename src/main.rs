use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use skillenv::{
    DoctorOptions, InitOptions, LinkOptions, ScopeSelector, Shell, SkillInventoryOptions,
    SkillInventoryTool, StatusOptions, TargetOverride, UnlinkOptions, doctor, format_doctor_report,
    format_init_report, format_link_report, format_skill_inventory_report, format_status_report,
    hook_script, init_repo, link_global, link_repo, skill_inventory, status_global, status_repo,
    unlink_global, unlink_repo,
};

const ROOT_AFTER_HELP: &str = r#"Workflow:
  1. Run `skillenv init` once in each repository.
  2. Put repo-owned skills under `skillenv/default`, `skillenv/local`, or `skillenv/profiles/<profile>`.
  3. Run `skillenv link` to refresh `.agents/skills` and optional `.claude/skills` outputs.
  4. Use `skillenv add`, `skillenv fetch`, and `skillenv update` for managed sources recorded in `skillenv.lock.json`.

Repo layout:
  skillenv/
    default/
      <skill-name>/SKILL.md
    local/
      <skill-name>/SKILL.md
    profiles/
      <profile-name>/
        <skill-name>/SKILL.md
    remote/
      <source-name>/...

Naming rules:
  - Repository, profile, skill, and managed source names are normalized to kebab-case.
  - Repo-local generated names: `skillenv-<repo-slug>-<scope>-<skill-slug>`.
  - Global generated names: `skillenv-<repo-slug>-g<path-hash>-<scope>-<skill-slug>`.
  - Profile scopes appear as `profile:<name>` in status output and `profile-<name>` in generated names.

Examples:
  skillenv init
  skillenv link --profile review
  skillenv fetch
  skillenv doctor
  skillenv add vercel-labs/agent-skills --skill frontend-design
  skillenv update vercel
  skillenv global status
  skillenv version"#;

const INIT_AFTER_HELP: &str = r#"This command prepares repo-local skill outputs. It creates the layout below when missing
and updates `.gitignore` with the managed `skillenv` entries needed for generated targets.

Created layout:
  skillenv/
    default/
    local/
    profiles/

Use `default/` for shared repo skills, `local/` for repo-private skills, and
`profiles/<profile-name>/` for opt-in groups selected with `--profile`.

This command does not link skills by itself. Run `skillenv link` after adding skills."#;

const ADD_AFTER_HELP: &str = r#"Supported source forms:
  - GitHub shorthand: `owner/repo`
  - Git URL: `https://github.com/owner/repo` or `git@github.com:owner/repo.git`
  - Local checkout path: `../shared-skills`

Managed sources install under `skillenv/remote/<source-name>` by default, are recorded in
`skillenv.lock.json`, and then relink the current repository's default/local scopes."#;

const LINK_AFTER_HELP: &str = r#"Scope selection:
  - no flags: link `default/` and `local/`
  - `--profile name`: link only the named profile scope; repeat for multiple profiles
  - `--all`: link every discovered scope, including all profiles

Generated names follow `skillenv-<repo-slug>-<scope>-<skill-slug>` for repo-local targets."#;

const GLOBAL_AFTER_HELP: &str = r#"Global targets are fixed to:
  - `$HOME/.agents/skills`
  - `$HOME/.claude/skills`

Global commands do not require `skillenv init` and do not edit `.gitignore`.
Generated names include a stable path hash so multiple repositories can coexist safely."#;

const UPDATE_AFTER_HELP: &str = r#"When no managed source names are passed, every entry in `skillenv.lock.json` is refreshed.
Changed sources are reinstalled into their managed roots and default/local scopes are relinked."#;

const FETCH_AFTER_HELP: &str = r#"This command restores the managed install roots recorded in `skillenv.lock.json`.

It fetches Git sources at the locked `resolved_revision`, reinstalls the selected skills into
their managed roots, and relinks default/local scopes without modifying the lock file.

Use this on a new machine or after cleaning `skillenv/remote/` when only the lock file is
checked into Git."#;

const SKILLS_AFTER_HELP: &str = r#"Discovery targets:
  - codex: current repo `.agents/skills`, `$HOME/.agents/skills`, `/etc/codex/skills`
  - claude: current repo `.claude/skills`, `$HOME/.claude/skills`
  - opencode: current repo `.opencode/skills`, `.claude/skills`, `.agents/skills`, plus `$HOME` global paths
  - antigravity: repo-root `.agents/skills`, legacy `.agent/skills`, `$HOME/.gemini/antigravity/skills`

Behavior:
  - default mode reports the custom skills visible from the current working directory
  - `--repo-tree` adds repo-wide inventory for nested tool directories that are not currently visible
  - `--json` prints a stable machine-readable report"#;

const DOCTOR_AFTER_HELP: &str = r#"This command prints detailed diagnostics for the current repository setup.

It includes:
  - detected repo root and HOME
  - the config file path and whether it exists
  - enabled targets and default strategy
  - resolved external source directories
  - managed source metadata from `skillenv.lock.json`, including source and transport URLs
  - repo-local and global target status"#;

#[derive(Debug, Parser)]
#[command(name = "skillenv")]
#[command(version)]
#[command(arg_required_else_help = true)]
#[command(about = "Manage repo-local and remote AI skills for agent-facing skill directories.")]
#[command(
    long_about = "Manage repo-local skills, managed remote skill sources, and generated links for agent-facing skill directories such as `.agents/skills` and `.claude/skills`."
)]
#[command(after_long_help = ROOT_AFTER_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Install a managed skill source and record it in skillenv.lock.json.",
        after_long_help = ADD_AFTER_HELP
    )]
    Add,
    #[command(
        about = "Create the repo-local skillenv layout and managed .gitignore entries.",
        after_long_help = INIT_AFTER_HELP
    )]
    Init(TargetArgs),
    #[command(
        about = "Link repo-local skills into this repository's target skill directories.",
        after_long_help = LINK_AFTER_HELP
    )]
    Link(ScopeArgs),
    #[command(
        about = "Remove generated skill links for the selected repo-local scopes.",
        after_long_help = LINK_AFTER_HELP
    )]
    Unlink(ScopeArgs),
    #[command(about = "Show repo-local target status and the number of managed entries.")]
    Status(TargetArgs),
    #[command(
        about = "List every skill the manifest declares, with its source and labels.",
        long_about = "Requires a skillenv.toml manifest. Shows each skill's source, the \
                      [[source]] entry that contributed it, its labels, and the locked \
                      revision when there is one."
    )]
    List,
    #[command(
        about = "Describe how this v0 setup would convert to a skillenv.toml manifest.",
        long_about = "Reads only. Reports the skills, sources, and deploy rules a v1 \
                      manifest would carry, the v0 deployments that must be cleared \
                      first, and the proposed manifest itself. Nothing is written."
    )]
    Migrate(MigrateArgs),
    #[command(
        about = "Compare the lock against what each remote ref points at now.",
        long_about = "Requires a skillenv.toml manifest. Reads only: contacts each remote \
                      with git ls-remote and touches neither the cache nor the lock."
    )]
    Outdated,
    #[command(
        about = "Scan the manifest's skills for hidden instructions and unsafe patterns.",
        long_about = "Requires a skillenv.toml manifest. Reports findings using Snyk's \
                      agent-scan codes and exits non-zero when anything is found."
    )]
    Lint,
    #[command(
        about = "Manually manage generated skill links under $HOME targets.",
        after_long_help = GLOBAL_AFTER_HELP
    )]
    Global {
        #[command(subcommand)]
        command: GlobalCommand,
    },
    #[command(
        about = "Refresh managed sources recorded in skillenv.lock.json and relink them.",
        after_long_help = UPDATE_AFTER_HELP
    )]
    Update(ManagedSourceArgs),
    #[command(
        about = "Restore managed sources from skillenv.lock.json using the locked revisions.",
        after_long_help = FETCH_AFTER_HELP
    )]
    Fetch(ManagedSourceArgs),
    #[command(
        about = "List tool-visible custom skills across repository and home directories.",
        after_long_help = SKILLS_AFTER_HELP
    )]
    Skills(SkillsArgs),
    #[command(
        about = "Show detailed diagnostics for config, sources, and targets.",
        after_long_help = DOCTOR_AFTER_HELP
    )]
    Doctor(DoctorArgs),
    #[command(about = "Print a shell hook that runs `skillenv link --quiet` on repo changes.")]
    Hook {
        #[command(subcommand)]
        shell: HookCommand,
    },
    #[command(about = "Print the skillenv CLI version.")]
    Version,
}

#[derive(Debug, Args)]
struct ScopeArgs {
    #[arg(
        long = "profile",
        help = "Profile scope name to operate on. Repeat to select multiple profiles."
    )]
    profiles: Vec<String>,
    #[arg(
        long,
        conflicts_with = "profiles",
        help = "Operate on every discovered scope, including all profiles."
    )]
    all: bool,
    #[arg(
        long,
        conflicts_with = "no_claude",
        help = "Also target `.claude/skills`."
    )]
    claude: bool,
    #[arg(
        long,
        conflicts_with = "claude",
        help = "Disable `.claude/skills` even if config enables it."
    )]
    no_claude: bool,
    #[arg(long, help = "Suppress normal output. Useful from shell hooks.")]
    quiet: bool,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    /// Carry out the conversion. Without this, nothing is written.
    #[arg(long)]
    apply: bool,
    /// With --apply, also remove the old skillenv/ layout.
    #[arg(long, requires = "apply")]
    prune: bool,
}

#[derive(Debug, Args)]
struct TargetArgs {
    #[arg(
        long,
        conflicts_with = "no_claude",
        help = "Also target `.claude/skills`."
    )]
    claude: bool,
    #[arg(
        long,
        conflicts_with = "claude",
        help = "Disable `.claude/skills` even if config enables it."
    )]
    no_claude: bool,
}

#[derive(Debug, Args)]
struct ManagedSourceArgs {
    /// With a manifest: move to whatever each ref points at now, instead of
    /// restoring the locked revision.
    #[arg(long)]
    update: bool,
    #[arg(help = "Managed source names to operate on. Defaults to every recorded source.")]
    names: Vec<String>,
    #[arg(
        long,
        conflicts_with = "no_claude",
        help = "Also target `.claude/skills` for the follow-up relink."
    )]
    claude: bool,
    #[arg(
        long,
        conflicts_with = "claude",
        help = "Disable `.claude/skills` even if config enables it."
    )]
    no_claude: bool,
}

#[derive(Debug, Args)]
struct SkillsArgs {
    #[arg(
        long = "tool",
        value_enum,
        help = "Limit output to one or more tools. Repeat to keep multiple tools."
    )]
    tools: Vec<SkillToolArg>,
    #[arg(
        long,
        help = "Also scan nested tool directories across the repository tree."
    )]
    repo_tree: bool,
    #[arg(long, help = "Print a JSON report instead of human-readable text.")]
    json: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long, help = "Print a JSON report instead of human-readable text.")]
    json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SkillToolArg {
    Claude,
    Codex,
    Opencode,
    Antigravity,
}

#[derive(Debug, Subcommand)]
enum HookCommand {
    #[command(about = "Print a zsh hook that uses `add-zsh-hook` and runs on directory changes.")]
    Zsh,
    #[command(about = "Print a bash hook that uses `PROMPT_COMMAND` and runs on repo changes.")]
    Bash,
}

#[derive(Debug, Subcommand)]
enum GlobalCommand {
    #[command(about = "Link repo-local skills into global `$HOME` targets.")]
    Link(ScopeArgs),
    #[command(about = "Remove generated links from global `$HOME` targets.")]
    Unlink(ScopeArgs),
    #[command(about = "Show global target status under `$HOME`.")]
    Status(TargetArgs),
}

/// What a command produced.
///
/// `warnings` is separate from `stdout` on purpose: it goes to stderr even when
/// the caller asked for silence. `skillenv link --quiet` is what the shell hook
/// runs, and a skill that failed to deploy must not be invisible there — that
/// silence is how the original outage went unnoticed for six weeks.
struct CommandOutput {
    stdout: String,
    warnings: Vec<String>,
    problems: bool,
}

impl CommandOutput {
    fn text(stdout: String) -> Self {
        Self {
            stdout,
            warnings: Vec::new(),
            problems: false,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // A repository carrying skillenv.toml uses the v1 engine; one still on the
    // v0 layout keeps working unchanged. Replacing the binary must not break a
    // setup that has not migrated yet.
    let result = match dispatch_manifest(&cli) {
        Some(result) => result,
        None => run(cli).map(CommandOutput::text),
    };

    match result {
        Ok(output) => {
            if !output.stdout.is_empty() {
                println!("{}", output.stdout);
            }
            for warning in &output.warnings {
                eprintln!("{warning}");
            }
            if output.problems {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Handle a command against a v1 manifest, or `None` to fall through to the v0
/// path.
fn dispatch_manifest(cli: &Cli) -> Option<skillenv::Result<CommandOutput>> {
    match &cli.command {
        Command::Link(args) if skillenv::has_manifest(".") => {
            Some(link_manifest_command(args.quiet))
        }
        Command::List => Some(skillenv::list_manifest(".").map(CommandOutput::text)),
        // Being out of date is a state to report, not a failure, so this exits 0
        // either way. A CI job that wants to fail on staleness can match the output.
        Command::Outdated => Some(
            skillenv::outdated_manifest(".").map(|(stdout, _stale)| CommandOutput::text(stdout)),
        ),
        Command::Fetch(args) if skillenv::has_manifest(".") => Some(
            skillenv::fetch_manifest(".", args.update).map(|(stdout, warnings, problems)| {
                CommandOutput {
                    stdout,
                    warnings,
                    problems,
                }
            }),
        ),
        Command::Migrate(args) => Some(if args.apply {
            skillenv::apply_migration(".", args.prune).map(CommandOutput::text)
        } else {
            skillenv::plan_migration(".").map(CommandOutput::text)
        }),
        Command::Lint => {
            Some(
                skillenv::lint_manifest(".").map(|(stdout, problems)| CommandOutput {
                    stdout,
                    warnings: Vec::new(),
                    problems,
                }),
            )
        }
        _ => None,
    }
}

fn link_manifest_command(quiet: bool) -> skillenv::Result<CommandOutput> {
    let report = skillenv::link_manifest(".")?;
    Ok(CommandOutput {
        stdout: if quiet {
            String::new()
        } else {
            skillenv::format_link_manifest_report(&report)
        },
        // Deliberately not gated on `quiet`.
        warnings: report.warnings(),
        problems: report.has_problems(),
    })
}

fn run(cli: Cli) -> skillenv::Result<String> {
    match cli.command {
        // v0 installed sources into skillenv/remote and recorded them in
        // skillenv.lock.json. In v1 a source is declared in the manifest, which is
        // a file edit rather than a command, so this points at that instead of
        // half-doing it.
        Command::Add => Err(skillenv::SkillenvError::InvalidSource {
            input: "add".to_string(),
            message: "v1 declares sources in skillenv.toml. Add a [[source]] entry \
                      (name, from, skills) and run `skillenv fetch`"
                .to_string(),
        }),
        Command::Init(args) => {
            let report = init_repo(
                ".",
                InitOptions {
                    claude: target_override(args.claude, args.no_claude),
                },
            )?;
            Ok(format_init_report(&report))
        }
        Command::Link(args) => {
            let report = link_repo(
                ".",
                LinkOptions {
                    selector: scope_selector(&args),
                    claude: target_override(args.claude, args.no_claude),
                    quiet: args.quiet,
                },
            )?;
            Ok(if args.quiet {
                String::new()
            } else {
                format_link_report(&report, "linked")
            })
        }
        Command::Unlink(args) => {
            let report = unlink_repo(
                ".",
                UnlinkOptions {
                    selector: scope_selector(&args),
                    claude: target_override(args.claude, args.no_claude),
                    quiet: args.quiet,
                },
            )?;
            Ok(if args.quiet {
                String::new()
            } else {
                format_link_report(&report, "unlinked")
            })
        }
        Command::Status(args) => {
            let report = status_repo(
                ".",
                StatusOptions {
                    claude: target_override(args.claude, args.no_claude),
                },
            )?;
            Ok(format_status_report(&report))
        }
        // These only exist against a manifest. Reached only when
        // `dispatch_manifest` declined, i.e. there is no skillenv.toml, so the
        // error explains what is missing rather than silently doing nothing.
        Command::Migrate(_) => unreachable!("handled by dispatch_manifest"),
        Command::Outdated => Err(skillenv::SkillenvError::ManifestNotFound {
            searched_from: std::path::PathBuf::from("."),
        }),
        Command::List | Command::Lint => Err(skillenv::SkillenvError::ManifestNotFound {
            searched_from: std::path::PathBuf::from("."),
        }),
        Command::Global {
            command: GlobalCommand::Link(args),
        } => {
            let report = link_global(
                ".",
                LinkOptions {
                    selector: scope_selector(&args),
                    claude: target_override(args.claude, args.no_claude),
                    quiet: args.quiet,
                },
            )?;
            Ok(if args.quiet {
                String::new()
            } else {
                format_link_report(&report, "linked")
            })
        }
        Command::Global {
            command: GlobalCommand::Unlink(args),
        } => {
            let report = unlink_global(
                ".",
                UnlinkOptions {
                    selector: scope_selector(&args),
                    claude: target_override(args.claude, args.no_claude),
                    quiet: args.quiet,
                },
            )?;
            Ok(if args.quiet {
                String::new()
            } else {
                format_link_report(&report, "unlinked")
            })
        }
        Command::Global {
            command: GlobalCommand::Status(args),
        } => {
            let report = status_global(
                ".",
                StatusOptions {
                    claude: target_override(args.claude, args.no_claude),
                },
            )?;
            Ok(format_status_report(&report))
        }
        Command::Update(_) => Err(skillenv::SkillenvError::InvalidSource {
            input: "update".to_string(),
            message: "use `skillenv fetch --update`, or `skillenv outdated` first to \
                      see what would move"
                .to_string(),
        }),
        // Reached only when dispatch_manifest declined, i.e. there is no manifest.
        // v0 restored into skillenv/remote from skillenv.lock.json; there is nothing
        // to restore without a v1 lock.
        Command::Fetch(_) => Err(skillenv::SkillenvError::ManifestNotFound {
            searched_from: std::path::PathBuf::from("."),
        }),
        Command::Skills(args) => {
            let report = skill_inventory(
                ".",
                SkillInventoryOptions {
                    tools: args.tools.into_iter().map(inventory_tool).collect(),
                    repo_tree: args.repo_tree,
                },
            )?;
            if args.json {
                serde_json::to_string_pretty(&report).map_err(|source| {
                    skillenv::SkillenvError::SerializeLock {
                        path: std::path::PathBuf::from("stdout"),
                        source,
                    }
                })
            } else {
                Ok(format_skill_inventory_report(&report))
            }
        }
        Command::Doctor(args) => {
            let report = doctor(".", DoctorOptions)?;
            if args.json {
                serde_json::to_string_pretty(&report).map_err(|source| {
                    skillenv::SkillenvError::SerializeLock {
                        path: std::path::PathBuf::from("stdout"),
                        source,
                    }
                })
            } else {
                Ok(format_doctor_report(&report))
            }
        }
        Command::Hook {
            shell: HookCommand::Zsh,
        } => Ok(hook_script(Shell::Zsh)),
        Command::Hook {
            shell: HookCommand::Bash,
        } => Ok(hook_script(Shell::Bash)),
        Command::Version => Ok(format!("skillenv {}", env!("CARGO_PKG_VERSION"))),
    }
}

fn scope_selector(args: &ScopeArgs) -> ScopeSelector {
    if args.all {
        ScopeSelector::All
    } else if args.profiles.is_empty() {
        ScopeSelector::DefaultLocal
    } else {
        ScopeSelector::Profiles(args.profiles.clone())
    }
}

fn target_override(claude: bool, no_claude: bool) -> TargetOverride {
    if claude {
        TargetOverride::ForceEnabled
    } else if no_claude {
        TargetOverride::ForceDisabled
    } else {
        TargetOverride::UseConfig
    }
}

fn inventory_tool(tool: SkillToolArg) -> SkillInventoryTool {
    match tool {
        SkillToolArg::Claude => SkillInventoryTool::Claude,
        SkillToolArg::Codex => SkillInventoryTool::Codex,
        SkillToolArg::Opencode => SkillInventoryTool::Opencode,
        SkillToolArg::Antigravity => SkillInventoryTool::Antigravity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_configuration_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn root_help_describes_workflow_and_naming() {
        let mut command = Cli::command();
        let mut buffer = Vec::new();
        command.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("Manage repo-local skills, managed remote skill sources"));
        assert!(help.contains("Repo layout:"));
        assert!(help.contains("skillenv-<repo-slug>-<scope>-<skill-slug>"));
        assert!(help.contains("skillenv version"));
    }

    #[test]
    fn init_help_describes_created_layout() {
        let mut command = Cli::command();
        let init = command.find_subcommand_mut("init").unwrap();
        let mut buffer = Vec::new();
        init.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("Create the repo-local skillenv layout"));
        assert!(help.contains("Created layout:"));
        assert!(help.contains("profiles/<profile-name>/"));
        assert!(help.contains("Run `skillenv link`"));
    }

    #[test]
    fn skills_help_describes_discovery_targets() {
        let mut command = Cli::command();
        let skills = command.find_subcommand_mut("skills").unwrap();
        let mut buffer = Vec::new();
        skills.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("List tool-visible custom skills"));
        assert!(help.contains("codex: current repo `.agents/skills`"));
        assert!(help.contains("--repo-tree"));
        assert!(help.contains("--json"));
    }

    #[test]
    fn doctor_help_describes_diagnostics() {
        let mut command = Cli::command();
        let doctor = command.find_subcommand_mut("doctor").unwrap();
        let mut buffer = Vec::new();
        doctor.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("Show detailed diagnostics"));
        assert!(help.contains("config file path"));
        assert!(help.contains("transport URLs"));
        assert!(help.contains("--json"));
    }

    #[test]
    fn fetch_help_describes_locked_revision_restore() {
        let mut command = Cli::command();
        let fetch = command.find_subcommand_mut("fetch").unwrap();
        let mut buffer = Vec::new();
        fetch.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("locked `resolved_revision`"));
        assert!(help.contains("without modifying the lock file"));
        assert!(help.contains("new machine"));
    }

    #[test]
    fn version_command_returns_package_version() {
        let output = run(Cli {
            command: Command::Version,
        })
        .unwrap();
        assert_eq!(output, format!("skillenv {}", env!("CARGO_PKG_VERSION")));
    }
}
