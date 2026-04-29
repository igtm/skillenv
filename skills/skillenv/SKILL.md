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
# 1. repo-local skill を link
skillenv link

# 2. remote skill pack を追加
skillenv add vercel-labs/agent-skills --skill frontend-design

# 3. 現在の linked 状態を確認
skillenv status

# 4. 管理中の remote source を更新
skillenv update
```

## 主要 CLI

- `skillenv link [--all] [--profile <name>...] [--claude|--no-claude] [--quiet]`
  - repo-local source と managed source を target dir に reconcile します。
  - 既定では `default` と `local` だけを対象にします。
- `skillenv unlink [--all] [--profile <name>...] [--claude|--no-claude] [--quiet]`
  - 対象 scope の generated skill だけを安全に削除します。
- `skillenv status [--claude|--no-claude]`
  - `.agents/skills` / `.claude/skills` の linked 状態を表示します。
- `skillenv add <source> [--skill <slug>...] [--ref <git-ref>] [--into <dir>] [--name <logical-name>]`
  - GitHub shorthand、GitHub URL、local path を managed source として導入します。
  - `skillenv.lock.json` に記録し、install 後に即 `link` します。
- `skillenv update [<name>...] [--claude|--no-claude]`
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
skillenv link
```

- `default` と `local` だけを target dir に反映します。
- generated skill は `skillenv-<repo>-<scope>-<skill>` 名になります。

### 2. review profile だけを反映する

```bash
skillenv link --profile review
```

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
# zsh
eval "$(skillenv hook zsh)"

# bash
eval "$(skillenv hook bash)"
```

## Rust library API

この crate は CLI の薄い wrapper だけでなく library としても使えます。

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
- GitHub や local git repo を lock 付きで扱いたいなら `add_source` / `update_sources`
- shell integration が必要なら `hook_script` / `skillenv hook <shell>`
