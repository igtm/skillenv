---
name: skillenv
description: skillenv で agent skill を一元管理し、各 provider の skill ディレクトリへ展開するための実践ガイド。skillenv.toml による宣言（skill・source・deploy ルール・safeguard）、v0 レイアウトからの移行、fetch/link/outdated/lint の使い分け、安全性検査の意味を扱う。skillenv リポジトリ自体を触るときや、dotfiles で skill を管理するときに使用する。
---

# skillenv Skill

skillenv は agent skill を 1 箇所で宣言し、各 provider が読むディレクトリへ展開するツールです。

## まず `skillenv.toml` があるか確認する

```bash
ls skillenv.toml        # 無ければ移行が必要
```

**1.0 で v0 レイアウト（`skillenv/{default,local,profiles}/`）の実行経路は削除されました。** `skillenv.toml` が無いリポジトリで動くのは `migrate` だけです。v0 を理解しているコードは移行専用に残してあります。

## v0 から移行する

移行は 2 段階で、1 段階目は**何も書き込みません**。

```bash
skillenv migrate           # 計画を表示するだけ。読み取り専用
skillenv migrate --apply   # 実行する（旧 skillenv/ は残す）
skillenv migrate --prune   # 結果を確認してから、旧 skillenv/ を削除
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
skillenv init             # skillenv.toml と skills/ と .gitignore を用意する
skillenv list             # 宣言されている skill を source・label つきで一覧
skillenv lint             # frontmatter の妥当性と安全性検査
skillenv fetch            # lock の revision で cache を復元
skillenv fetch --update   # remote の最新に移動する
skillenv link             # 展開する
skillenv status           # 各 target に何が展開されているか
skillenv doctor           # どの manifest / cache / target に解決されたか
skillenv outdated         # remote と比べて古いか（読み取り専用、書き込みなし）
skillenv diff <name>      # cache / 展開済み / remote の 3 者を比較する
skillenv remove <name>    # manifest と lock から外し、展開先も掃除する
skillenv unlink           # この manifest の展開をすべて削除する
skillenv skills           # 各 tool から見える skill（管理外も含む）
```

`status` と `skills` は役割が違います。**`status` は「この manifest が置いたもの」**、**`skills` は「各 tool から見えるもの全部」**です。手で置いた skill は後者にしか出ません。

`doctor` は「なぜそこに行ったのか」に答えます。展開先が想定と違うとき、あるいはどこにも展開されないときはこれを見ます。

`outdated` は「この source が動いた」まで、**`diff` は「何が動いたか」**を出します。lock と remote の revision、展開済みが cache と同じ内容から来ているか、違う場合は SKILL.md の差分です。内容の比較はネットワーク無しで動き、remote の revision だけが通信を要します。

差分は**本文だけ**を比べます。frontmatter は provider ごとに書き換わり `name` は生成ディレクトリ名なので、含めると毎回「変更ではない差分」が出てしまいます。

**新しいマシンでは先に `fetch` が必要です。** cache（`.skillenv/cache/`）は git 管理外なので、clone 直後は manifest と lock だけがあります。`fetch` 無しで `link` すると、remote skill が「cache に無い」と名指しで報告されます。

`link` は `--quiet` でも**警告を stderr に出し、問題があれば非 0 で終了します**。shell フックが実行するのはこの形なので、展開できなかった skill が無音で消えないようにするためです。

ただし**manifest が見つからないときは `--quiet` なら無言で成功します**。フックは `cd` ごとに走るので、管理外のディレクトリでエラーを出していたらフック自体が外されてしまいます。手で打った `skillenv link` は従来どおり理由を出して失敗します。

## 他の repo へ展開する（フックを効かせる）

`target = "*:repo"` は「いま立っている repo」を指しますが、`link` は manifest を**上位ディレクトリに遡って**探します。dotfiles の外に `cd` すると manifest が見つからず、`*:repo` のルールは発火しません。

manifest の位置を明示すると、どの repo にいても解決されます。

```bash
export SKILLENV_MANIFEST="$HOME/tmp/dotfiles/skillenv.toml"
eval "$(skillenv hook zsh)"
```

こうすると `cd ~/work/foo` で `~/work/foo/.claude/skills` に展開されます。特定の repo だけに限りたいときは `when.repo` を付けます。

## skillenv.toml

### 短い書き方（source ごとのリスト）

skill を並べるだけなら `[skills]` が最短です。キーが source、値がそこから取る skill です。

```toml
[skillenv]
version = 1

[skills]
local = [
  "draft-pr", "japanese-tech-writing", "gof-design-patterns",
]
"github:igtm/skills" = ["visual-explainer", "user-context"]
"gist:fd287c31" = ["jp-writing"]
"github:igtm/kinko" = ["*"]        # その source の全 skill

[[deploy]]
target = "claude:home"
include = ["*"]
```

**source 名はキーから導出されます**（repository 名か gist id）。`via=` の表示と cache ディレクトリ名になるので、既存の lock と揃うかは確認してください。`https://github.com/openclaw/agent-skills` は `agent-skills` になります。名前を固定したい、あるいは `ref` や `labels` が必要な source は、従来の `[[source]]` で書けば併用できます。

`local` に `["*"]` は書けません。`skills/` に置いてあるもの全部という意味になり、意図と違うものを拾うためです。追従したいなら `path:` を使ってください。

### 詳しい書き方（1 つずつ）

