---
name: skillenv
description: skillenv で agent skill を一元管理し、各 provider の skill ディレクトリへ展開するための実践ガイド。skillenv.toml による宣言（skill・source・deploy ルール・safeguard）、v0 レイアウトからの移行、fetch/link/outdated/lint の使い分け、安全性検査の意味を扱う。skillenv リポジトリ自体を触るときや、dotfiles で skill を管理するときに使用する。
---

# skillenv Skill

skillenv は agent skill を 1 箇所で宣言し、各 provider が読むディレクトリへ展開するツールです。

## まず現在どちらのレイアウトか確認する

skillenv には 2 世代あり、**`skillenv.toml` があるかどうか**で挙動が変わります。

```bash
ls skillenv.toml        # ある → v1。無い → v0（旧レイアウト）
```

v0 は `skillenv/{default,local,profiles}/` にディレクトリで scope を表現していました。v1 は `skillenv.toml` に宣言を集約し、skill 名前空間を平坦にしています。**v0 レイアウトはそのまま動き続けます**が、新しい機能（safeguard、gist、outdated、provider 別 frontmatter）は v1 だけにあります。

## v0 から移行する

移行は 2 段階で、1 段階目は**何も書き込みません**。

```bash
skillenv migrate                   # 計画を表示するだけ。読み取り専用
skillenv migrate --apply           # 実行する
skillenv migrate --apply --prune   # 確認後、旧 skillenv/ も削除
```

`migrate` が出力するもの:

- `skills/` へ移動する自作 skill
- `skillenv.toml` に載る managed source と、記録されていた revision
- **実際に展開されているディレクトリから推定した** `[[deploy]]` ルール
- 先に掃除される v0 の展開数
- 生成予定の `skillenv.toml` 全文

**`--apply` は旧 `skillenv/` を残します。** 結果を確認してから `--prune` してください。移行を取り消したいだけなら `skillenv.toml` と `skillenv.lock` を消せば v0 に戻ります。

`--apply` は v0 の vendored コピーから cache を種付けするので、**移行直後にネットワーク無しで `link` が通ります**。

移行が止まる条件（黙って進めずエラーにする）:

| 条件 | 理由 |
|---|---|
| `profiles/` が使われている | label への対応付けは推測になるため、手で宣言してもらう |
| `default/x` と `local/x` が両方ある | 平坦名前空間では共存できない |
| `skillenv.toml` が既にある | 移行済み |

移行後、`skillenv/remote` が git 追跡されていれば `git rm -r --cached skillenv/remote` の実行を促されます。`.gitignore` だけでは追跡が外れないためです。

## v1 の日常操作

```bash
skillenv list             # 宣言されている skill を source・label つきで一覧
skillenv lint             # frontmatter の妥当性と安全性検査
skillenv link             # 展開する
skillenv outdated         # remote と比べて古いか（読み取り専用、書き込みなし）
skillenv fetch            # lock の revision で cache を復元
skillenv fetch --update   # remote の最新に移動する
```

**新しいマシンでは先に `fetch` が必要です。** cache（`.skillenv/cache/`）は git 管理外なので、clone 直後は manifest と lock だけがあります。`fetch` 無しで `link` すると、remote skill が「cache に無い」と名指しで報告されます。

`link` は `--quiet` でも**警告を stderr に出し、問題があれば非 0 で終了します**。shell フックが実行するのはこの形なので、展開できなかった skill が無音で消えないようにするためです。

## skillenv.toml

```toml
[skillenv]
version = 1

# --- 自作 skill: skills/<name>/SKILL.md を読む ---
[[skill]]
name = "japanese-tech-writing"
source = "local"
labels = ["writing"]

# --- gist: frontmatter が無いので description を補う ---
[[skill]]
name = "jp-writing-upstream"
source = "gist:fd287c3133457c4fd8f5601d34aa817d"
description = "日本語技術文書の文章規範"
labels = ["writing"]

# --- 1 つの source から複数 skill ---
[[source]]
name = "igtm-skills"
from = "github:igtm/skills"
ref = "main"
skills = ["user-context"]   # "*" で全追従
labels = ["tools"]

# --- どこへ展開するか ---
[[deploy]]
target = "claude:home"      # ~/.claude/skills
include = ["*"]

[[deploy]]
target = "claude:repo"           # 実行中の repo の .claude/skills
include = ["writing"]
when.repo = "~/tmp/kaijin-web"   # この repo でだけ有効

[safeguard]
on_critical = "block"       # 既定
on_high = "warn"
allow = ["W012:figma-to-code:sha256:abc123..."]
```

### source の書き方

| 形式 | 意味 |
|---|---|
| `local` | `skills/<name>/` |
| `gist:<id>` | gist（git repo として clone される） |
| `github:owner/repo` | GitHub |
| `path:../shared` | ローカルパス |
| `git@...` / `https://...` | 任意の git remote |

### target の書き方

`<provider>:<scope>` 形式。scope は `home`（`$HOME` 配下）か `repo`（実行中の repo）です。

| provider | ディレクトリ |
|---|---|
| `claude` | `.claude/skills` |
| `agents` | `.agents/skills`（Agent Skills open standard。多くの tool が読む） |
| `codex` | `$CODEX_HOME/skills`（既定 `~/.codex/skills`） |
| `opencode` | `.opencode/skills` |

**`agents` は「codex 用」ではありません。** 共有の標準ディレクトリで、Codex 自身は `~/.codex/skills` を読みます。

provider ごとに frontmatter が変わります。Claude 系は `compatibility` を受けますが Codex 系は拒否するので、その場合は落としたキーが note として報告されます。Codex では frontmatter に置けない情報は `agents/openai.yaml` サイドカーへ出ます。

