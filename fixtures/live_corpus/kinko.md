---
name: kinko
description: kinko CLI でシークレットを OS キーチェーン（macOS Keychain / Windows Credential Manager / Linux Secret Service・pass）に保管し、サブプロセスにだけ環境変数として注入するための実践ガイドです。平文 .env を作らずに開発する設定・登録・実行・移行・Docker 連携の手順と、値を漏らさないための注意点をまとめています。
---

# kinko Skill

このスキルは、`kinko` コマンドでローカル開発のシークレットを安全に扱うための最小手順と運用パターンを提供します。値は OS のキーチェーンに保管し、`kinko run -- <cmd>` で **そのサブプロセスにだけ** 注入します。平文 `.env` をディスクに残さず、値を `ps`・ログ・コマンドライン引数に出しません。

## 使う場面

- 「平文 `.env` をやめて、シークレットを OS キーチェーンに保管したい」
- 「アプリ起動時だけシークレットを環境変数として渡したい（親シェルは汚さない）」
- 「既存の `.env` をキーチェーンへ一括移行したい」
- 「多行の JSON / PEM（サービスアカウント等）を壊さずに保存したい」
- 「docker compose にシークレットを安全に渡したい」
- 「どのキーが登録済みか、値を出さずに確認したい」

## クイックスタート

```bash
# 1. 雛形 kinko.toml を生成してキーを定義（値は含まれないのでコミット可）
kinko init
kinko config add keys DATABASE_URL STRIPE_API_KEY GCP_SERVICE_ACCOUNT_JSON

# 2. 値を非表示入力で保存
kinko set DATABASE_URL

# 3. 登録状況を確認（✓/✗ のみ・値は出ない）
kinko ls

# 4. シークレットを注入してアプリを起動
kinko run -- npm run dev
kinko run -- docker compose up
```

## エイリアス

| コマンド | エイリアス |
| --- | --- |
| `run` | `exec`, `x` |
| `set` | `add` |
| `list` | `ls` |
| `delete` | `rm`, `del` |

`get` / `migrate` / `docker` / `prefix` / `keys` にエイリアスはありません。

## 主要コマンド

- `kinko init [KEY...]` `[--force]`
  - カレントに雛形 `kinko.toml` を生成します（コメント付き）。`KEY` を渡すと `keys` に書き込みます。既存ファイルは `--force` なしでは上書きしません。
- `kinko config <show|get|set|unset|add|remove>`
  - `kinko.toml` をインラインで確認/編集します（**コメント・書式は保持**。書込前に検証し、不正なら書きません）。
  - `kinko config show` — 現在の設定ファイルをそのまま表示。
  - `kinko config get <FIELD>` — 1 フィールドの値を表示。
  - `kinko config set <FIELD> <VALUE>` / `unset <FIELD>` — スカラー（`prefix` | `backend`）を設定/解除。
  - `kinko config add <FIELD> <VALUE>...` / `remove <FIELD> <VALUE>...` — リスト（`keys` | `docker_secret_keys` | `file_input_suffixes`）に追加/削除。`keys` から削除すると `docker_secret_keys` からも自動的に外れます。
  - 例: `kinko config add keys DATABASE_URL` / `kinko config set backend pass`
- `kinko run [KEY...] -- <CMD> [ARGS...]` / `kinko x ...`
  - 対象キーをキーチェーンから取得し、環境変数として注入して `<CMD>` を exec 実行します。
  - `KEY...` を省略すると設定の全キーが対象。未登録キーは既定で空文字として export します（`--no-empty` で無効化）。
  - 例: `kinko run -- docker compose up` / `kinko run DATABASE_URL -- psql`
- `kinko set [KEY]` / `kinko add ...`
  - 値を保存します。`KEY` 省略時は設定の全キーを順に対話登録。
  - `--file <PATH>` ファイルから（多行を保持）、`--stdin` パイプから、`--from-env [PATH]` で `.env` を一括移行、`--protect` でマスターパスフレーズ暗号化。
  - `file_input_suffixes`（既定 `*_JSON`）に合致するキーはファイルパス入力を促します。
  - 例: `printf '%s' "$VAL" | kinko set STRIPE_API_KEY --stdin`