`description` や `labels` を skill 単位で付けるならこちらです。

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
| `path:../shared` | ローカルのツリー。`fetch` 不要で、その中から skill を探します |
| `gist:<id>` | gist（git repo として clone される） |
| `github:owner/repo` | GitHub |
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

`"*"` は「この source が持つ skill すべてに追従する」意味です。メンバーの決まり方は source の種類で違います。

- **remote**（github/gist/git URL）は `fetch` が発見して `skillenv.lock` に記録します。**新しいマシンでは `fetch` の前は 0 件**です
- **`path:`** はツリーがディスク上にあるので、`fetch` を待たず直接読みます（`fetch` は `path:` を何もしません）

`"*"` のメンバーが既存の id と衝突した場合、**その skill だけを報告して残りは展開します**。上流が他所と同じ名前を採用するのは利用者の落ち度ではないので致命的にはしません（ここで manifest を開けなくすると、直す手段である `remove` まで道連れになります）。

**`"*"` のメンバーが上流から消えたら lock からも消えます。** 「この source が持つもの全部」の定義がツリーそのものなので、消えたものは消えたものです。残すと展開できないのに毎回 catalog に載り、`link` が「unavailable」を出し続け、**しかも unavailable なので既存の展開が削除されます**。`remove` でも直せません（wildcard のメンバーは manifest にエントリを持たないため）。明示リストは逆で、名前を指定したのは利用者なので**消えても報告するだけ**です。

明示リストは固定です。

移行では**明示リストが選ばれます**。v0 は「全件追従」と「手書きの列挙」を同じ形で記録していたため区別できず、`"*"` にすると移行直後に未レビューの skill が一気に入ってしまうからです。追従したい source だけ手で `"*"` に変えてください。

## 新しすぎる revision を取らない

`uv` のリリース経過時間の設定と同じ考え方です。上流が乗っ取られても数時間から数日で気づかれるのが普通なので、公開から agent の文脈に入るまでに待ち時間を挟みます。

```toml
[fetch]
minimum_revision_age = "7d"   # s / m / h / d / w
```

`fetch --update` は tip ではなく**その時間以上経過した最新の revision**を取ります。動いたことは黙らず報告します。

```
note: up took 9cb236b3c034 rather than f3aa484bf5fc: nothing newer is 7d old yet
```

判定は committer 日付です。挙動で押さえておく点が 3 つあります。

- **pin された revision には適用されません。** `fetch`（`--update` なし）は lock の revision を復元するので、そこで年齢を再判定すると「書いた時点では問題なかった lock からの復元」を拒否してしまいます
- **該当する revision が無ければエラーです。** tip に黙って落とすと、設定が効いているように見えて何もしていない状態になります
- **lock は後退しません。** すでに新しい revision を指している lock は維持され、`note:` で報告されます。これが無いと、設定を有効にした瞬間に案内 skill が 1.0 前の版に巻き戻ります
- **`outdated` は日付を見ません。** `ls-remote` は sha しか返さないので、「tip は動いたが `fetch --update` はそれを取らないかもしれない」と明示します

`path:` と `local` には revision が無いので対象外です。ローカルの git repo を history 込みで参照したい場合は `file://` で書けます。

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

**skill 内の symlink は拒否されます。** `fs::copy` は walk がリンクを辿らなくてもリンク先を開いて複製するので、`notes.md` という名前で `~/.ssh/id_rsa` を指すリンクがあると、その中身が agent が読むディレクトリに展開されてしまいます。`local` と `path:` の skill は取得時検査を通らないため、展開直前が唯一の関門です。該当 skill だけ skip され、他は展開されます。

`skillenv link` は marker を**最初に**書きます。生成が途中で失敗しても残骸は自分のものと認識され、次回の実行で置き換わります。

削除は marker が「この manifest のものだ」と言っている場合だけです。marker が無いディレクトリ、他の manifest の marker を持つディレクトリは、`status` に出るだけで消されません。`$HOME` は複数リポジトリで共有されるので、この判定を prefix だけに任せると別リポジトリの展開を消してしまいます。

`link` は safeguard の判定結果を `skillenv.lock` に記録しますが、**内容が変わっていなければ書き込みません**。shell フックは `cd` ごとに `link` を走らせるので、毎回 lock を書き換えると git 管理下のファイルが常に差分ありになってしまいます。

## Rust library として使う

```rust
use skillenv::{apply_migration, doctor_manifest, fetch_manifest, has_manifest,
               init_manifest, link_manifest, lint_manifest, list_manifest,
               outdated_manifest, plan_migration, remove_from_manifest,
               scan_skill_text, status_manifest, unlink_manifest};

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
| コマンド表面 | `src/main.rs` |
| v0 の掃除と移行 | `src/legacy_sweep.rs`, `src/migrate.rs` |

## 判断基準

- **v0 レイアウトのまま使い続けてよいか** — 1.0 では動かない。`skillenv migrate --apply` が必須。`--prune` するまで取り消せる
- **`link` が「unavailable」と言う** — cache が無い。`skillenv fetch`
- **`link` が skill を skip する** — その skill 固有の問題（frontmatter 不正、target 衝突、名前長超過）。理由が出力され、他の skill は展開済み。`skillenv lint` で先に見つけられる
- **`link` が失敗して止まる** — I/O 障害（書き込み不能、容量不足）。全 skill に影響するので即座に止める設計
- **`outdated` で古いと出た** — `skillenv fetch --update`。ただし上流で skill が改名・削除されていると該当 skill だけ報告される
- **private repo が fetch できない** — 認証は skillenv では扱わない。無人実行で固まらないよう、プロンプトは常に無効化してある
