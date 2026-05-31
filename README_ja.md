# skillenv

[English README](./README.md)

`skillenv` は、リポジトリ内で管理する AI skill と、外部から導入した managed skill source をまとめて扱い、`.agents/skills` や `.claude/skills` のような agent 向け skill directory にリンクするためのツールです。

提供するもの:

- `skillenv/` のひな型と managed `.gitignore` エントリを作る初期化コマンド
- `default` / `local` / `profile` scope を対象にした repo-local の link / unlink
- `skillenv.lock.json` で追跡する remote / local の managed source インストール
- `$HOME/.agents/skills` と `$HOME/.claude/skills` への手動 global link
- リポジトリ移動時に自動で relink する shell hook
- 同じ操作を埋め込める Rust ライブラリ

現在のバージョンは `0.3.0` です。

## インストール

Linux または macOS で最新の GitHub Release をインストールします。

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh
```

インストール先を指定する場合:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -b=$HOME/.local/bin
```

バージョンを指定する場合:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -v=v0.3.0
```

Cargo で GitHub からインストールする場合:

```bash
cargo install --git https://github.com/igtm/skillenv.git --locked
```

ローカル checkout からインストールする場合:

```bash
cargo install --path . --locked
```

## クイックスタート

まず、対象リポジトリごとに 1 回だけ初期化します。

```bash
cd my-repo
skillenv init
```

repo-local の skill は次のように配置します。

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

`default` と `local` を link します。

```bash
skillenv link
```

必要な profile だけ link する場合:

```bash
skillenv link --profile migration
```

managed source を追加して relink する場合:

```bash
skillenv add vercel-labs/agent-skills --skill frontend-design
```

インストール済み CLI のバージョン確認:

```bash
skillenv version
skillenv --version
```

## 使い方

バイナリ名は `skillenv` です。

```bash
skillenv init [--claude|--no-claude]
skillenv link [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv unlink [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv status [--claude|--no-claude]
skillenv skills [--tool <claude|codex|opencode|antigravity>...] [--repo-tree] [--json]
skillenv doctor [--json]
skillenv add <source> [--skill <slug>...] [--into <dir>] [--ref <ref>] [--name <source-name>] [--claude|--no-claude]
skillenv update [<managed-source>...] [--claude|--no-claude]
skillenv global link [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv global unlink [--profile <name>...] [--all] [--claude|--no-claude] [--quiet]
skillenv global status [--claude|--no-claude]
skillenv hook <zsh|bash>
skillenv version
```

## コマンドの整理

### Repo-local の初期化と linking

- `skillenv init`: repo-local の `skillenv/` layout と managed `.gitignore` エントリを作成
- `skillenv link`: デフォルトで `default/` と `local/` を生成
- `skillenv link --profile <name>`: 指定した profile scope だけを link
- `skillenv link --all`: 見つかった全 scope を link
- `skillenv unlink`: 対象 scope の generated link を削除
- `skillenv status`: repo-local target の状態を確認

### Skill inventory

- `skillenv skills`: Codex / Claude Code / OpenCode / Antigravity から現在見えている custom skill を列挙
- `skillenv skills --tool codex --tool opencode`: 対象 tool を絞って表示
- `skillenv skills --repo-tree`: 現在は見えていない nested tool dir も repo inventory として追加
- `skillenv skills --json`: 安定した機械可読 JSON を出力

### Diagnostics

- `skillenv doctor`: config path、解決済み source root、managed source metadata、repo/global target 状態を詳細表示
- `skillenv doctor --json`: 同じ診断情報を JSON で出力

### Managed source

- `skillenv add`: GitHub shorthand、Git URL、または local checkout path から managed source を導入
- `skillenv update`: `skillenv.lock.json` に記録された managed source を更新

### Global target

- `skillenv global link`: 現在のリポジトリを `$HOME/.agents/skills` と必要なら `$HOME/.claude/skills` に手動 link
- `skillenv global unlink`: 現在のリポジトリに属する generated entry だけを global target から削除
- `skillenv global status`: global target の状態を確認

### Shell hook

- `skillenv hook zsh`: `add-zsh-hook` を使う `zsh` hook を出力
- `skillenv hook bash`: `PROMPT_COMMAND` を使う `bash` hook を出力

### バージョン表示

- `skillenv version`: インストール済み `skillenv` のバージョンを表示
- `skillenv --version`: 短い標準形式

## Repository Layout

repo-local source は次の layout を使います。

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

生成された skill は次に link されます。

- デフォルトでは `.agents/skills`
- config または CLI flag で有効なら `.claude/skills`

`skillenv init` が作るのは `default/`, `local/`, `profiles/` です。`remote/` は `skillenv add` を実行したときに必要に応じて作られます。

## 命名規則

`skillenv` は repository 名、profile 名、skill 名、managed source 名を kebab-case に正規化します。

- 英字は小文字化
- ASCII の英数字はそのまま利用
- それ以外の連続文字は `-` に変換
- 先頭末尾の `-` は削除

例:

- `My Repo` -> `my-repo`
- `Review Helpers` -> `review-helpers`
- `frontend_design` -> `frontend-design`

生成される output 名は次の規則です。

- repo-local target: `skillenv-<repo-slug>-<scope>-<skill-slug>`
- global target: `skillenv-<repo-slug>-g<path-hash>-<scope>-<skill-slug>`
- profile scope は status では `profile:<name>`、generated name では `profile-<name>`

例:

- `skillenv-my-repo-default-review`
- `skillenv-my-repo-local-private-helper`
- `skillenv-my-repo-profile-migration-schema-audit`
- `skillenv-my-repo-g2f9d13e4c1ab-default-review`

## `init` の詳細

repo-local output を使うリポジトリでは、最初に 1 回 `skillenv init` を実行します。

```bash
skillenv init
skillenv init --claude
```

このコマンドが行うこと:

- `skillenv/default/`, `skillenv/local/`, `skillenv/profiles/` を不足時に作成
- generated target 用に必要な managed `skillenv` エントリを `.gitignore` に追加
- skill の link 自体は実行しない

このコマンドが行わないこと:

- global の `$HOME/.agents/skills` や `$HOME/.claude/skills` の作成
- remote source のインストール
- shell startup file の編集

repo-local の `link`, `add`, `update`, shell hook を使う前に `skillenv init` を実行してください。`$HOME` 配下の global target は固定 path を使うため、`init` は不要です。

## Skill Inventory

`skillenv skills` は、「`skillenv` が何を link したか」ではなく「この場所から各 tool が実際にどの custom skill を見に行くか」を確認したいときに使います。

```bash
skillenv skills
skillenv skills --tool codex
skillenv skills --tool claude --repo-tree
skillenv skills --json
```

レポートには次を含みます。

- 対象 tool と scope
- 可視な skill 名と directory path
- `skillenv` 管理物らしいかどうか
- `repo:default`、`repo:profile:review`、`external:shared`、`managed:vercel` のような由来
- `duplicate-visible`、`shadowed`、`legacy`、frontmatter 不正、`SKILL.md` 欠落などの warning

`--repo-tree` を付けると、通常の current discovery を残したまま、repo 全体の nested tool dir inventory を追加します。Claude Code の nested `.claude/skills` は `nested-on-demand`、それ以外の追加 entry は `repo-tree-only` と表示されます。

## Doctor

`status` では情報が足りないときは `skillenv doctor` を使います。設定と source の配線をまとめて確認できます。

```bash
skillenv doctor
skillenv doctor --json
```

レポートには次を含みます。

- repo root と `HOME`
- config file path と存在有無
- 有効 target と default strategy
- config から解決した external source directory
- `skillenv.lock.json` の managed source metadata。元の source と transport URL も含みます
- repo-local / global target の状態

## Managed Source

generated link と managed install root を ignore した状態にしたいので、先に `skillenv init` を実行してください。

GitHub repo shorthand を追加する場合:

```bash
skillenv add vercel-labs/agent-skills
```

特定の skill だけ追加する場合:

```bash
skillenv add vercel-labs/agent-skills --skill frontend-design
```

ref を固定し、custom な managed directory に入れる場合:

```bash
skillenv add vercel-labs/agent-skills --ref main --into skillenv/remote/vercel
```

GitHub URL や local checkout から追加する場合:

```bash
skillenv add https://github.com/vercel-labs/agent-skills
skillenv add ../agent-skills-local --name local-pack
```

`skillenv.lock.json` に記録された全 source を更新する場合:

```bash
skillenv update
```

指定した source だけ更新する場合:

```bash
skillenv update vercel local-pack
```

## Global Target

global target は固定です。

- `$HOME/.agents/skills`
- `$HOME/.claude/skills`

これらのコマンドは手動運用向けです。`skillenv init` は不要で、`.gitignore` も編集せず、repo-local の `skillenv/default`, `skillenv/local`, `skillenv/profiles` も作成しません。

```bash
skillenv global link
skillenv global link --claude
skillenv global unlink --all
skillenv global status
```

global の generated name には repository path の安定 hash が入るため、basename が同じ別 repository と衝突しません。

## Shell Setup

ディレクトリ移動時に自動で relink したい場合は shell hook を使います。

`zsh` の場合は `~/.zshrc` に追加します。

```bash
eval "$(skillenv hook zsh)"
```

`bash` の場合は `~/.bashrc` または `~/.bash_profile` に追加します。

```bash
eval "$(skillenv hook bash)"
```

`$HOME/.local/bin` のような custom directory に入れた場合は、hook を設定する前にその directory が `PATH` に入っていることを確認してください。

shell hook が実行するのは `skillenv link --quiet` だけで、`.gitignore` は編集しません。

## Lock File

remote source と managed local source は repository root の `skillenv.lock.json` に記録されます。

各 entry には次が入ります。

- logical source name
- requested source と optional ref
- managed install root
- selected skill slug
- 現在 install されている resolved revision

複数マシンで同じ skill set を再現したい場合は、この file を commit してください。

## Config

global config は `~/.config/skillenv/config.toml` にあります。

現在サポートしている key:

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

古い `[gitignore].auto_update` 設定は `0.1.1` 以降無視されます。repo-local ignore の管理には `skillenv init` を使ってください。

## バージョニング

`skillenv` は `Cargo.toml` の crate version を CLI version と release version に使います。

- `skillenv version` でインストール済み version を表示
- `skillenv --version` でも同じ値を表示
- GitHub Release の tag は `vX.Y.Z`

## Library Usage

`skillenv` は Rust ライブラリとしても使えます。

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

主な exported flow:

- `init_repo`: repo-local layout と `.gitignore` entry の作成
- `link_repo` / `unlink_repo`: generated skill の反映と削除
- `status_repo`: linked 状態の確認
- `link_global` / `unlink_global` / `status_global`: `$HOME` 配下の global target を手動操作
- `hook_script`: shell hook の生成
- `add_source`: managed source の install と lock
- `update_sources`: lock 済み source の更新

正確な API surface は [src/lib.rs](./src/lib.rs) と [src/remote.rs](./src/remote.rs) を参照してください。

## 開発

```bash
cargo build
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo package --locked
sh -n install.sh
```

## リリース自動化

GitHub Actions は pull request と `main` への push で CI を実行します。pull request の merge も GitHub 上では `main` への push になるため、release workflow が実行されます。

release workflow は `Cargo.toml` の `version` を読み取り、`vX.Y.Z` の GitHub Release を作成または更新し、以下のクロスビルド成果物をアップロードします。

- `skillenv_vX.Y.Z_x86_64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_aarch64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_x86_64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_aarch64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_x86_64-pc-windows-msvc.tar.gz`
