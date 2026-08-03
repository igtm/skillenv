use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use skillenv::{
    Shell, SkillInventoryOptions, SkillInventoryTool, format_skill_inventory_report, hook_script,
    skill_inventory,
};

const ROOT_AFTER_HELP: &str = r#"Workflow:
  1. Run `skillenv init` once, wherever you want the manifest to live — typically a dotfiles repository.
  2. Declare your skills, their sources, and where they go, in `skillenv.toml`.
  3. Run `skillenv fetch` to populate the cache, then `skillenv link` to deploy.
  4. Use `skillenv outdated` to see what has moved upstream, and `skillenv lint` before trusting new material.

Layout:
  skillenv.toml            the one hand-written file
  skillenv.lock            what each source resolved to; commit this
  skills/<name>/SKILL.md   skills you write yourself
  .skillenv/cache/         fetched sources; not committed

Naming:
  - A skill id is `[a-z0-9-]`, at most 32 characters, and unique across every source.
  - Generated directories are `skillenv-<repo>-<id>` in a repo target, and
    `skillenv-<repo>-g<hash>-<id>` under `$HOME`, where the hash distinguishes repositories
    sharing one home directory.

Examples:
  skillenv init
  skillenv fetch
  skillenv link
  skillenv status
  skillenv outdated
  skillenv lint
  skillenv doctor
  skillenv version"#;

const INIT_AFTER_HELP: &str = r#"Creates, when missing:
  skillenv.toml   a commented template with one deploy rule
  skills/         where your own skills go

and adds the managed `skillenv` entries to `.gitignore`, so the cache and any generated
directories stay out of `git status`.

An existing `skillenv.toml` is never overwritten: it is the only hand-written input.

This command does not deploy anything. Run `skillenv link` once you have declared a skill."#;

const LINK_AFTER_HELP: &str = r#"Deploys every skill each `[[deploy]]` rule selects, into the directory that rule names.

Rules resolving to the same directory have their selections unioned, so two rules cannot
take turns removing each other's work. A rule with `when.repo` applies only inside that
repository, which is what makes running this from a directory-change hook useful.

Failure is per skill: a malformed `SKILL.md`, a name collision, or a skill held back by the
safeguard is reported and skipped, and the rest still deploy. Only a systemic I/O failure
stops the run.

Warnings go to stderr and the exit code is non-zero on a problem **even under `--quiet`**,
which is the form the shell hook runs."#;

const STATUS_AFTER_HELP: &str = r#"Reports every `skillenv-` directory in each target this manifest deploys to, including
directories belonging to a different manifest and directories carrying the prefix but no
marker. Those are never removed — without a marker there is no evidence skillenv created
them — and hiding them would make the count disagree with `ls`.

A skill a rule selects but that is not on disk is listed by name. The usual cause is a
cache that was never fetched."#;

const FETCH_AFTER_HELP: &str = r#"Populates `.skillenv/cache/` for every remote source the manifest declares.

Without `--update`, restores exactly the revisions `skillenv.lock` records. That is what a
fresh clone needs: the cache is not committed, so a new machine has the manifest and the
lock and nothing else.

With `--update`, moves to whatever each ref points at now and rewrites the lock. Run
`skillenv outdated` first to see what would move.

The lock is saved after each source rather than once at the end, so an unreachable source
part-way through cannot leave the installed trees and the recorded revisions disagreeing."#;

const SKILLS_AFTER_HELP: &str = r#"Discovery targets:
  - codex: current repo `.agents/skills`, `$HOME/.agents/skills`, `/etc/codex/skills`
  - claude: current repo `.claude/skills`, `$HOME/.claude/skills`
  - opencode: current repo `.opencode/skills`, `.claude/skills`, `.agents/skills`, plus `$HOME` global paths
  - antigravity: repo-root `.agents/skills`, legacy `.agent/skills`, `$HOME/.gemini/antigravity/skills`

Behavior:
  - default mode reports the custom skills visible from the current working directory
  - `--repo-tree` adds repo-wide inventory for nested tool directories that are not currently visible
  - `--json` prints a stable machine-readable report