- `kinko get <KEY>` `[-q|--quiet] [--force]`
  - 1 キーを stdout に出力（スクリプト連携用）。保護キーはマスターパスフレーズを要求。
  - 端末への直接出力は誤露出防止のため `--force` が必要。`--quiet` は末尾改行なし。
  - 例: `export TOKEN="$(kinko get API_TOKEN -q)"`
- `kinko list` / `kinko ls`
  - 設定の各キーの登録状況（`✓`/`✗`、保護キーは `🔒`）と件数のみ表示。**値は絶対に出しません。**
- `kinko delete <KEY>` / `kinko rm <KEY>` / `kinko del <KEY>`
  - 1 キーを削除します。
- `kinko protect <KEY>|--all` / `kinko unprotect <KEY>|--all`
  - 保管済みシークレットをマスターパスフレーズで暗号化/復号化。標的型攻撃対策（`kinko get` を叩かれても復号にパスフレーズが要る）。
  - 暗号化は XChaCha20-Poly1305、鍵は Argon2id でパスフレーズから導出。パスフレーズは保存しない。
  - `kinko run` は保護キーが複数あっても**パスフレーズ入力は1回**。`KINKO_PASSPHRASE` で非対話も可（CI 用・安全性は下がる）。
- `kinko migrate [PATH]` `[--dry-run] [--protect]`
  - `.env`（既定 `.env`）から設定キーのみを一括移行。`legacy_env_map` を適用し、空値は skip。
  - `--dry-run` はキー名と件数のみ表示（値は出ません）。`--protect` で取込値を暗号化。移行後も `.env` は自動削除しません。
- `kinko docker secrets-yaml`
  - 設定の `docker_secret_keys` から Compose の `secrets:` ブロックを生成して stdout 出力。
- `kinko docker entrypoint --dir <DIR> -- <CMD>`
  - コンテナ内ヘルパー。`<DIR>`（既定 `/run/secrets`）配下のファイルを同名環境変数に展開して `<CMD>` を exec。
- `kinko prefix show` / `kinko prefix set <P>`
  - 解決済み prefix の確認 / 設定ファイルへの書込。
- `kinko keys list` / `kinko keys path`
  - 設定のキー一覧 / 使用中の設定ファイルパスを表示。

グローバルフラグ（全サブコマンド共通）: `--prefix <P>` / `--config <PATH>` / `--backend <keyring|pass|auto>`。
`-v` / `--version` はバージョンを表示します（kinko に verbose モードは無いため `-v` をバージョンに割り当てています）。

## 設定ファイル `kinko.toml`

```toml
prefix = "myapp"               # 省略時は git リポジトリ名から自動導出
backend = "auto"               # keyring | pass | auto（Linux のみ意味あり）
keys = ["DATABASE_URL", "STRIPE_API_KEY", "GCP_SERVICE_ACCOUNT_JSON"]
docker_secret_keys = ["STRIPE_API_KEY"]
file_input_suffixes = ["_JSON"]

[legacy_env_map]               # migrate 時の旧キー名マッピング（任意）
GCP_SERVICE_ACCOUNT_JSON = "GOOGLE_CREDENTIALS"
```

- 設定探索順: `--config` → `$KINKO_CONFIG` → カレントから上位の `kinko.toml`（git ルートで打ち切り） → OS 標準の設定ディレクトリ。
- prefix 導出順: 設定 `prefix`（または `--prefix`） → `$KINKO_PREFIX` → `git remote.origin.url` のリポジトリ名 → git トップレベル名 → `kinko`。

## ユースケース

### 1. ホストでアプリ/コマンドの手前に `kinko run --` を付ける（最も基本）

普段のコマンドの先頭に `kinko run --` を付けるだけ。シークレットはその子プロセス（とその子孫）にだけ
注入され、対話シェルや他のツールには載りません。実行後はキーチェーン内にのみ残り、環境には残りません。

```bash
# Python (uv)
kinko run -- uv run python manage.py runserver
kinko run -- uv run pytest

# 素の uv run で「空文字を有効値扱いされたくない」場合は未登録キーを export しない
kinko run --no-empty -- uv run python main.py

# Node / その他
kinko run -- npm run dev
kinko run -- pnpm start
kinko run -- rails server

# 特定キーだけ注入したいとき
kinko run DATABASE_URL REDIS_URL -- uv run alembic upgrade head
```

