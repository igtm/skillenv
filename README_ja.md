# skillenv

[English README](./README.md)

`skillenv` は、agent skill の取得・バージョン管理・展開を 1 つの `skillenv.toml`
から行うツールです。skill の出どころは自分の `skills/` ディレクトリ、GitHub
リポジトリ、gist、ローカルパスのいずれかで、展開先は各 agent が読むディレクトリ
——`.claude/skills`、`.agents/skills`、`$CODEX_HOME/skills`、`.opencode/skills`
——です。展開時に frontmatter は provider ごとに書き換えられ、すべての skill は
書き込み前に検査されます。

提供するもの:

- 何が存在し、どこから来て、どこへ行くかを宣言する、手書きの manifest 1 つ
- 各 source の解決結果を記録する lock ファイル（別マシンで同じ状態を再現するため）
- provider ごとの frontmatter（公式バリデータ同士で許可キーが一致しないため）
- Snyk `agent-scan` のコード体系による、skill の供給経路検査
- リポジトリ移動時に relink する shell hook
- 同じ操作を呼び出せる Rust ライブラリ

現在のバージョンは `1.0.0` です。

## 1.0 は破壊的リリースです

v0 のレイアウト——`skillenv/{default,local,profiles}/` でディレクトリとして scope
を表現し、`skillenv.lock.json` を使い、その上に `add` / `update` / `global` を
載せていたもの——は**もう動きません**。skill を対象に動くコマンドはすべて
`skillenv.toml` を読み、無ければ
`no skillenv.toml found from <dir> upwards; create one or set SKILLENV_MANIFEST`
で停止します。