This reports what each tool can see, managed or not. Use `skillenv status` for what this
manifest put there."#;

const DOCTOR_AFTER_HELP: &str = r#"Answers "why did it go there", where `status` answers "what is deployed".

It reports:
  - which `skillenv.toml` governs this directory, and the repository it resolved
  - the home directory and the cache path, with how many sources are cached
  - how many skills and deploy rules the manifest declares, and how many the lock records
  - each resolved target, its provider, and how many deployments it holds

`--json` prints the same information in a stable shape."#;

#[derive(Debug, Parser)]
#[command(name = "skillenv")]
#[command(version)]
#[command(arg_required_else_help = true)]
#[command(about = "Declare AI skills once, deploy them to every agent's skill directory.")]
#[command(
    long_about = "Acquire, version, and deploy agent skills from a single `skillenv.toml`. Skills come from your own `skills/` directory, GitHub repositories, gists, or local paths, and are deployed into the directories each agent reads — `.claude/skills`, `.agents/skills`, `$CODEX_HOME/skills`, `.opencode/skills` — with frontmatter rewritten per provider and every skill scanned before it is written."
)]
#[command(after_long_help = ROOT_AFTER_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Create skillenv.toml, skills/, and the managed .gitignore entries.",
        after_long_help = INIT_AFTER_HELP
    )]
    Init,
    #[command(
        about = "Deploy the manifest's skills into every target its rules select.",
        after_long_help = LINK_AFTER_HELP
    )]
    Link(QuietArgs),
    #[command(
        about = "Remove every deployment belonging to this manifest.",
        long_about = "Removes only directories whose marker names this manifest. A directory \
                      carrying the prefix without a marker, or with another manifest's, is \
                      reported and left in place."
    )]
    Unlink(QuietArgs),
    #[command(
        about = "Drop a skill or source from the manifest and clear its deployments.",
        long_about = "Edits skillenv.toml in place, keeping every comment and the order of \
                      what remains, then relinks so the removed entry's directories go with \
                      it. Naming a [[source]] removes every skill it contributed."
    )]
    Remove(RemoveArgs),
    #[command(
        about = "Show what this manifest has deployed, in each of its targets.",
        after_long_help = STATUS_AFTER_HELP
    )]
    Status,
    #[command(
        about = "List every skill the manifest declares, with its source and labels.",
        long_about = "Shows each skill's source, the [[source]] entry that contributed it, its \
                      labels, and the locked revision when there is one."
    )]
    List,
    #[command(
        about = "Convert a v0 skillenv/ layout to a skillenv.toml manifest.",
        long_about = "Without --apply, reads only: reports the skills, sources, and deploy rules \
                      a v1 manifest would carry, the v0 deployments that must be cleared first, \
                      and the proposed manifest itself. Nothing is written."
    )]
    Migrate(MigrateArgs),
    #[command(
        about = "Compare the lock against what each remote ref points at now.",
        long_about = "Reads only: contacts each remote with git ls-remote and touches neither \
                      the cache nor the lock. Being out of date is a state, not a failure, so \
                      this exits 0 either way."
    )]
    Outdated,
    #[command(
        about = "Scan the manifest's skills for hidden instructions and unsafe patterns.",
        long_about = "Reports findings using Snyk's agent-scan codes and exits non-zero when \
                      anything is found. `link` runs the same checks and blocks on critical \
                      findings; this is how to see them before deploying."
    )]
    Lint,
    #[command(
        about = "Populate the cache for every remote source the manifest declares.",
        after_long_help = FETCH_AFTER_HELP
    )]
    Fetch(FetchArgs),
    #[command(
        about = "List tool-visible custom skills across repository and home directories.",
        after_long_help = SKILLS_AFTER_HELP
    )]
    Skills(SkillsArgs),
    #[command(
        about = "Show how this invocation resolved: manifest, cache, and targets.",
        after_long_help = DOCTOR_AFTER_HELP
    )]
    Doctor(JsonArgs),
    #[command(about = "Print a shell hook that runs `skillenv link --quiet` on repo changes.")]
    Hook {
        #[command(subcommand)]
        shell: HookCommand,
    },
    #[command(about = "Print the skillenv CLI version.")]
    Version,
}

