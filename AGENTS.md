# Repository Guidelines

## Project Structure & Module Organization

`skillenv` is a small Rust crate with both a CLI and a reusable library.

- `src/lib.rs`: core linking, unlinking, status, config, and render/symlink logic
- `src/remote.rs`: managed remote sources, lock file handling, install/update flows
- `src/main.rs`: CLI entrypoint and subcommand wiring
- `skills/skillenv/SKILL.md`: AI-facing usage guide for this repository
- `.github/workflows/`: CI and release automation
- `install.sh`: release installer for Linux and macOS

Do not edit `target/`; it is build output and should remain ignored.

## Build, Test, and Development Commands

- `cargo build`: build the library and CLI
- `cargo run -- <subcommand>`: run the CLI locally, for example `cargo run -- status`
- `cargo test --locked`: run all unit tests
- `cargo fmt --check`: verify formatting
- `cargo clippy --all-targets -- -D warnings`: enforce lint-clean code
- `cargo package --locked`: verify the crate packages correctly
- `sh -n install.sh`: syntax-check the installer script

Before opening a PR, run the full validation set above.

## Coding Style & Naming Conventions

Use standard Rust style with 4-space indentation and `rustfmt` formatting. Keep modules focused: shared library logic in `lib.rs`, managed-source logic in `remote.rs`, CLI parsing in `main.rs`. Use `snake_case` for functions and variables, `CamelCase` for structs/enums, and imperative CLI/report wording such as `link`, `unlink`, and `update`.

Prefer small helper functions over deeply nested control flow. Keep shell scripts POSIX-compatible `sh`.

## Testing Guidelines

Tests currently live inline under `#[cfg(test)]` in `src/lib.rs` and `src/remote.rs`. Add tests next to the code they exercise. Name tests descriptively, e.g. `update_sources_skips_unchanged_and_reinstalls_changed_source`.

Cover both happy paths and safety checks: duplicate skills, cleanup boundaries, lock updates, and installer-adjacent packaging behavior when relevant.

## Commit & Pull Request Guidelines

Commit messages in this repo use short, imperative subjects with leading capitals, e.g. `Add release automation and installation docs`. Keep commits scoped to one logical change.

PRs should include:

- a short summary of user-visible changes
- notes on tests run
- README, installer, or workflow updates when distribution behavior changes

## Agent-Specific Instructions

Respond to the user in Japanese for this repository.
