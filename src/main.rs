use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use skillenv::{
    AddSourceOptions, InitOptions, LinkOptions, ScopeSelector, Shell, StatusOptions,
    TargetOverride, UnlinkOptions, UpdateSourcesOptions, add_source, format_add_source_report,
    format_init_report, format_link_report, format_status_report, format_update_sources_report,
    hook_script, init_repo, link_global, link_repo, status_global, status_repo, unlink_global,
    unlink_repo, update_sources,
};

#[derive(Debug, Parser)]
#[command(name = "skillenv")]
#[command(about = "Link repo-local skills into agent skill directories")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Add(AddArgs),
    Init(TargetArgs),
    Link(ScopeArgs),
    Unlink(ScopeArgs),
    Status(TargetArgs),
    Global {
        #[command(subcommand)]
        command: GlobalCommand,
    },
    Update(UpdateArgs),
    Hook {
        #[command(subcommand)]
        shell: HookCommand,
    },
}

#[derive(Debug, Args)]
struct AddArgs {
    source: String,
    #[arg(long = "skill")]
    skills: Vec<String>,
    #[arg(long = "into")]
    into: Option<std::path::PathBuf>,
    #[arg(long = "ref")]
    ref_name: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, conflicts_with = "no_claude")]
    claude: bool,
    #[arg(long, conflicts_with = "claude")]
    no_claude: bool,
}

#[derive(Debug, Args)]
struct ScopeArgs {
    #[arg(long = "profile")]
    profiles: Vec<String>,
    #[arg(long, conflicts_with = "profiles")]
    all: bool,
    #[arg(long, conflicts_with = "no_claude")]
    claude: bool,
    #[arg(long, conflicts_with = "claude")]
    no_claude: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct TargetArgs {
    #[arg(long, conflicts_with = "no_claude")]
    claude: bool,
    #[arg(long, conflicts_with = "claude")]
    no_claude: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    names: Vec<String>,
    #[arg(long, conflicts_with = "no_claude")]
    claude: bool,
    #[arg(long, conflicts_with = "claude")]
    no_claude: bool,
}

#[derive(Debug, Subcommand)]
enum HookCommand {
    Zsh,
    Bash,
}

#[derive(Debug, Subcommand)]
enum GlobalCommand {
    Link(ScopeArgs),
    Unlink(ScopeArgs),
    Status(TargetArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> skillenv::Result<String> {
    match cli.command {
        Command::Add(args) => {
            let report = add_source(
                ".",
                AddSourceOptions {
                    source: args.source,
                    into: args.into,
                    skills: args.skills,
                    ref_name: args.ref_name,
                    name: args.name,
                    claude: target_override(args.claude, args.no_claude),
                },
            )?;
            Ok(format_add_source_report(&report))
        }
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
        Command::Update(args) => {
            let report = update_sources(
                ".",
                UpdateSourcesOptions {
                    names: args.names,
                    claude: target_override(args.claude, args.no_claude),
                },
            )?;
            Ok(format_update_sources_report(&report))
        }
        Command::Hook {
            shell: HookCommand::Zsh,
        } => Ok(hook_script(Shell::Zsh)),
        Command::Hook {
            shell: HookCommand::Bash,
        } => Ok(hook_script(Shell::Bash)),
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
