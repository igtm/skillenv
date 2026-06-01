# skillenv

[日本語版 README](./README_ja.md)

`skillenv` manages repo-local and remote-installed AI skills, then links them into agent-facing skill directories such as `.agents/skills` and `.claude/skills`.

It provides:

- a repo initializer that scaffolds `skillenv/` and updates managed `.gitignore` entries
- repo-local linking and unlinking for default, local, and profile scopes
- managed remote or local source installs tracked in `skillenv.lock.json`
- manual global linking into `$HOME/.agents/skills` and `$HOME/.claude/skills`
- shell hooks for automatic relinking when you change repositories
- a reusable Rust library for embedding the same workflows in other tools

The current version is `0.3.1`.

## Install

Install the latest GitHub Release on Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh
```

Install to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -b=$HOME/.local/bin
```

Install a specific release:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -v=v0.3.1
```

Install from GitHub with Cargo:

```bash
cargo install --git https://github.com/igtm/skillenv.git --locked
```

Install from a local checkout:

```bash
cargo install --path . --locked
```

## Quickstart

Initialize each repository once:

```bash
cd my-repo
skillenv init
```

Add repo-local skills:

```text
skillenv/
  default/
    review/SKILL.md
  local/
    private-helper/SKILL.md
  profiles/
    migration/
      schema-audit/SKILL.md
