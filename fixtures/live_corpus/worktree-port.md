---
name: worktree-port
description: worktree-port リポジトリで `wtport` コマンドを使い、Git worktree ごとの決定論的な port 割り当て、環境変数注入、collision 調査、`wtport.toml` の設定を行うための実践ガイドです。`port` `exec` `list`、`--range`、`--salt`、`--seed`、`--shell` を使うときに利用します。
---

# worktree-port Skill

このスキルは、このリポジトリで Rust 製 `wtport` コマンドを使うときの最小手順と運用パターンを提供します。

`profile`、`range`、`salt` はすべて省略できます。省略時は profile=`default`、range=`10000-59999` です。

## 使う場面

- 「worktree ごとに dev server の port を安定して分けたい」
- 「`pnpm dev --port "$PORT"` を毎回手で決めたくない」
- 「`docker compose` や任意コマンドへ port を環境変数で渡したい」
- 「`wtport.toml` の profile / range / salt を整理したい」
- 「port collision の原因を確認したい」
- 「`--salt` を変えて再配置したい」

## クイックスタート

```bash
# 共有 repo root に設定を置く
cat > wtport.toml <<'EOF'
[profiles.front]
range = "3000-3999"

[profiles.api]
range = "8000-8999"
EOF

# 現在の worktree の port を取得
wtport port
wtport port front

# port を環境変数として渡して起動
wtport exec -- docker compose up
wtport exec --profile front --shell 'pnpm dev --port "$PORT"'
```

## config 形式

`wtport.toml` は shared repo root に置きます。

```toml
salt = "optional-global-salt"

[profiles.front]
range = "3000-3999"

[profiles.api]
range = "8000-8999"
salt = "optional-profile-salt"
```

解決順序は次です。

1. CLI `--range` / `--salt`
2. `[profiles.<name>]`
3. top-level `salt`

`--seed` は `--salt` の alias です。

## 主要コマンド

- `wtport port [profile] [--range <start-end>] [--salt <value>]`
  - 現在の worktree に割り当てられた port を標準出力へ数値だけで出します。
- `wtport exec [--range <start-end>] [--salt <value>] [--profile <spec>]... -- <command>...`
  - `PORT` などを注入してコマンドをそのまま実行します。
- `wtport exec [--range <start-end>] [--salt <value>] [--profile <spec>]... --shell '<script>'`
  - shell 展開が必要な `pnpm dev --port "$PORT"` 向けです。
- `wtport env [--range <start-end>] [--salt <value>] [--profile <spec>]...`
  - 実行はせず、POSIX shell 向けの `export KEY='VALUE'` 行だけを出します。
- `wtport list [profile] [--range <start-end>] [--salt <value>]`
  - 同一 repo の全 worktree と port 割り当てを表示します。

`exec` / `env` の `<spec>` は v1 で次です。

```text
<profile>[,env=<ENV>][,range=<start-end>][,salt=<value>]
```

`port` / `list` では `[profile]` を省略すると `default` が使われます。`exec` / `env` では `--profile` を 0 個にすると implicit primary として `default` が使われます。

## 注入される環境変数

- `PORT`
- `WTPORT_PORT`
- `WTPORT_PROFILE`
- `WTPORT_WORKTREE`
- `<SANITIZED_PROFILE>_PORT`
  - 例: profile が `front` なら `FRONT_PORT`
  - profile を省略した場合は `DEFAULT_PORT`

`wtport exec` / `wtport env` では primary profile に対して上の 5 つを使います。追加の `--profile` ごとに 1 つの named export が増えます。

- 1 個目の `--profile` が primary です。
- 2 個目以降は `env=` があればその env 名、なければ `<SANITIZED_PROFILE>_PORT` を使います。
- top-level `--range` / `--salt` は全 `--profile` の default です。
- inline `range=` / `salt=` はその `--profile` だけを上書きします。
- `--seed` は top-level `--salt` の alias のままです。
- primary の export 群は固定なので、`env=` は追加 profile 用です。
- 同じ env 名が 2 回出る組み合わせは fail します。
- `wtport exec -- <command>...` は raw argv 実行で、`$PORT` や placeholder の置換はしません。
- `$PORT` や `$API_PORT` をその場で展開したいときだけ `--shell` を使います。
- v0.1.2 で `wtport exec front -- ...` は廃止され、`wtport exec --profile front -- ...` へ移行しました。

## よく使う運用フロー

### 1. frontend 開発を worktree ごとに分離する

```bash
wtport exec --profile front --profile api,env=API_PORT --shell 'VITE_API_BASE_URL=/api pnpm dev --host 0.0.0.0 --port "$PORT"'
```

`nohup` や `portless` を shell 側で組み立てたいときは `env` が向いています。

```bash
eval "$(wtport env --profile front --profile api,env=API_PORT)"
nohup env VITE_API_BASE_URL=/api \
  pnpm dev --port "$PORT" \
  2>&1 | npx portless --backend "http://127.0.0.1:$API_PORT" \
  > front.log &
```

- v1 では `wtport` は env 解決だけを担当します。
- `nohup`、`> front.log`、`&`、固定 env は shell alias/function 側で管理します。

### 2. docker compose へ port を渡す

```bash
wtport exec -- docker compose up
wtport exec --profile api -- docker compose up
```

- `wtport` は `compose.yml` を直接書き換えません。
- `docker compose` を子プロセスとして起動し、`PORT` などの環境変数を渡します。
- Compose 側で `${PORT}`、`${DEFAULT_PORT}`、`${API_PORT}` を参照していれば、その値が補間されます。
- `docker compose up` 自体は shell 展開不要なので `--shell` は不要です。

```yaml
services:
  api:
    ports:
      - "${PORT}:8000"
```

### 3. 全 worktree の割り当てを確認する

```bash
wtport list front
```

### 4. port を意図的に再配置する

```bash
wtport port front --salt staging
wtport exec --profile front --salt staging --shell 'pnpm dev --port "$PORT"'
```

## collision と異常系

- `wtport` は現在の worktree だけでなく、同じ repo の全 worktree に対して assignment table を作ってから返します。
- 同一 profile / range / salt で collision が出ると非 0 で失敗します。
- range 幅が worktree 数未満なら、その range では一意化不可能です。
- worktree root basename が repo 内で重複していても失敗します。

調査の第一手は次です。

```bash
wtport list front
wtport list front --range 3000-3999 --salt debug
```

対処は次を優先します。

- `--salt` または profile salt を変える
- range を広げる
- basename が重複している worktree directory を rename する

## 参照ドキュメント

- 全体仕様: `README.md`
- CLI 実装: `src/main.rs`
- コアロジック: `src/lib.rs`
- integration test: `tests/integration.rs`