`direnv` の `dotenv` や `docker compose env_file` を、これに置き換えると平文 `.env` を消せます。

### 2. 平文 `.env` からキーチェーンへ移行する

```bash
kinko migrate --dry-run .env   # 何が登録/スキップされるか（キー名のみ）確認
kinko migrate .env             # 実際に移行
kinko ls                       # ✓/✗ で取りこぼしが無いか確認
# 案内に従い .env から該当行を削除（kinko は自動削除しません）。移行後は `kinko run --` 起動に切替
```

### 3. 多行 JSON / PEM（サービスアカウント等）を登録する

```bash
kinko set GCP_SERVICE_ACCOUNT_JSON --file ./sa.json
# 改行を壊さずに保管され、run / get でそのまま復元されます（pass/keyring 双方）
```

### 4. Docker Compose に environment ソース secret で渡す

値を `docker inspect` の Env やイメージレイヤーに残さず、`/run/secrets/<KEY>`（tmpfs）経由で渡せます。
（Compose v2.24.0+）

```yaml
# compose.yaml
services:
  app:
    secrets:
      - STRIPE_API_KEY
      - GCP_SERVICE_ACCOUNT_JSON
secrets:
  STRIPE_API_KEY:
    environment: STRIPE_API_KEY
  GCP_SERVICE_ACCOUNT_JSON:
    environment: GCP_SERVICE_ACCOUNT_JSON
```

```bash
# secrets: ブロックは設定の docker_secret_keys から生成できる（手書き不要・CI で drift 検出にも）
kinko docker secrets-yaml

# ホスト環境変数を kinko が用意して compose を起動（先頭に付けるだけ）
kinko run -- docker compose up
```

ポイント: `kinko run` は設定の全キーを（未登録なら空文字で）export するので、Compose の `environment`
ソースが「参照先環境変数が未定義」で起動失敗するのを防げます。コンテナ側のアプリは
`/run/secrets/<KEY>` を読むか、次の entrypoint で env 化します。

### 5. コンテナ内で `/run/secrets/*` を環境変数に展開して exec する

コンテナ内のアプリが「ファイルではなく環境変数」を期待する場合のヘルパー。

```dockerfile
# kinko を同梱して entrypoint に使う例
ENTRYPOINT ["kinko", "docker", "entrypoint", "--dir", "/run/secrets", "--"]
CMD ["uv", "run", "python", "main.py"]
```

`<DIR>`（既定 `/run/secrets`）配下の各ファイルを同名の環境変数に展開してから `<CMD>` を exec します。
kinko を同梱したくない場合は、等価な薄い shell スクリプトでも代替できます。

### 6. スクリプト/CI から 1 値だけ取り出す

```bash
# 端末直出力は誤露出防止で --force が必要。パイプ/リダイレクト（非 tty）なら不要
TOKEN="$(kinko get API_TOKEN -q)"
curl -H "Authorization: Bearer $(kinko get API_TOKEN -q)" https://api.example.com
```

## バックエンド

- **macOS / Windows:** 常にネイティブキーチェーン（`keyring`）。`--backend` は無視されます。
- **Linux:** 既定は Secret Service。`--backend pass`（または `auto` = `pass` が PATH にあれば優先）で `pass` を使用。

## セキュリティ上の注意

- 値が出るのは `kinko get` だけです。`list` と `migrate --dry-run` はキー名のみ。`run` も値を表示しません。
- `kinko get` を端末に直接出すと露出するため `--force` が必要です。スクリプトではパイプ/リダイレクトを使ってください。
- `prefix` を変えるとキーチェーンのサービス名が変わり、既存エントリが読めなくなります。変えたらキーを `set` し直してください。
- キー名は `^[A-Za-z0-9_]+$` のみ。これに反するキーは設定読み込み時にエラーになります。

## 参照ドキュメント

- 全体仕様 / セキュリティモデル: `README.md`（日本語: `README_ja.md`）
- ライブラリ API: `kinko-core/src/lib.rs`
- CLI 定義: `kinko/src/cli.rs`、各コマンド実装: `kinko/src/commands.rs`
- 結合テスト（実コマンドの使用例）: `kinko/tests/cli.rs`
- リポジトリ開発ガイド: `CLAUDE.md`