```

Link the default and local scopes:

```bash
skillenv link
```

Link a profile when you need it:

```bash
skillenv link --profile migration
```

Add a managed source and relink:

```bash
skillenv add vercel-labs/agent-skills --skill frontend-design
```

Restore managed sources from `skillenv.lock.json` on another machine:

```bash
skillenv fetch
```

Check the installed CLI version:

```bash
skillenv version
skillenv --version
```

## Usage

The CLI binary is `skillenv`:

```bash
skillenv init [--claude|--no-claude]
skillenv link [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv unlink [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv status [--claude|--no-claude]
skillenv skills [--tool <claude|codex|opencode|antigravity>...] [--repo-tree] [--json]
skillenv doctor [--json]
skillenv add <source> [--skill <slug>...] [--into <dir>] [--ref <ref>] [--name <source-name>] [--claude|--no-claude]
skillenv fetch [<managed-source>...] [--claude|--no-claude]
skillenv update [<managed-source>...] [--claude|--no-claude]
skillenv global link [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv global unlink [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv global status [--claude|--no-claude]
skillenv hook <zsh|bash>
skillenv version
```

## Command Groups

### Repo-local setup and linking

- `skillenv init`: create the repo-local `skillenv/` layout and managed `.gitignore` entries
- `skillenv link`: generate links for `default/` and `local/` by default
- `skillenv link --profile <name>`: link only selected profile scopes
- `skillenv link --all`: link every discovered scope, including all profiles
- `skillenv unlink`: remove generated links for the selected scopes
- `skillenv status`: inspect whether repo-local targets are linked

### Skill inventory

- `skillenv skills`: list the custom skills currently visible to Codex, Claude Code, OpenCode, and Antigravity
- `skillenv skills --tool codex --tool opencode`: limit output to selected tools
- `skillenv skills --repo-tree`: add nested repo inventory that is not currently visible from the working directory
- `skillenv skills --json`: emit the stable machine-readable report shape

### Diagnostics

- `skillenv doctor`: show detailed diagnostics for config paths, resolved source roots, managed source metadata, and repo/global target state
- `skillenv doctor --json`: emit the same diagnostics as JSON

### Managed sources

- `skillenv add`: install a managed source from GitHub shorthand, a Git URL, or a local checkout path
- `skillenv fetch`: restore managed install roots from `skillenv.lock.json` without changing the lock file
- `skillenv update`: refresh one or more managed sources recorded in `skillenv.lock.json`

### Global targets

- `skillenv global link`: manually link the current repository into `$HOME/.agents/skills` and optional `$HOME/.claude/skills`
- `skillenv global unlink`: remove only this repository's managed entries from the global targets
- `skillenv global status`: inspect global target state

### Shell hooks

- `skillenv hook zsh`: print a `zsh` hook using `add-zsh-hook`
- `skillenv hook bash`: print a `bash` hook using `PROMPT_COMMAND`

### Version output

- `skillenv version`: print the installed `skillenv` CLI version
- `skillenv --version`: standard short form

## Repository Layout

Repo-local sources use this layout:

```text
skillenv/
  default/
    <skill-name>/SKILL.md
  local/
    <skill-name>/SKILL.md
  profiles/
    <profile-name>/
      <skill-name>/SKILL.md
  remote/
    <source-name>/
      ...
```

Generated skills are linked into:

- `.agents/skills` by default
- `.claude/skills` when enabled by config or CLI flags

`skillenv init` creates `default/`, `local/`, and `profiles/`. The `remote/` tree is created on demand by `skillenv add`.

## Naming Rules

`skillenv` normalizes repository, profile, skill, and managed source names to kebab-case:

- letters are lowercased
- ASCII letters and digits are kept
- runs of other characters are converted to `-`
- leading and trailing `-` are removed

Examples:

- `My Repo` -> `my-repo`
- `Review Helpers` -> `review-helpers`
- `frontend_design` -> `frontend-design`

Generated output names follow these patterns:

- repo-local targets: `skillenv-<repo-slug>-<scope>-<skill-slug>`
- global targets: `skillenv-<repo-slug>-g<path-hash>-<scope>-<skill-slug>`
- profile scopes render as `profile:<name>` in status output and `profile-<name>` in generated names

Examples:

- `skillenv-my-repo-default-review`
- `skillenv-my-repo-local-private-helper`
- `skillenv-my-repo-profile-migration-schema-audit`
- `skillenv-my-repo-g2f9d13e4c1ab-default-review`

## `init` in Detail

Run `skillenv init` once inside each repository where you want repo-local outputs:

```bash
skillenv init
skillenv init --claude
```

This command:

- creates `skillenv/default/`, `skillenv/local/`, and `skillenv/profiles/` when missing
- updates `.gitignore` with the managed `skillenv` entries needed for generated targets
- does not link skills by itself

This command does not:

- create global `$HOME/.agents/skills` or `$HOME/.claude/skills`
- install remote sources by itself
- modify shell startup files

Run `skillenv init` before repo-local `link`, `add`, `fetch`, `update`, or any shell hook. Global targets use fixed paths under `$HOME` and do not require `init`.

## Skill Inventory

Use `skillenv skills` when you need to answer "which custom skills does this tool actually see from here?" rather than "what did `skillenv` link?".

```bash
skillenv skills
skillenv skills --tool codex
skillenv skills --tool claude --repo-tree
skillenv skills --json
```

The report includes:

- the tool and scope being inspected
- the visible skill name and directory path
- whether the entry looks `skillenv`-managed
- the detected origin such as `repo:default`, `repo:profile:review`, `external:shared`, or `managed:vercel`
- warnings for duplicate-visible, shadowed, legacy, invalid frontmatter, or missing `SKILL.md`

`--repo-tree` keeps the normal "currently visible" entries and adds repo-wide inventory for nested tool directories. For Claude Code, nested `.claude/skills` paths are labeled `nested-on-demand`; other extra repo entries are labeled `repo-tree-only`.

## Doctor

Use `skillenv doctor` when `status` is too short and you need to inspect the underlying configuration and source wiring.

```bash
skillenv doctor
skillenv doctor --json
```

The report includes:

- repo root and `HOME`
- config file path and whether it exists
- enabled targets and default strategy
- resolved external source directories from config
- managed source metadata from `skillenv.lock.json`, including the original source and transport URL
- repo-local and global target status

## Managed Sources

Run `skillenv init` first so generated links and managed install roots stay ignored.

Add a GitHub repo shorthand:

```bash
skillenv add vercel-labs/agent-skills
```

Add a specific skill from a managed source:

```bash
skillenv add vercel-labs/agent-skills --skill frontend-design
```

Pin a ref and install into a custom managed directory:

```bash
skillenv add vercel-labs/agent-skills --ref main --into skillenv/remote/vercel
```

Add from a GitHub URL or local checkout:

```bash
skillenv add https://github.com/vercel-labs/agent-skills
skillenv add ../agent-skills-local --name local-pack
```

Restore the exact locked revisions from `skillenv.lock.json`:

```bash
skillenv fetch
```

Restore only selected managed sources:

```bash
skillenv fetch vercel local-pack
```

Update all managed sources recorded in `skillenv.lock.json`:

```bash
skillenv update
```

Update only selected managed sources:

```bash
skillenv update vercel local-pack
```

Use `fetch` when you want another machine to reproduce the current lock exactly. Use `update` when you want to move managed sources forward to newer revisions and rewrite the lock file.

## Global Targets

Global targets are fixed:

- `$HOME/.agents/skills`
- `$HOME/.claude/skills`

These commands are manual-only. They do not require `skillenv init`, do not edit `.gitignore`, and do not create the repo-local `skillenv/default`, `skillenv/local`, or `skillenv/profiles` layout for you.

```bash
skillenv global link
skillenv global link --claude
skillenv global unlink --all
skillenv global status
```

Global generated names include a stable hash of the repository path so multiple repositories with the same basename do not collide.

## Shell Setup

`skillenv` can relink skills automatically when you change directories.

For `zsh`, add this to `~/.zshrc`:

```bash
eval "$(skillenv hook zsh)"
```

For `bash`, add this to `~/.bashrc` or `~/.bash_profile`:

```bash
eval "$(skillenv hook bash)"
```

If you installed into a custom directory such as `$HOME/.local/bin`, make sure that directory is in your `PATH` before you add the hook.

The shell hook only runs `skillenv link --quiet`. It does not edit `.gitignore`.

## Lock File

Remote and managed local sources are tracked in `skillenv.lock.json` at the repo root.

Each entry records:

- the logical source name
- the requested source and optional ref
- the managed install root
- selected skill slugs
- the resolved revision used for the current install

Commit this file if you want reproducible shared skill sets across machines.

On another machine, run `skillenv init` once and then `skillenv fetch` to recreate the managed install roots from the lock file. Git-based sources are restored at the locked revision. Local path sources can only be restored when the referenced path also exists on that machine.

## Config

Global config lives at `~/.config/skillenv/config.toml`.

Current supported keys:

```toml
[targets]
agents = true
claude = false

[defaults]
strategy = "render" # or "symlink"

[[external_sources]]
name = "shared"
path = "/path/to/skills"
```

Older `[gitignore].auto_update` settings are ignored as of `0.1.1`. Use `skillenv init` to manage repo-local ignore entries instead.

## Versioning

`skillenv` uses the crate version from `Cargo.toml` as the CLI version and release version.

- `skillenv version` prints the installed version
- `skillenv --version` prints the same value
- GitHub Releases are tagged as `vX.Y.Z`

## Library Usage

`skillenv` also exports a Rust library.

```rust
use skillenv::{
    add_source, init_repo, link_global, link_repo, status_global, status_repo,
    AddSourceOptions, InitOptions, LinkOptions, ScopeSelector, StatusOptions, TargetOverride,
};

let init_report = init_repo(".", InitOptions::default())?;

let add_report = add_source(
    ".",
    AddSourceOptions {
        source: "vercel-labs/agent-skills".to_string(),
        into: None,
        skills: vec!["frontend-design".to_string()],
        ref_name: None,
        name: Some("vercel".to_string()),
        claude: TargetOverride::UseConfig,
    },
)?;

let link_report = link_repo(
    ".",
    LinkOptions {
        selector: ScopeSelector::DefaultLocal,
        claude: TargetOverride::UseConfig,
        quiet: false,
    },
)?;

let status = status_repo(".", StatusOptions::default())?;

let global_link_report = link_global(".", LinkOptions::default())?;
let global_status = status_global(".", StatusOptions::default())?;
```

Key exported flows:

- `init_repo` for creating the repo-local layout and `.gitignore` entries
- `link_repo` / `unlink_repo` for reconciling generated skills
- `status_repo` for linked state inspection
- `link_global` / `unlink_global` / `status_global` for manual global targets under `$HOME`
- `hook_script` for shell hook generation
- `add_source` for installing and locking managed sources
- `update_sources` for refreshing locked sources

See [src/lib.rs](./src/lib.rs) and [src/remote.rs](./src/remote.rs) for the canonical API surface.

## Development

```bash
cargo build
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo package --locked
sh -n install.sh
```

## Release Automation

GitHub Actions runs CI on pull requests and pushes to `main`. A push to `main`, including a merged pull request, also runs the release workflow.

The release workflow reads `version` from `Cargo.toml`, creates or refreshes the `vX.Y.Z` GitHub Release, and uploads cross-built assets:

- `skillenv_vX.Y.Z_x86_64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_aarch64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_x86_64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_aarch64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_x86_64-pc-windows-msvc.tar.gz`