#[derive(Debug, Args)]
struct QuietArgs {
    #[arg(
        long,
        help = "Suppress normal output. Warnings still go to stderr. Useful from shell hooks."
    )]
    quiet: bool,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    #[arg(help = "Name of the [[skill]] or [[source]] entry to remove.")]
    name: String,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    /// Carry out the conversion. Without this, nothing is written.
    #[arg(long)]
    apply: bool,
    /// Remove the old skillenv/ layout. Usable on its own after migrating, which
    /// is the order that makes sense: migrate, check the result, then discard.
    #[arg(long)]
    prune: bool,
}

#[derive(Debug, Args)]
struct FetchArgs {
    /// Move to whatever each ref points at now, instead of restoring the locked
    /// revision.
    #[arg(long)]
    update: bool,
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
struct JsonArgs {
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
    match run(Cli::parse()) {
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

fn run(cli: Cli) -> skillenv::Result<CommandOutput> {
    match cli.command {
        Command::Init => skillenv::init_manifest(".").map(CommandOutput::text),
        Command::Link(args) => {
            // Standing outside a managed tree is the normal state for a shell hook,
            // not a failure. `--quiet` is the form the hook runs, so there it exits
            // silently; typed by hand it still says what is missing.
            if args.quiet && !skillenv::has_manifest(".") {
                return Ok(CommandOutput::text(String::new()));
            }
            let report = skillenv::link_manifest(".")?;
            Ok(CommandOutput {
                stdout: if args.quiet {
                    String::new()
                } else {
                    skillenv::format_link_manifest_report(&report)
                },
                // Deliberately not gated on `quiet`.
                warnings: report.warnings(),
                problems: report.has_problems(),
            })
        }
        Command::Unlink(args) => {
            let report = skillenv::unlink_manifest(".")?;
            Ok(CommandOutput {
                stdout: if args.quiet {
                    String::new()
                } else {
                    skillenv::format_link_manifest_report(&report)
                },
                warnings: report.warnings(),
                problems: report.has_problems(),
            })
        }
        Command::Remove(args) => {
            skillenv::remove_from_manifest(".", &args.name).map(|report| CommandOutput {
                stdout: report.summary,
                warnings: report.warnings,
                problems: report.problems,
            })
        }
        Command::Status => skillenv::status_manifest(".").map(|(stdout, problems)| CommandOutput {
            stdout,
            warnings: Vec::new(),
            problems,
        }),
        Command::List => skillenv::list_manifest(".").map(CommandOutput::text),
        Command::Migrate(args) => match (args.apply, args.prune) {
            (true, prune) => skillenv::apply_migration(".", prune).map(CommandOutput::text),
            // --prune alone acts on a repository that has already been migrated.
            (false, true) => skillenv::prune_legacy_layout(".").map(CommandOutput::text),
            (false, false) => skillenv::plan_migration(".").map(CommandOutput::text),
        },
        // Being out of date is a state to report, not a failure, so this exits 0
        // either way. A CI job that wants to fail on staleness can match the output.
        Command::Outdated => {
            skillenv::outdated_manifest(".").map(|(stdout, _stale)| CommandOutput::text(stdout))
        }
        Command::Lint => skillenv::lint_manifest(".").map(|(stdout, problems)| CommandOutput {
            stdout,
            warnings: Vec::new(),
            problems,
        }),
        Command::Fetch(args) => {
            skillenv::fetch_manifest(".", args.update).map(|(stdout, warnings, problems)| {
                CommandOutput {
                    stdout,
                    warnings,
                    problems,
                }
            })
        }
        Command::Skills(args) => {
            let report = skill_inventory(
                ".",
                SkillInventoryOptions {
                    tools: args.tools.into_iter().map(inventory_tool).collect(),
                    repo_tree: args.repo_tree,
                },
            )?;
            let stdout = if args.json {
                serde_json::to_string_pretty(&report).map_err(|source| {
                    skillenv::SkillenvError::SerializeLock {
                        path: std::path::PathBuf::from("stdout"),
                        source,
                    }
                })?
            } else {
                format_skill_inventory_report(&report)
            };
            Ok(CommandOutput::text(stdout))
        }
        Command::Doctor(args) => skillenv::doctor_manifest(".", args.json).map(CommandOutput::text),
        Command::Hook {
            shell: HookCommand::Zsh,
        } => Ok(CommandOutput::text(hook_script(Shell::Zsh))),
        Command::Hook {
            shell: HookCommand::Bash,
        } => Ok(CommandOutput::text(hook_script(Shell::Bash))),
        Command::Version => Ok(CommandOutput::text(format!(
            "skillenv {}",
            env!("CARGO_PKG_VERSION")
        ))),
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
        assert!(help.contains("Acquire, version, and deploy agent skills"));
        assert!(help.contains("Layout:"));
        assert!(help.contains("skillenv-<repo>-g<hash>-<id>"));
        assert!(help.contains("skillenv version"));
    }

    #[test]
    fn init_help_describes_created_layout() {
        let mut command = Cli::command();
        let init = command.find_subcommand_mut("init").unwrap();
        let mut buffer = Vec::new();
        init.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("Creates, when missing:"));
        assert!(help.contains("skillenv.toml"));
        assert!(help.contains("never overwritten"));
        assert!(help.contains("Run `skillenv link`"));
    }

    /// The distinction between the two listing commands is the one users get
    /// wrong, so each help text has to draw it.
    #[test]
    fn skills_and_status_help_distinguish_themselves() {
        let mut command = Cli::command();
        let mut skills = Vec::new();
        command
            .find_subcommand_mut("skills")
            .unwrap()
            .write_long_help(&mut skills)
            .unwrap();
        let skills = String::from_utf8(skills).unwrap();
        assert!(skills.contains("managed or not"));
        assert!(skills.contains("`skillenv status` for what this"));

        let mut status = Vec::new();
        command
            .find_subcommand_mut("status")
            .unwrap()
            .write_long_help(&mut status)
            .unwrap();
        let status = String::from_utf8(status).unwrap();
        assert!(status.contains("never removed"));
        assert!(status.contains("selects but that is not on disk"));
    }

    #[test]
    fn doctor_help_describes_diagnostics() {
        let mut command = Cli::command();
        let doctor = command.find_subcommand_mut("doctor").unwrap();
        let mut buffer = Vec::new();
        doctor.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("why did it go there"));
        assert!(help.contains("how many sources are cached"));
        assert!(help.contains("--json"));
    }