### skill 名 (id) の規則

- `[a-z0-9-]` のみ、32 文字以内、先頭末尾と連続のハイフン不可
- **非 ASCII は自動変換せずエラー**。`skillenv.toml` で明示的な ASCII の id を付ける
- 大文字小文字を区別せず一意（macOS は既定で case-insensitive なので、区別すると展開時に衝突する）
- 32 文字の上限は、生成名 `skillenv-<repo>-g<hash>-<id>` が provider の 64 文字上限に収まるようにするため。repo ディレクトリ名が長いと超過するので、その場合は `link` が該当 skill を上限値つきで skip する

### `skills = "*"` を使うかどうか

`"*"` は「この source が持つ skill すべてに追従する」意味で、解決結果は `skillenv.lock` に記録されます。明示リストは固定です。

移行では**明示リストが選ばれます**。v0 は「全件追従」と「手書きの列挙」を同じ形で記録していたため区別できず、`"*"` にすると移行直後に未レビューの skill が一気に入ってしまうからです。追従したい source だけ手で `"*"` に変えてください。

## safeguard

skill は agent の文脈に直接読み込まれる指示文なので、供給経路として検査します。検出コードは Snyk agent-scan の体系に揃えてあります。

| code | 内容 | 既定 |
|---|---|---|
| E004 | 文脈を上書きする指示（隠し命令） | critical → block |
| E005 | ダウンロードを shell に直接パイプ | high |
| E006 | 秘密情報を読んで送り出す指示 | critical → block |
| W007 | 秘密情報の読み取り指示（送出先不明） | high |
| W008 | 資格情報リテラルの埋め込み | high |
| W012 | 実行時に外部 URL から指示を取得 | high |
| W021 | 不可視 Unicode | medium、条件付きで critical |

**W021 は語彙ではなく構造で判定します。** Unicode Tags（`U+E0000`–`U+E007F`）や zero-width によるステガノグラフィを、連続長・種類の混在・デコード可能性で評価し、デコードできた場合は隠されていた文面を findings に出します。絵文字の ZWJ、`U+3000`、`U+00A0` は発火しません。

**E004/E006 も語彙では判定しません。** `.env` を説明するのは文書であり、「読んで返答に含めろ」は findings です。fenced code block 内は severity が下がり、loopback ホスト（`127.0.0.1` など）は外部の指示源として扱いません。

`block` された skill は**展開されず、既存の展開も消されません**。そうしないと、上流を乗っ取った側が意図的に検査を踏ませて skill を消せてしまいます。

`allow` は content digest に束縛されます（`<code>:<skill>:<digest>`）。内容が変われば抑制は失効します。

## 展開の仕組みと、触ってよいもの

生成されるディレクトリは `skillenv-<repo>-<id>`（repo scope）または `skillenv-<repo>-g<hash>-<id>`（home scope）です。`$HOME` はマシン全体で共有されるので、hash が repo を区別します。

各ディレクトリには `.skillenv-generated.json`（marker）が入ります。**marker が「skillenv が作った」ことの唯一の証拠**で、これが無いディレクトリは決して削除されず、報告されるだけです。手で置いたものは安全です。

`skillenv link` は marker を**最初に**書きます。生成が途中で失敗しても残骸は自分のものと認識され、次回の実行で置き換わります。

## Rust library として使う

```rust
use skillenv::{apply_migration, fetch_manifest, has_manifest, link_manifest,
               lint_manifest, list_manifest, outdated_manifest, plan_migration,
               remove_legacy, scan_skill_text, sweep_legacy};

// v1 か v0 かを判定する
if has_manifest(".") {
    let report = link_manifest(".")?;
    for warning in report.warnings() {
        eprintln!("{warning}");
    }
    if report.has_problems() { /* 非 0 終了に反映する */ }
}

// SKILL.md 単体を検査する
for finding in scan_skill_text(&text) {
    if finding.blocks_by_default() { /* 既定では展開されない */ }
}
```

## 実装を読む場所

| 関心 | ファイル |
|---|---|
| `skillenv.toml` のパースと id 検証 | `src/manifest.rs` |
| `skillenv.lock`、content digest | `src/lock.rs` |
| 平坦カタログ、id 一意性 | `src/catalog.rs` |
| provider 別 frontmatter、target 解決 | `src/provider/` |
| 取得、gist、ls-remote、取得時検査 | `src/source/` |
| 展開、marker、skill 単位の隔離 | `src/deploy.rs` |
| 安全性検査 | `src/safeguard/` |
| 全体の組み立て | `src/session.rs` |
| v0 の掃除と移行 | `src/legacy_sweep.rs`, `src/migrate.rs` |

## 判断基準

- **v0 レイアウトのまま使い続けてよいか** — 動くが、safeguard も gist も outdated も効かない。移行は 1 コマンドで、`--prune` するまで取り消せる
- **`link` が「unavailable」と言う** — cache が無い。`skillenv fetch`
- **`link` が skill を skip する** — その skill 固有の問題（frontmatter 不正、target 衝突、名前長超過）。理由が出力され、他の skill は展開済み。`skillenv lint` で先に見つけられる
- **`link` が失敗して止まる** — I/O 障害（書き込み不能、容量不足）。全 skill に影響するので即座に止める設計
- **`outdated` で古いと出た** — `skillenv fetch --update`。ただし上流で skill が改名・削除されていると該当 skill だけ報告される
- **private repo が fetch できない** — 認証は skillenv では扱わない。無人実行で固まらないよう、プロンプトは常に無効化してある
