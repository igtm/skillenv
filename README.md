# skillenv

`skillenv` manages repo-local and remote-installed AI skills, then links them into agent-facing skill directories like `.agents/skills` and `.claude/skills`.

It provides:

- a CLI for linking repo skills, installing remote skill packs, and updating them with a lock file
- a reusable Rust library for embedding the same workflows in other tools
- shell hooks for automatic relinking when you move between repositories

The current version is `0.1.0`.

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
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -v=v0.1.0
```

Install from GitHub with Cargo:

```bash
cargo install --git https://github.com/igtm/skillenv.git --locked
```

Install from a local checkout:

```bash
cargo install --path . --locked
```

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

## Repo Layout

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
```

Generated skills are linked into:

- `.agents/skills` by default
- `.claude/skills` when enabled by config or CLI flags

## CLI Usage

### Link repo-local skills

```bash
skillenv link
skillenv link --all
skillenv link --profile review --profile migration
skillenv unlink --profile review
skillenv status
```

### Install remote skill packs

Add a GitHub repo shorthand:

```bash
skillenv add vercel-labs/agent-skills
```

Add a specific skill from a remote source:

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

Update all managed sources recorded in `skillenv.lock.json`:

```bash
skillenv update
```

Update only selected managed sources:

```bash
skillenv update vercel local-pack
```

## Lock File

Remote and managed local sources are tracked in `skillenv.lock.json` at the repo root.

Each entry records:

- the logical source name
- the requested source and optional ref
- the managed install root
- selected skill slugs
- the resolved revision used for the current install

Commit this file if you want reproducible shared skill sets across machines.

## Config

Global config lives at `~/.config/skillenv/config.toml`.

Current supported keys:

```toml
[targets]
agents = true
claude = false

[gitignore]
auto_update = true

[defaults]
strategy = "render" # or "symlink"

[[external_sources]]
name = "shared"
path = "/path/to/skills"
```

## Library Usage

`skillenv` also exports a Rust library.

```rust
use skillenv::{
    add_source, link_repo, status_repo, AddSourceOptions, LinkOptions, ScopeSelector,
    StatusOptions, TargetOverride,
};

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
```

Key exported flows:

- `link_repo` / `unlink_repo` for reconciling generated skills
- `status_repo` for linked state inspection
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
```

## Release Automation

GitHub Actions runs CI on pull requests and pushes to `main`. A push to `main`, including a merged pull request, also runs the release workflow.

The release workflow reads `version` from `Cargo.toml`, creates or refreshes the `vX.Y.Z` GitHub Release, and uploads cross-built assets:

- `skillenv_vX.Y.Z_x86_64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_aarch64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_x86_64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_aarch64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_x86_64-pc-windows-msvc.tar.gz`
