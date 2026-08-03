---
name: easy-worktree
description: easy-worktree-rs リポジトリで `wt` コマンドを使って worktree を作成・切替・整理するための実践ガイドです。2 文字エイリアス、基本操作、主要オプション、よく使う運用フローをまとめています。
---

# easy-worktree Skill

このスキルは、このリポジトリで Rust 版 `wt` コマンドを使うときの最小手順と運用パターンを提供します。

## 使う場面

- 「新しい作業ブランチを別ディレクトリで始めたい」
- 「今の変更を退避して別 worktree へ移したい」
- 「PR 番号から作業環境をすぐ作りたい」
- 「古い worktree を安全に掃除したい」
- 「bare リポジトリ運用で `--git-dir` を使いたい」
- 「2 文字エイリアスで短く操作したい」

## クイックスタート

```bash
# 既存リポジトリを初期化
wt in

# feature ブランチ用 worktree を作成
wt ad feature-123

# 作成した worktree にジャンプ（サブシェル）
wt se feature-123

# 作業後は exit で戻る
exit
```

## 2 文字エイリアス

| コマンド | エイリアス |
| --- | --- |
| `clone` | `cn` |
| `init` | `in` |
| `add` | `ad` |
| `list` | `li`, `ls` |
| `diff` | `di`, `df` |
| `config` | `cf` |
| `rm` / `remove` | `rm` |
| `clean` | `cl` |
| `setup` | `su` |
| `stash` | `st` |
| `pr` | `pr` |
| `select` | `se`, `sl` |
| `current` | `cu`, `cur` |
| `checkout` | `co` |
| `run` | `ru` |
| `completion` | `cm` |
| `doctor` | `dr` |

## 主要コマンド

- `wt init` / `wt in`
  - 現在の Git リポジトリを easy-worktree 管理として初期化します。
- `wt add [<name> [base_branch]] [--skip-setup|--no-setup] [--select [command...]]` / `wt ad ...`
  - 新しい worktree を作成します。
  - `<name>` なしの場合は作業名を入力し、`--select` なしなら作成後に選択するかを `Y/n` で確認します。Enter の既定値は `yes` です。
- `wt select [<name>|-] [command...]` / `wt se ...` / `wt sl ...`
  - worktree に切り替えます。
  - 引数なしで `fzf` 選択、`-` で直前の worktree に戻ります。
- `wt list [--pr] [--days N] [--merged] [--closed] [--all] [--sort ...] [--asc|--desc]` / `wt li ...` / `wt ls ...`
  - worktree 一覧を表示します。
- `wt stash <name> [base_branch]` / `wt st ...`
  - 現在の変更を stash し、新規 worktree 側へ移します。
- `wt rm [<name>] [-f|--force]`
  - worktree を削除します。
  - `<name>` なしの場合は削除対象の worktree を対話選択します。
- `wt clean [--days N] [--merged] [--closed] [--all] [--yes|-y]` / `wt cl ...`
  - 条件に合う不要 worktree をまとめて削除します。
- `wt pr add <number>`
  - PR の head branch 名と同じ名前の worktree を作成します（`gh` 必須）。
- `wt setup` / `wt su`
  - 設定に従って `.env` などをコピーし、hook を実行します。
- `wt diff [<name>] [args...]` / `wt di ...` / `wt df ...`
  - 対象 worktree で diff を確認します。
- `wt config [--global|--local] [key [value]]` / `wt cf ...`
  - 設定の確認・更新を行います。優先順位は Global > Local > Project です。
  - 例: `wt cf --global worktrees_dir ".my_global_worktrees"`
- `wt current` / `wt cu` / `wt cur`
  - 現在の worktree 名を表示します。
- `wt co <name>` / `wt checkout <name>`
  - 対象 worktree のパスを表示します。
- `wt run <name> <command...>` / `wt ru ...`
  - 対象 worktree でコマンドを実行します。
- `wt completion <bash|zsh>` / `wt cm ...`
  - シェル補完スクリプトを出力します。
- `wt doctor` / `wt dr`
  - システム環境や依存ツール、設定ファイル（および無効キーの警告）を確認します。

## よく使う運用フロー

### 1. 新規機能を並行開発する

```bash
wt ad feat/search-ui main --select
```

- `main` から `feat/search-ui` を切って、そのまま作業に入れます。

### 2. 今の変更を別作業として切り出す

```bash
wt st fix/login-bug
wt se fix/login-bug
```

- 現在の未コミット変更を新 worktree に移し、main 側を汚さず整理できます。

### 3. PR からレビュー/検証環境を作る

```bash
wt pr add 123
wt pr co 123
```

### 4. 古い worktree を掃除する

```bash
wt li --days 30
wt cl --merged
```

## bare リポジトリでの使い方

`--git-dir` をグローバル引数として付けます。また、`-C` を使って特定のパスで実行することも可能です。

```bash
wt --git-dir=/path/to/sandbox.git in
wt --git-dir=/path/to/sandbox.git ad feat/abc main
wt -C /other/repo li
```

bare リポジトリでは、新しく追加される worktree は `repo.git/` の内部ではなく、ベース worktree と同じ親ディレクトリ（例: `repo/` 直下や `repo/.worktrees/`）に作成されます。

## 参照ドキュメント

- 全体仕様: `README.md`
- 日本語手順: `README_ja.md`
- 実装ベースのコマンド定義: `src/lib.rs`
- integration test: `tests/integration.rs`