    #[test]
    fn fetch_help_describes_locked_revision_restore() {
        let mut command = Cli::command();
        let fetch = command.find_subcommand_mut("fetch").unwrap();
        let mut buffer = Vec::new();
        fetch.write_long_help(&mut buffer).unwrap();
        let help = String::from_utf8(buffer).unwrap();
        assert!(help.contains("skillenv.lock` records"));
        assert!(help.contains("fresh clone"));
        assert!(help.contains("saved after each source"));
    }

    /// The hook runs `link --quiet` on every directory change, and most directories
    /// are not under a manifest. Erroring there would print on every `cd`, and the
    /// hook would be removed. Typed by hand the same command still explains itself.
    #[test]
    fn quiet_link_outside_a_managed_tree_is_a_silent_no_op() {
        // Mutates process-global cwd, so nothing else in this binary may depend on
        // it. Everything else here only renders help text.
        let dir = tempfile::TempDir::new().unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let quiet = run(Cli {
            command: Command::Link(QuietArgs { quiet: true }),
        });
        let loud = run(Cli {
            command: Command::Link(QuietArgs { quiet: false }),
        });

        std::env::set_current_dir(previous).unwrap();

        let quiet = quiet.expect("quiet link must not fail without a manifest");
        assert!(quiet.stdout.is_empty());
        assert!(quiet.warnings.is_empty());
        assert!(!quiet.problems);
        assert!(loud.is_err(), "an explicit link should say what is missing");
    }

    #[test]
    fn version_command_returns_package_version() {
        let output = run(Cli {
            command: Command::Version,
        })
        .unwrap();
        assert_eq!(
            output.stdout,
            format!("skillenv {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