**移行していないリポジトリでは `skillenv migrate --apply` の実行が必要です。**
旧レイアウトを理解するのは `migrate` だけです。詳細は
[v0 から移行する](#v0-から移行する)を参照してください。

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
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -v=v1.0.0
```

Cargo で GitHub からインストールする場合:

```bash
cargo install --git https://github.com/igtm/skillenv.git --locked
```

ローカル checkout からインストールする場合:

```bash
cargo install --path . --locked
```

## はじめかた

1. manifest を置きたい場所——通常は dotfiles リポジトリ——で `skillenv init` を 1 回だけ実行する
2. skill、その source、展開先を `skillenv.toml` に宣言する
3. `skillenv fetch` で cache を用意し、`skillenv link` で展開する
4. 上流が動いたかは `skillenv outdated`、新しい素材を信用する前には `skillenv lint`

```text
skillenv.toml            手で書く唯一のファイル
skillenv.lock            各 source の解決結果。commit する
skills/<name>/SKILL.md   自分で書く skill
.skillenv/cache/         取得した source。commit しない
```

manifest を読むコマンドは、作業ディレクトリから上に向かって manifest を探します。
manifest があるリポジトリの任意のサブディレクトリから実行できます。
`SKILLENV_MANIFEST` にファイルパスを与えると、その探索を上書きできます。

`skillenv init` は `.gitignore` にも次の行を加えます。cache と生成ディレクトリを
`git status` に出さないためです。

```text
.skillenv/
.agents/skills/skillenv-*
.claude/skills/skillenv-*
.opencode/skills/skillenv-*
```

既存の `skillenv.toml` は決して上書きしません。手で書く入力はこのファイルだけ
なので、テンプレートで置き換えると設定全体が失われます。また `init` は何も展開
しません。skill を宣言したら `skillenv link` を実行してください。

## コマンド

```bash
skillenv init
skillenv link [--quiet]
skillenv unlink [--quiet]
skillenv status
skillenv list
skillenv remove <name>
skillenv migrate [--apply] [--prune]
skillenv outdated
skillenv diff <name>
skillenv lint
skillenv fetch [--update]
skillenv skills [--tool <claude|codex|opencode|antigravity>]... [--repo-tree] [--json]
skillenv doctor [--json]
skillenv hook <zsh|bash>
skillenv version
```

`skillenv <command> --help` で、同じ理由づけをより詳しく読めます。

### link と unlink

`link` は、各 `[[deploy]]` ルールが選んだ skill を、そのルールが名指しした
ディレクトリへ展開します。同じディレクトリに解決したルールは選択を**合併**します。
そうしないと、2 つのルールが毎回互いの成果を消し合うことになります。`when.repo`
付きのルールはそのリポジトリの中でだけ効きます。これがあるので、ディレクトリ移動
フックから実行する意味が出ます。

失敗は skill 単位です。`SKILL.md` の不正、名前の衝突、safeguard による保留は、
報告してその skill を skip するだけで、残りは展開されます。全体を止めるのは
systemic な I/O 障害だけです。

警告は stderr に出て、問題があれば**`--quiet` でも**終了コードが非 0 になります。
shell hook が実行するのはこの形なので、展開できなかった skill がそこで無音に
なってはいけません。

`unlink` が削除するのは、marker がこの manifest を指しているディレクトリだけです。
`skillenv-` の prefix を持つが marker が無いもの、あるいは別の manifest の marker
を持つものは、報告してそのまま残します。

### status

この manifest が展開先とする各ディレクトリについて、`skillenv-` で始まる
ディレクトリをすべて報告します。別の manifest のものや、prefix はあるが marker が
無いものも含みます。それらは決して削除しません——marker が無ければ skillenv が
作った証拠が無いからです——し、隠すと件数が `ls` と食い違います。

ルールが選んでいるのにディスク上に無い skill は、名前を挙げて報告します。よくある
原因は cache を取得していないことです。

### fetch

manifest が宣言する remote source について `.skillenv/cache/` を用意します。

`--update` なしのときは、`skillenv.lock` が記録している revision をそのまま復元
します。clone 直後に必要なのはこれです。cache は commit しないので、新しいマシンに
あるのは manifest と lock だけです。`--update` を付けると、各 ref が今指している
ものへ移動し、lock を書き換えます。何が動くかは先に `skillenv outdated` で見て
ください。

lock は最後に 1 回ではなく source ごとに保存します。途中で到達できない source が
あっても、インストール済みのツリーと記録された revision が食い違った状態を残さない
ためです。

取得するツリーには 500 ファイル、1 ファイル 2 MiB、合計 16 MiB の上限があります。
敵対的な source や、事故で巨大になった source が、ディスクを埋めたり shell hook を
止めたりしないようにするためです。`.git` と `.DS_Store` は決してコピーしません。

### outdated

読み取り専用です。各 remote に `git ls-remote` で問い合わせるだけで、cache も lock
も触りません。古いことは失敗ではなく状態なので、どちらでも終了コードは 0 です。
陳腐化で落としたい CI は出力を突き合わせてください。

### diff

`outdated` は「source が動いた」までを、`diff` は「何が動いたか」を出します。lock の
revision と remote の現在位置、各展開が cache の現在の内容から来ているか、違う場合は
`SKILL.md` の差分です。ネットワークが必要なのは remote の revision だけで、内容の
比較はオフラインで動きます。手を打てるのはそちら側なので、そこが重要です。

**本文だけを比べます。** frontmatter は provider ごとに書き換わり `name` は生成
ディレクトリ名なので、含めると毎回「変更ではない差分」が出てしまいます。

cache が無い場合、あるいは marker が digest を記録していない場合は、一致を主張せず
「比較できない」と言います。存在しないことは一致ではありません。この manifest の
marker を持たないディレクトリは、この skill の展開としては報告しません——`status` と
`link` が既にそれを自分のものと認めないのと揃えています。

### lint

宣言されている skill をすべて検査し、何か見つかれば非 0 で終了します。`link` も
同じ検査を実行して critical でブロックするので、`lint` は展開前にそれを見るための
コマンドです。frontmatter がパースできない場合——skill が展開されない最も多い原因
——も報告し、`SKILL.md` が無い場合は `W014 [low]` として報告します。

### remove

`skillenv.toml` をその場で編集し、コメントと残るエントリの順序をすべて保ったまま、
続けて relink して、消したエントリのディレクトリも一緒に片付けます。`[[source]]`
を名指しすると、その source が持ち込んだ skill すべてが消えます。

manifest を先に編集してから relink します。逆順だと、relink がまだエントリを見て
しまい、出ていく途中でもう一度展開してしまいます。

### skills

「ここから、この tool には実際どの custom skill が見えているのか」に答えます。
managed かどうかは問いません。この manifest が置いたものを知りたいときは
`skillenv status` を使ってください。

探索先:

- `codex`: 現在の repo の `.agents/skills`、`$HOME/.agents/skills`、`/etc/codex/skills`
- `claude`: 現在の repo の `.claude/skills`、`$HOME/.claude/skills`
- `opencode`: 現在の repo の `.opencode/skills`、`.claude/skills`、`.agents/skills`、および `$HOME` 側の global パス
- `antigravity`: repo ルートの `.agents/skills`、旧 `.agent/skills`、`$HOME/.gemini/antigravity/skills`

既定では作業ディレクトリから見えているものを報告します。`--repo-tree` を付けると、
今は見えていないネストした tool ディレクトリも repo 全体から拾います。`--json` は
安定した機械可読の報告を出します。

### doctor

`status` が「何が展開されているか」に答えるのに対し、`doctor` は「なぜそこへ行った
のか」に答えます。報告する内容は、このディレクトリを支配している `skillenv.toml`
と解決されたリポジトリ、home ディレクトリと cache パスおよび cache 済み source 数、
manifest が宣言する skill 数と deploy ルール数に対する lock の記録数、そして解決
された各 target とその provider・展開数です。`--json` で同じ内容を安定した形で
出せます。

## skillenv.toml

```toml
[skillenv]
version = 1

# 自作の skill: skills/<name>/SKILL.md を読む
[[skill]]
name = "japanese-tech-writing"
source = "local"
labels = ["writing"]

# gist には frontmatter が無いので、description をここで補う
[[skill]]
name = "jp-writing-upstream"
source = "gist:fd287c3133457c4fd8f5601d34aa817d"
description = "日本語技術文書の文章規範"
labels = ["writing"]

# 1 つの source から複数の skill
[[source]]
name = "igtm-skills"
from = "github:igtm/skills"
ref = "main"
skills = ["user-context"]   # "*" にすると、その source の全 skill に追従する
labels = ["tools"]

# どこへ展開するか
[[deploy]]
target = "claude:home"           # ~/.claude/skills
include = ["*"]

[[deploy]]
target = "claude:repo"           # 実行中のリポジトリの .claude/skills
include = ["writing"]
exclude = ["jp-writing-upstream"]
when.repo = "~/tmp/kaijin-web"   # このリポジトリでだけ有効

[safeguard]
on_critical = "block"            # 既定
on_high = "warn"
allow = ["W012:figma-to-code:sha256:abc123…"]
```

`version` は `1` でなければなりません。`[skillenv]` テーブル自体を省略した場合も
同じ意味になります。ファイル中のどこであれ、未知のキーは無視ではなくエラーです。

### source の書き方

| 形式 | 意味 |
|---|---|
| `local` | manifest と同じ場所の `skills/<name>/` |
| `gist:<id>` | gist。他と同じく git リポジトリとして clone する |
| `github:owner/repo` | GitHub。末尾の `.git` は許容する |
| `path:../shared` | このマシン上のパス |
| `git@…` / `ssh://…` / `https://…` / `.git` で終わる文字列 | そのまま git remote として渡す |

取得したツリーの中では、ルート自身、`<id>/`、`skills/<id>/`、
`.agents/skills/<id>/` の順に skill を探します。実際に存在するレイアウトを
カバーするためです。

`description` は skill 自身の frontmatter を上書きする指定で、source が frontmatter
を持たない場合——典型的には gist——にここで与えます。provider はいずれも description
を要求するので、manifest にも frontmatter にも無ければ、`link` は妥当にならない
ファイルを書くのではなく `Instructions for the <id> skill.` を合成します。この一文は
agent が skill を読み込むか判断するときに読む説明文なので、実際の説明を宣言して
ください。

### target と provider

target は `<provider>:<scope>` 形式です。scope は `home`（`$HOME` 配下。マシン上の
全リポジトリで共有）か `repo`（link 対象のリポジトリ）です。

| provider | ディレクトリ |
|---|---|
| `claude` | `.claude/skills` |
| `agents` | `.agents/skills` |
| `codex` | `$CODEX_HOME/skills`（既定 `~/.codex/skills`） |
| `opencode` | `.opencode/skills` |

**`.agents/skills` は Codex の宛先ではありません。** これは多くの tool が読む
Agent Skills open standard のディレクトリで、だからこそ独立した provider です。
Codex 自身が読むのは `$CODEX_HOME/skills` です。`codex:repo` はリポジトリ内の
`.codex/skills` に解決されます。

opencode は `.claude/skills` と `.agents/skills` も読みます。したがって opencode
へ直接展開する意味があるのは、その skill を opencode にだけ見せ、それらの
ディレクトリを共有する他の tool には見せたくない場合です。

すべての tool に共通するキーは `name` と `description` だけなので、frontmatter は
provider ごとに書き換えます。Claude・`agents`・opencode は `license`、
`allowed-tools`、`metadata`、`compatibility` を受け付けます。Codex のバリデータは
`compatibility` を拒否するので、黙って捨てるのではなく、落としたキーとして報告
します。`allowed-tools` は取り込み時に正規化します——スペース区切りの文字列、
カンマ区切りの文字列、inline sequence、block sequence の 4 形式が実在する skill に
現れます——そのうえで Claude・`agents`・Codex にはスペース区切りの文字列、opencode
には sequence として書き出します。

Codex にはさらに `agents/openai.yaml` サイドカーを出し、`metadata` の表示系キー
——`short-description`、`display-name`、`brand-color`——を `interface` 以下へ移します。
frontmatter は `name` と `description` に限り、product 固有の設定はサイドカーに置く
（agent ではなく harness が読む設定として）というのが Codex 自身の指針なので、これ
らのキーは捨てずにそこへ回します。入れるものが無い skill にはサイドカーを作りません。

複数の provider が同じディレクトリに解決した場合は、許可キーが最も少ない provider
の形で書き出します。そうすればどの provider から見ても妥当な出力になります。

### deploy ルール

`include` は必須です。include が無いルールは何も展開しないので、意図したルールと
いうより壊れたルールに見えます。全 skill を意味するときは `include = ["*"]` と
書いてください。`exclude` は `include` を上書きします。`*` 以外の selector は
skill の label と id の両方に一致するので、1 つの skill を指すために label を
作る必要はありません。

`when.repo` はルールを 1 つのリポジトリに限定します。`~` は展開され、末尾の `/**`
はサブツリーに一致するので、`when.repo = "~/work/**"` は `~/work` 以下すべてを
覆います。省略は「すべてのリポジトリ」で、`home` target では通常こちらです。

### skill の id

- `[a-z0-9-]` のみ、32 文字以内、先頭・末尾・連続のハイフン不可
- **非 ASCII は自動変換せずエラー**にします。v0 は名前を slugifier に通して非 ASCII
  を削り、代わりの語を当てていたため、名前の異なる日本語の skill 2 つが黙って同じ
  id になりました。`skillenv.toml` で明示的な ASCII の id を付けてください
- 大文字小文字を区別せず一意。macOS は既定で case-insensitive なので、素朴な一意性
  検査を通った `Foo` と `foo` がディスク上で衝突します
- 予約語はありません。selector の wildcard は `*`、target の scope は `:` の後にしか
  現れず、`local` は source の位置にしか現れないので、どれも曖昧ではありません
  ——実際、これらを予約していた過去の版は `skillenv` という名前の実在の skill を
  壊しました

32 文字という上限があるのは、provider が frontmatter の `name` を 64 文字までしか
受け付けず、展開後の名前が `skillenv-<repo>-g<hash>-<id>` になるからです。この上限は
早い段階で気づくための静的な見積もりで、prefix はリポジトリ名の長さで伸びるため
厳密にはなりません。`link` は実際の生成名を測り、それでも超えるものは skip します。

### `skills = "*"` と明示リスト

`"*"` は source が持つものに追従する指定です。メンバーの決まり方は source によって
違います。remote は `fetch` が発見して `skillenv.lock` に記録するので、clone 直後は
`fetch` するまで 0 件です。`path:` のツリーは直接読みます——`fetch` にダウンロードする
ものが無いためです。

**上流から消えたメンバーは lock からも消えます。** wildcard の「持っているもの全部」
という定義がツリーそのものなので、消えたものは消えたものです。残すと、毎回 catalog に
載り直すのに展開はできず、しかも wildcard のメンバーは manifest にエントリを持たない
ので名前を指定して `remove` することもできません。さらに悪いことに、展開できない状態は
`link` が展開を消す条件そのものなので、動いていた展開が失われます。

明示リストは逆です。名前を指定したのは利用者なので、消えた名前はその skill だけの報告
として残し、判断は利用者に委ねます。

id が既に使われている wildcard メンバーは、致命的にせず報告して skip します。2 つの
上流が同じ名前を採用するのは利用者の落ち度ではありませんし、ここで manifest を
読み込めなくすると、直す手段である `remove` まで道連れになります。

## safeguard

skill は他人のリポジトリから取ってきた、実行される指示文です。つまりここは
サプライチェーンです。検出コードは独自体系ではなく Snyk `agent-scan` の分類に
揃えてあり、既存のスキャナと突き合わせられます。frontmatter も本文と一緒に検査
します。`description` は agent の文脈へ先に読み込まれる一方で本文はそうではなく、
指示を隠すのに最も効く場所だからです。

| code | 内容 | 既定の severity |
|---|---|---|
| E004 | 文脈にある指示を上書きする命令 | critical |
| E005 | ダウンロードを shell に直接パイプ | high |
| E006 | 秘密情報を読んでどこかへ渡す指示 | critical |
| W007 | 秘密情報を読む指示（渡し先は不明） | high |
| W008 | 資格情報リテラルの埋め込み | high |
| W012 | 実行時に外部 URL から指示を取得 | high |
| W021 | 不可視 Unicode | medium。構築されたものに見える場合は critical |

`[safeguard]` で severity ごとに `block` / `warn` / `allow` を割り当てます。既定は
`on_critical = "block"`、残りは `warn` です。設定しなかった severity は緩くなるの
ではなく既定のままになります。`block` はその skill を展開せず、findings を stderr に
名指しします。`warn` は展開したうえで findings を `skillenv.lock` に記録します。
`lint` は policy も `allow` も適用せず、見つけた findings をすべて報告します。だから
`link` が黙って展開するものを `lint` が挙げることがあります。

判定は語彙ではなく**指示の形**で行います。ここで難しいのは検出そのものではなく、
既に使っている正当な skill で発火しないことです。秘密管理の skill は `.env` を
何度も挙げ、Figma の skill は `127.0.0.1` から取得し `curl … | sh` を説明し、PR の
skill は `gh pr create` を実行します。これらがブロックされれば機能ごと切られてしまい、
それは機能が無いより悪い結果です。ですから、秘密のパスを挙げるのは文書であり、それを
読んで返答に含めろと言うのが findings です。fenced code block 内の検出は severity
が下がり、loopback ホストは外部の指示源として扱いません。

W021 も文字種ではなく構造で判定します。Unicode Tags（`U+E0000`–`U+E007F`）の連続、
zero-width によるステガノグラフィ、閉じていない bidi override を、連続長・混在した
種類の数・デコード可能性で評価し、デコードできた場合は隠されていた文面を findings に
出します。絵文字の joiner、`U+3000`、`U+00A0` では発火しません。

ブロックされた skill は**展開されず、既存の展開も削除されません**。そうでないと、
上流を乗っ取った側が意図的に検査を踏ませて skill を消せてしまいます。

`allow` の書式は `<code>:<skill>:<digest>` で、digest は必須です。レビューした内容が
変わった時点で抑制が失効するようにするためです。

## 何が書き込まれるか

展開された skill は、repo target では `skillenv-<repo>-<id>`、`$HOME` 配下では
`skillenv-<repo>-g<hash>-<id>` というディレクトリになります。`<repo>` は manifest
ディレクトリ名を slug 化したもの、`<hash>` はその正規化パスの sha256 の先頭 12 桁
です。hash があるのは、`$HOME` がマシン上の全リポジトリで共有される一方、削除は
名前の prefix を手がかりにしているからです。これが無ければ、あるリポジトリの
`link` が別のリポジトリのエントリを消してしまいます。

**skill 内の symlink は拒否されます。** walk はリンクを辿らなくても `fs::copy` は
リンク先を開いて中身を複製するので、`notes.md` という名前で `~/.ssh/id_rsa` を指す
リンクがあれば、その中身が agent が読むディレクトリに展開されます。`local` と `path:`
の skill は取得時検査を通らないため、ここが唯一の関門です。該当 skill だけが skip され、
他は展開されます。

各ディレクトリには `.skillenv-generated.json`（marker）が入り、どの manifest の
ものか、skill、provider、revision、content digest を記録します。**marker は
skillenv が作ったことの唯一の証拠**なので、marker が無いディレクトリは削除されず、
報告されるだけです。手で置いたものは安全です。

`link` は marker を `SKILL.md` の生成や asset のコピーより**先に**書きます。v0 は
最後に書いていたため、frontmatter のパースに失敗した skill は asset だけがあって
marker が無いディレクトリを残し、以降のすべての実行がそこに触るのを拒否しました。
打ち間違い 1 つで、ある環境が 6 週間凍りついたままになりました。

`SKILL.md` はコピーではなく生成します。frontmatter に生成名と provider が受け付ける
キーを載せるためです。skill ディレクトリの他のファイルはそのままコピーされます。

## v0 から移行する

1 段階目は何も書き込みません。

```bash
skillenv migrate           # 計画を表示するだけ。読み取り専用
skillenv migrate --apply   # 実行する（旧 skillenv/ は残す）
skillenv migrate --prune   # 結果を確認してから、旧 skillenv/ を削除
```

`migrate` が報告するのは、v1 manifest に載る skill・source・deploy ルール、先に
片付ける必要がある v0 の展開、そして生成予定の `skillenv.toml` 全文です。deploy
ルールは**実際にディスク上に展開されているものから推定**します。その記録は v0 の
marker しか持っておらず、変換はその marker を壊すので、読み直す機会は二度と
ありません。

`--apply` は、marker がまだ存在するレイアウトを指している状態で v0 の展開を片付け、
続いて v0 の vendored コピーから新しい cache を種付けします。だから直後に
ネットワーク無しで `link` が通ります。旧 `skillenv/` は残るので、結果を確認してから
`--prune` してください。それより前に取り消したいだけなら、`skillenv.toml` と
`skillenv.lock` を削除すれば戻ります。

移行は推測せず、理由を名指しして止まります。

| 条件 | 理由 |
|---|---|
| `profiles/` が使われている | profile を label に対応付けるのは推測になるので、手で宣言してもらう |
| `default/` と `local/` に同じ id がある | 平坦な名前空間では共存できない |
| `skillenv.toml` が既にある | 移行済み |
| `skillenv/` が無い、または中身が無い | 移行するものが無い |

source は `"*"` ではなく**明示リスト**として移行します。v0 は「全件追従」と「手書きの
列挙」を同じフィールドに記録していたので両者を区別できず、`"*"` を選ぶと、その
source が今提供している skill 全部が未レビューで入ってきます。追従したいものだけ
手で書き換えてください。

`skillenv/remote` が git 追跡されていた場合、`migrate` は
`git rm -r --cached skillenv/remote` の実行を促します。`.gitignore` のエントリだけ
ではファイルの追跡は外れません。

## shell hook

`skillenv` はディレクトリ移動時に relink できます。

`zsh` では `~/.zshrc` に次を追加します。

```bash
eval "$(skillenv hook zsh)"
```

`bash` では `~/.bashrc` または `~/.bash_profile` に次を追加します。

```bash
eval "$(skillenv hook bash)"
```

zsh 側は `add-zsh-hook chpwd`、bash 側は `PROMPT_COMMAND` を使い、リポジトリルートが
変わったときだけ動きます。どちらも実行するのは `skillenv link --quiet` だけで、
`.gitignore` は触りません。`$HOME/.local/bin` のような場所へインストールした場合は、
hook を入れる前にそのディレクトリを `PATH` に入れてください。

## lock ファイル

`skillenv.lock` は manifest ルートに置かれる JSON です。`skillenv.toml` が*意図*を
記録するのに対し、lock はその*解決結果*を記録します。skill ごとに、書かれたままの
source、持ち込んだ `[[source]]`、解決された ref と revision、取得したツリーの digest、
そして safeguard の findings とそれを算出した digest を持ちます。digest を持つので、
revision が動いていなくても内容の変化に気づけ、古い findings は信用せず再検査します。

このファイルは commit してください。clone 直後にあるのは manifest と lock だけで、
`skillenv fetch` はそこから cache を作り直します。エントリは id 順に並ぶので diff に
出るのは実際の変更だけです。将来のバージョンが書いた lock は、黙って劣化させるのでは
なく拒否します。

## ライブラリとして使う

`skillenv` は Rust ライブラリでもあります。

```rust
use skillenv::{
    LinkReport, format_link_manifest_report, has_manifest, link_manifest, scan_skill_text,
};

if has_manifest(".") {
    let report: LinkReport = link_manifest(".")?;
    print!("{}", format_link_manifest_report(&report));
    for warning in report.warnings() {
        eprintln!("{warning}");
    }
    if report.has_problems() {
        // 終了コードに反映する
    }
}

// SKILL.md 単体を検査する
for finding in scan_skill_text(&text) {
    if finding.blocks_by_default() {
        // 既定の policy では展開されない
    }
}
```

公開している操作:

- `init_manifest`、`has_manifest`
- `link_manifest`、`unlink_manifest`、`status_manifest`、`format_link_manifest_report`、`format_status_manifest_report`
- `list_manifest`、`lint_manifest`、`remove_from_manifest`
- `fetch_manifest`、`outdated_manifest`
- `doctor_manifest`
- `plan_migration`、`apply_migration`、`prune_legacy_layout`、`sweep_legacy`、`remove_legacy`
- `scan_skill_text`（`Vec<Finding>` を返す）
- `skill_inventory`、`format_skill_inventory_report`
- `hook_script`

正確な API surface は [src/lib.rs](./src/lib.rs) を参照してください。crate 内部では、
`src/manifest.rs` がパースと id 検証、`src/lock.rs` が lock と content digest、
`src/catalog.rs` が平坦な名前空間、`src/provider/` が provider 別 frontmatter と
target 解決、`src/source/` が取得、`src/deploy.rs` が書き込みと marker、
`src/safeguard/` が検査を持ち、`src/session.rs` がそれらを組み立てます。

## バージョニング

`skillenv` は `Cargo.toml` の crate バージョンを、CLI のバージョンとリリース
バージョンの両方に使います。

- `skillenv version` はインストール済みのバージョンを表示します
- `skillenv --version` も同じ値を表示します
- GitHub Release のタグは `vX.Y.Z` です

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

GitHub Actions は pull request と `main` への push で CI を実行します。pull request
の merge も `main` への push になるため、release workflow が実行されます。

release workflow は `Cargo.toml` の `version` を読み取り、`vX.Y.Z` の GitHub Release
を作成または更新し、以下のクロスビルド成果物をアップロードします。

- `skillenv_vX.Y.Z_x86_64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_aarch64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_x86_64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_aarch64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_x86_64-pc-windows-msvc.tar.gz`
