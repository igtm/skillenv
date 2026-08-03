---
name: skillenv
description: skillenv リポジトリで repo-local skill の link/unlink、remote skill pack の add/update、Rust library API の使い分けを行うための実践ガイドです。CLI と library の両方を対象に、典型的な運用フローと主要データ構造をまとめています。
---

# skillenv Skill

このスキルは、このリポジトリで `skillenv` を使って skill source を管理し、AI 向け skill directory に安全に反映するときの最小手順を提供します。

## 使う場面

- 「repo 配下の `skillenv/default` や `skillenv/profiles/*` を `.agents/skills` に link したい」
- 「GitHub の skill repo を lock 付きで導入したい」
- 「`skillenv.lock.json` に入っている managed source を一括更新したい」
- 「CLI ではなく Rust library として `skillenv` を呼び出したい」
- 「zsh/bash で directory change ごとに自動 relink したい」

## クイックスタート

```bash
# 1. repo を初期化
skillenv init

# 2. repo-local skill を link
skillenv link

# repo-local 初期化なしで global target へ手動 link
skillenv global link

# 3. remote skill pack を追加
skillenv add vercel-labs/agent-skills --skill frontend-design

# 4. 現在の linked 状態を確認
skillenv status

# 4b. 各 tool から見える custom skill を確認
skillenv skills --tool codex

# 4c. config と source の診断を確認
skillenv doctor

# 5. 管理中の remote source を更新
skillenv update
```

## 主要 CLI

- `skillenv init [--claude|--no-claude]`
  - `skillenv/default` `skillenv/local` `skillenv/profiles` を作成します。
  - 管理対象の `skillenv` エントリだけを `.gitignore` に追記します。
- `skillenv link [--all] [--profile <name>...] [--claude|--no-claude] [--quiet]`
  - repo-local source と managed source を target dir に reconcile します。
  - 実行前に `skillenv init` が必要です。
  - 既定では `default` と `local` だけを対象にします。
- `skillenv unlink [--all] [--profile <name>...] [--claude|--no-claude] [--quiet]`
  - 対象 scope の generated skill だけを安全に削除します。
- `skillenv status [--claude|--no-claude]`
  - `.agents/skills` / `.claude/skills` の linked 状態を表示します。
- `skillenv skills [--tool <claude|codex|opencode|antigravity>...] [--repo-tree] [--json]`
  - 現在の CWD から各 tool が見える custom skill を列挙します。
  - `--repo-tree` で nested tool dir の repo inventory も追加します。
  - `--json` で機械可読な report を出します。
- `skillenv doctor [--json]`
  - config file path、resolved external source、managed source metadata、repo/global target 状態を表示します。
  - `status` より詳細な診断用です。
- `skillenv global link [--all] [--profile <name>...] [--claude|--no-claude] [--quiet]`
  - 現在の repo の skill を `$HOME/.agents/skills` / `$HOME/.claude/skills` に手動 link します。
  - `skillenv init` は不要で、`.gitignore` も更新しません。
- `skillenv global unlink [--all] [--profile <name>...] [--claude|--no-claude] [--quiet]`
  - global target 上の現在 repo 由来の generated skill だけを安全に削除します。
- `skillenv global status [--claude|--no-claude]`
  - global target 上の現在 repo の linked 状態を表示します。
- `skillenv add <source> [--skill <slug>...] [--ref <git-ref>] [--into <dir>] [--name <logical-name>]`
  - GitHub shorthand、GitHub URL、local path を managed source として導入します。
  - 実行前に `skillenv init` が必要です。
  - `skillenv.lock.json` に記録し、install 後に即 `link` します。
- `skillenv update [<name>...] [--claude|--no-claude]`
  - 実行前に `skillenv init` が必要です。
  - lock 済み managed source を全件または個別更新します。
- `skillenv hook zsh`
  - `chpwd` hook を出力します。
- `skillenv hook bash`
  - `PROMPT_COMMAND` 用の hook を出力します。

## source layout

repo-local source は次の layout を前提にします。

```text
skillenv/
  default/<skill>/SKILL.md
  local/<skill>/SKILL.md
  profiles/<profile>/<skill>/SKILL.md
```

managed remote source は install 後に同じ layout へ正規化されます。flat `skills/<skill>` layout や単一 skill directory も受け付けます。

## よく使う運用フロー

### 1. repo-local skill だけを反映する

```bash
skillenv init
skillenv link
```

- `init` は layout と `.gitignore` を 1 回だけ整えます。
- `link` は `default` と `local` だけを target dir に反映します。
- generated skill は `skillenv-<repo>-<scope>-<skill>` 名になります。

### 2. review profile だけを反映する

```bash
skillenv link --profile review
```

### 2c. tool から見える custom skill を棚卸しする

```bash
skillenv skills
skillenv skills --tool claude --repo-tree
skillenv skills --json
```

- `status` は link 状態を見るコマンドです。
- `skills` は tool 側の custom skill discovery 結果を見るコマンドです。
- `--repo-tree` を付けると、Claude Code の nested `.claude/skills` は `nested-on-demand`、それ以外の追加 entry は `repo-tree-only` として出ます。

### 2d. config / external source / managed source を診断する

```bash
skillenv doctor
skillenv doctor --json
```

- config file path と存在有無を確認できます。
- config の `external_sources` がどの directory に解決されるか確認できます。
- `skillenv.lock.json` に入っている managed source の source 名、transport URL、install root、revision を確認できます。

### 2b. 現在の repo を global target に手動反映する

```bash
skillenv global link
skillenv global status
```

- global target は固定で `$HOME/.agents/skills` と `$HOME/.claude/skills` です。
- `init` は不要です。
- repo basename が同じ別 repo と衝突しない generated 名になります。

### 3. GitHub の skill pack を 1 つだけ導入する

```bash
skillenv add vercel-labs/agent-skills --skill frontend-design --name vercel
```

- install root の既定値は `skillenv/remote/<name>` です。
- 導入結果は `skillenv.lock.json` に記録されます。

### 4. lock 済み source を更新する

```bash
skillenv update
skillenv update vercel
```

### 5. zsh/bash で自動 relink する

```bash
# 先に repo 側を 1 回初期化
skillenv init

# zsh
eval "$(skillenv hook zsh)"

# bash
eval "$(skillenv hook bash)"
```

- hook は `skillenv link --quiet` だけを実行し、`.gitignore` は更新しません。
- hook は repo-local target だけを扱い、global target には一切触れません。
- 旧 `gitignore.auto_update` 設定に頼らず、repo ごとに `skillenv init` を実行します。

## Rust library API

この crate は CLI の薄い wrapper だけでなく library としても使えます。

### repo-local layout の初期化

```rust
use skillenv::{init_repo, InitOptions};

let report = init_repo(".", InitOptions::default())?;
```

### repo-local / managed source の反映

```rust
use skillenv::{link_repo, LinkOptions, ScopeSelector, TargetOverride};

let report = link_repo(
    ".",
    LinkOptions {
        selector: ScopeSelector::DefaultLocal,
        claude: TargetOverride::UseConfig,
        quiet: false,
    },
)?;
```

### global target への手動反映

```rust
use skillenv::{link_global, status_global, LinkOptions, StatusOptions};

let report = link_global(".", LinkOptions::default())?;
let status = status_global(".", StatusOptions::default())?;
```

### managed remote source の導入

```rust
use skillenv::{add_source, AddSourceOptions, TargetOverride};

let report = add_source(
    ".",
    AddSourceOptions {
        source: "vercel-labs/agent-skills".to_string(),
        into: None,
        skills: vec!["frontend-design".to_string()],
        ref_name: Some("main".to_string()),
        name: Some("vercel".to_string()),
        claude: TargetOverride::UseConfig,
    },
)?;
```

### lock 済み source の更新

```rust
use skillenv::{update_sources, UpdateSourcesOptions, TargetOverride};

let report = update_sources(
    ".",
    UpdateSourcesOptions {
        names: vec!["vercel".to_string()],
        claude: TargetOverride::UseConfig,
    },
)?;
```

## 重要な型

- `LinkOptions`
  - `selector`, `claude`, `quiet`
- `InitOptions`
  - `claude`
- `UnlinkOptions`
  - `selector`, `claude`, `quiet`
- `StatusOptions`
  - `claude`
- `AddSourceOptions`
  - `source`, `into`, `skills`, `ref_name`, `name`, `claude`
- `UpdateSourcesOptions`
  - `names`, `claude`

## 実装を読む場所

- 公開 API: `src/lib.rs`
- managed source / lock file: `src/remote.rs`
- CLI surface: `src/main.rs`
- 導入テスト: `src/remote.rs` の `remote::tests`
- repo-local link/unlink テスト: `src/lib.rs` の `tests`

## 判断基準

- repo-local skill を反映したいだけなら `link_repo` / `skillenv link`
- global target に手動で反映したいなら `link_global` / `skillenv global link`
- repo の layout と `.gitignore` を整えたいなら `init_repo` / `skillenv init`
- GitHub や local git repo を lock 付きで扱いたいなら `add_source` / `update_sources`
- shell integration が必要なら `hook_script` / `skillenv hook <shell>` ただし hook は repo-local only
