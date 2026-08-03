# skillenv

[日本語版 README](./README_ja.md)

`skillenv` acquires, versions, and deploys agent skills from a single
`skillenv.toml`. Skills come from your own `skills/` directory, GitHub
repositories, gists, or local paths, and are deployed into the directories each
agent reads — `.claude/skills`, `.agents/skills`, `$CODEX_HOME/skills`,
`.opencode/skills` — with frontmatter rewritten per provider and every skill
scanned before it is written.

It provides:

- one hand-written manifest declaring what exists, where it comes from, and where it goes
- a lock file recording what each source resolved to, so another machine reproduces it
- per-provider frontmatter, because the official validators do not agree on which keys are legal
- supply-chain checks on every skill, using Snyk's `agent-scan` codes
- shell hooks that relink when you change repositories
- a reusable Rust library exposing the same operations

The current version is `1.0.0`.

## 1.0 is a breaking release

The v0 layout — `skillenv/{default,local,profiles}/`, scopes expressed as
directories, `skillenv.lock.json`, and the `add`/`update`/`global` commands built
on it — no longer runs. Every command that acts on skills reads `skillenv.toml`
now, and without one it stops with `no skillenv.toml found from <dir> upwards;
create one or set SKILLENV_MANIFEST`.

**A repository that has not migrated must run `skillenv migrate --apply`.**
`migrate` is the only thing left that understands the old layout; see
[Migrating from v0](#migrating-from-v0).

## Install

Install the latest GitHub Release on Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh
```

Install to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -b=$HOME/.local/bin
```

Install a specific release:

```bash
curl -fsSL https://raw.githubusercontent.com/igtm/skillenv/main/install.sh | sh -s -- -v=v1.0.0
```

Install from GitHub with Cargo:

```bash
cargo install --git https://github.com/igtm/skillenv.git --locked
```

Install from a local checkout:

```bash
cargo install --path . --locked
```

## Getting started

1. Run `skillenv init` once, wherever you want the manifest to live — typically a
   dotfiles repository.
2. Declare your skills, their sources, and where they go, in `skillenv.toml`.
3. Run `skillenv fetch` to populate the cache, then `skillenv link` to deploy.
4. Use `skillenv outdated` to see what has moved upstream, and `skillenv lint`
   before trusting new material.

```text
skillenv.toml            the one hand-written file
skillenv.lock            what each source resolved to; commit this
skills/<name>/SKILL.md   skills you write yourself
.skillenv/cache/         fetched sources; not committed
```

Commands that read the manifest find it by walking up from the working directory,
so any subdirectory of the manifest's repository works. `SKILLENV_MANIFEST`
overrides that with an explicit file path.

`skillenv init` also adds these lines to `.gitignore`, so the cache and any
generated directories stay out of `git status`:

```text
.skillenv/
.agents/skills/skillenv-*
.claude/skills/skillenv-*
.opencode/skills/skillenv-*
```

It never overwrites an existing `skillenv.toml`: that file is the only
hand-written input, so replacing it with a template would discard the whole
configuration. It does not deploy anything either — run `skillenv link` once you
have declared a skill.

## Commands

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

`skillenv <command> --help` prints the same reasoning in more detail.

### link and unlink

`link` deploys every skill each `[[deploy]]` rule selects, into the directory
that rule names. Rules resolving to the same directory have their selections
unioned, so two rules cannot take turns removing each other's work. A rule with
`when.repo` applies only inside that repository, which is what makes running
this from a directory-change hook useful.

Failure is per skill: a malformed `SKILL.md`, a name collision, or a skill held
back by the safeguard is reported and skipped, and the rest still deploy. Only a
systemic I/O failure stops the run.

Warnings go to stderr and the exit code is non-zero on a problem **even under
`--quiet`**, which is the form the shell hook runs. A skill that failed to deploy
must not be invisible there.

`unlink` removes only directories whose marker names this manifest. A directory
carrying the `skillenv-` prefix without a marker, or with another manifest's, is
reported and left in place.

### status

Reports every `skillenv-` directory in each target this manifest deploys to,
including directories belonging to a different manifest and directories carrying
the prefix but no marker. Those are never removed — without a marker there is no
evidence skillenv created them — and hiding them would make the count disagree
with `ls`.

A skill a rule selects but that is not on disk is listed by name. The usual cause
is a cache that was never fetched.

### fetch

Populates `.skillenv/cache/` for every remote source the manifest declares.

Without `--update`, restores exactly the revisions `skillenv.lock` records. That
is what a fresh clone needs: the cache is not committed, so a new machine has the
manifest and the lock and nothing else. With `--update`, moves to whatever each
ref points at now and rewrites the lock; run `skillenv outdated` first to see
what would move.

The lock is saved after each source rather than once at the end, so an
unreachable source part-way through cannot leave the installed trees and the
recorded revisions disagreeing.

A fetched tree is capped at 500 files, 2 MiB per file, and 16 MiB in total, so a
hostile or accidentally-huge source cannot fill the disk or stall a shell hook.
`.git` and `.DS_Store` are never copied.

### outdated

Reads only: contacts each remote with `git ls-remote` and touches neither the
cache nor the lock. Being out of date is a state, not a failure, so this exits 0
either way. A CI job that wants to fail on staleness can match the output.

### diff

`outdated` says a source moved; this says what moved in it. Reports the locked
revision against what the remote points at now, whether each deployment came from
the bytes the cache currently holds, and a unified diff of `SKILL.md` where it did
not. Only the remote revision needs a network; the content comparison works
offline, and that is the half you can act on.

Bodies only. Frontmatter is rewritten per provider and the `name` is the generated
directory, so including it would put a difference in every diff that is not a
change to anything.

With no cache, or a marker that recorded no digest, it says it cannot compare
rather than claiming a match — absence is not agreement. A directory carrying the
prefix without this manifest's marker is not reported as this skill's deployment,
matching what `status` and `link` already refuse to claim.

### lint

Scans every declared skill and exits non-zero when anything is found. `link` runs
the same checks and blocks on critical findings; `lint` is how to see them before
deploying. It also reports unparseable frontmatter, which is the most common
reason a skill fails to deploy, and a missing `SKILL.md` as `W014 [low]`.

### remove

Edits `skillenv.toml` in place, keeping every comment and the order of what
remains, then relinks so the removed entry's directories go with it. Naming a
`[[source]]` removes every skill it contributed.

The manifest is edited before the relink, so the relink sees the entry gone and
clears its directories; the other order would deploy it again on the way out.

### skills

Answers "which custom skills does this tool actually see from here?", managed or
not. Use `skillenv status` for what this manifest put there.

Discovery targets:

- `codex`: current repo `.agents/skills`, `$HOME/.agents/skills`, `/etc/codex/skills`
- `claude`: current repo `.claude/skills`, `$HOME/.claude/skills`
- `opencode`: current repo `.opencode/skills`, `.claude/skills`, `.agents/skills`, plus the `$HOME` global paths
- `antigravity`: repo-root `.agents/skills`, legacy `.agent/skills`, `$HOME/.gemini/antigravity/skills`

The default mode reports what is visible from the working directory.
`--repo-tree` adds repo-wide inventory for nested tool directories that are not
currently visible. `--json` prints a stable machine-readable report.

### doctor

Answers "why did it go there", where `status` answers "what is deployed". It
reports which `skillenv.toml` governs this directory and the repository it
resolved, the home directory and the cache path with how many sources are cached,
how many skills and deploy rules the manifest declares against how many the lock
records, and each resolved target with its provider and how many deployments it
holds. `--json` prints the same information in a stable shape.

## skillenv.toml

```toml
[skillenv]
version = 1

# Your own skills: read from skills/<name>/SKILL.md
[[skill]]
name = "japanese-tech-writing"
source = "local"
labels = ["writing"]

# A gist carries no frontmatter, so the description has to be supplied here.
[[skill]]
name = "jp-writing-upstream"
source = "gist:fd287c3133457c4fd8f5601d34aa817d"
description = "Prose conventions for Japanese technical writing"
labels = ["writing"]

# One source, several skills.
[[source]]
name = "igtm-skills"
from = "github:igtm/skills"
ref = "main"
skills = ["user-context"]   # or "*" to follow every skill it offers
labels = ["tools"]

# Where the skills go.
[[deploy]]
target = "claude:home"           # ~/.claude/skills
include = ["*"]

[[deploy]]
target = "claude:repo"           # .claude/skills of the repository you are in
include = ["writing"]
exclude = ["jp-writing-upstream"]
when.repo = "~/tmp/kaijin-web"   # only inside this repository

[safeguard]
on_critical = "block"            # the default
on_high = "warn"
allow = ["W012:figma-to-code:sha256:abc123…"]
```

`version` must be `1`; omitting the `[skillenv]` table means the same thing. An
unknown key anywhere in the file is an error rather than being ignored.

### Sources

| Form | Meaning |
|---|---|
| `local` | `skills/<name>/` beside the manifest |
| `gist:<id>` | a gist, cloned as a git repository like any other |
| `github:owner/repo` | GitHub; a trailing `.git` is tolerated |
| `path:../shared` | a path on this machine |
| `git@…`, `ssh://…`, `https://…`, anything ending in `.git` | passed through verbatim |

Inside a fetched tree, a skill is looked for at the root itself, at `<id>/`, at
`skills/<id>/`, and at `.agents/skills/<id>/`, which covers the layouts that
occur in practice.

`description` overrides the skill's own frontmatter, and is what you supply when
the source carries none — a gist, typically. Every provider demands a
description, so when neither the manifest nor the frontmatter has one, `link`
synthesizes `Instructions for the <id> skill.` rather than writing a file that
will not validate. That sentence is what an agent reads when deciding whether to
load the skill, so declare a real one.

### Targets and providers

A target is `<provider>:<scope>`. The scope is `home` (under `$HOME`, shared by
every repository on the machine) or `repo` (the repository being linked).

| Provider | Directory |
|---|---|
| `claude` | `.claude/skills` |
| `agents` | `.agents/skills` |
| `codex` | `$CODEX_HOME/skills`, defaulting to `~/.codex/skills` |
| `opencode` | `.opencode/skills` |

**`.agents/skills` is not the Codex target.** It is the Agent Skills open
standard, read by many tools, which is why it is its own provider; Codex itself
reads `$CODEX_HOME/skills`. A `codex:repo` target resolves to `.codex/skills`
inside the repository.

opencode also reads `.claude/skills` and `.agents/skills`, so deploying to it
directly is only needed when a skill should be visible to opencode and not to the
tools sharing those directories.

Only `name` and `description` are common to every tool, so the frontmatter is
rewritten per provider. Claude, `agents`, and opencode accept `license`,
`allowed-tools`, `metadata`, and `compatibility`; Codex's validator rejects
`compatibility`, so it is dropped and reported rather than silently discarded.
`allowed-tools` is normalized on the way in — a space-separated string, a
comma-separated string, an inline sequence, and a block sequence all occur in
installed skills — and re-emitted as a space-separated string for Claude,
`agents`, and Codex, and as a sequence for opencode.

Codex additionally gets an `agents/openai.yaml` sidecar carrying the presentation
keys from `metadata` — `short-description`, `display-name`, `brand-color` — under
`interface`. Codex's own guidance reserves the frontmatter for `name` and
`description` and describes the sidecar as product-specific configuration for the
harness rather than the agent, so those keys are moved there instead of being
dropped. A skill with nothing to put in it gets no sidecar.

When several providers resolve to one directory, the one with the smallest
allowed-key set renders it, so the output satisfies all of them.

### Deploy rules

`include` is required: a rule with none would deploy nothing, which reads as a
broken rule rather than an intentional one. Use `include = ["*"]` to mean every
skill. `exclude` overrides `include`. A selector other than `*` matches a skill's
label or its id, so a single skill can be named without inventing a label for it.

`when.repo` restricts a rule to one repository. `~` is expanded, and a trailing
`/**` matches the subtree, so `when.repo = "~/work/**"` covers everything under
`~/work`. Absent means every repository, which is what a `home` target normally
wants.

### Skill ids

- `[a-z0-9-]` only, at most 32 characters, no leading, trailing, or consecutive hyphen.
- **Non-ASCII is an error, not transliterated.** v0 ran names through a slugifier
  that deleted non-ASCII characters and substituted a fallback word, so two
  differently-named Japanese skills silently became the same id. Give an explicit
  ASCII id in `skillenv.toml` instead.
- Unique regardless of case. macOS is case-insensitive by default, so `Foo` and
  `foo` would pass a naive uniqueness check and then collide on disk.
- No word is reserved. The selector wildcard is `*`, a target scope only ever
  appears after a `:`, and `local` only appears in a source position, so none of
  them is ambiguous — and an earlier version that reserved them broke a real
  skill named `skillenv`.

The 32-character cap exists because providers reject a frontmatter `name` over 64
characters and a deployed skill is named `skillenv-<repo>-g<hash>-<id>`. The cap
is a static budget for early feedback and cannot be exact, since the prefix grows
with the repository name; `link` measures the real generated name and skips a
skill that would still overflow.

### `skills = "*"` versus a list

`"*"` follows whatever the source offers. Where the members come from depends on
the source: `fetch` discovers a remote's and records them in `skillenv.lock`, so a
fresh clone has none until you run it; a `path:` tree is read directly, since
`fetch` has nothing to download for it.

A member that disappears upstream leaves the lock, because a wildcard's membership
*is* the tree — keeping it would mean a skill that is re-admitted on every run,
can never be deployed, and cannot be removed by name, since a wildcard member
appears in no manifest entry. Worse, being undeployable is what makes `link` clear
a deployment, so the one that was working would go.

An explicit list is the opposite case: you named it, so a name that disappears is
reported per skill and left for you to decide about.

A wildcard member whose id is already taken is reported and skipped, not fatal.
Two upstreams adopting one name is not your mistake, and refusing to load the
manifest would take `remove` — the way out — with it.

## The safeguard

A skill is executable instruction material fetched from someone else's
repository, so this is a supply chain. Codes follow Snyk's `agent-scan` taxonomy
rather than a scheme invented here, so output can be compared against an existing
scanner. The frontmatter is scanned along with the body, because `description` is
loaded eagerly into agent context while the body is not, which makes it the
highest-leverage place to hide an instruction.

| Code | Finding | Default severity |
|---|---|---|
| E004 | an instruction to override the instructions already in context | critical |
| E005 | a download piped straight into a shell | high |
| E006 | an instruction to read a secret and hand it somewhere | critical |
| W007 | an instruction to read a secret, with no destination | high |
| W008 | a credential literal committed into the skill | high |
| W012 | instructions fetched from an external URL at run time | high |
| W021 | invisible Unicode | medium, critical when it looks constructed |

`[safeguard]` maps each severity to `block`, `warn`, or `allow`. The defaults are
`on_critical = "block"` and `warn` for the rest; a policy left unset keeps its
default rather than becoming permissive. `block` refuses to deploy the skill and
names the finding on stderr; `warn` deploys it and records the finding in
`skillenv.lock`. `lint` reports every finding it sees, regardless of policy and
regardless of `allow`, which is why it can flag something `link` deploys without
comment.

Detection is on instruction *shape*, not vocabulary, because the requirement is
not detection — it is not firing on the legitimate skills already in use. A
secrets-manager skill mentions `.env` constantly, a Figma skill fetches from
`127.0.0.1` and documents a `curl … | sh` line, a PR skill runs `gh pr create`.
If any of those were blocked the feature would be turned off, which is worse than
not having it. So naming a sensitive path is documentation; telling the agent to
read one and put it in its reply is a finding. Findings inside a fenced code
block drop in severity, and loopback hosts are not treated as an external
instruction source.

W021 likewise judges structure, not characters: runs of Unicode Tags
(`U+E0000`–`U+E007F`), zero-width steganography, and unterminated bidi overrides
are weighed by run length, how many kinds are mixed, and whether they decode, and
a payload that decodes has its hidden text printed in the finding. Emoji joiners,
`U+3000`, and `U+00A0` do not fire.

A blocked skill is **not deployed, and an existing deployment is not removed
either**. Otherwise whoever took over an upstream could delete a skill by
deliberately tripping the scanner.

An `allow` entry is `<code>:<skill>:<digest>` and the digest is mandatory, so a
suppression stops applying the moment the content it was reviewed against
changes.

## What gets written

A deployed skill becomes `skillenv-<repo>-<id>` in a repo target, or
`skillenv-<repo>-g<hash>-<id>` under `$HOME`. `<repo>` is the manifest
directory's name, slugified; `<hash>` is the first twelve hex digits of a sha256
over its canonical path, and it exists because `$HOME` is shared by every
repository on the machine while removal keys on a name prefix — without it one
repository's `link` would delete another's entries.

**A symlink inside a skill is refused.** The walk does not follow one, but `fs::copy`
opens the path normally and so copies what it points at — a link named `notes.md`
aimed at an SSH key would be deployed as that key's contents, into a directory an
agent reads. A `local` or `path:` skill never passes the fetch-time checks, so this
is its only gate. Refused per skill, so the others still deploy.

Each directory holds a `.skillenv-generated.json` marker recording which manifest
owns it, the skill, the provider, the revision, and content digests.
**The marker is the only evidence skillenv created a directory**, so a directory
without one is never removed, only reported. Anything you placed by hand is safe.

`link` writes the marker *first*, before rendering `SKILL.md` and copying assets.
v0 wrote it last, so a skill whose frontmatter failed to parse left a directory
with assets and no marker, which every later run then refused to touch — one typo
froze an entire setup for six weeks.

`SKILL.md` is rendered rather than copied, so the frontmatter can carry the
generated name and whatever the provider accepts. Other files in the skill
directory are copied as they are.

## Migrating from v0

The first step writes nothing:

```bash
skillenv migrate           # show the plan; read-only
skillenv migrate --apply   # carry it out, keeping the old skillenv/
skillenv migrate --prune   # once the result is confirmed, remove it
```

`migrate` reports the skills, sources, and deploy rules a v1 manifest would
carry, the v0 deployments that must be cleared first, and the proposed
`skillenv.toml` in full. The deploy rules are *inferred from what is actually
deployed on disk*, since v0's own markers are the only record of it — and the
conversion destroys them, so there is no second chance to read them.

`--apply` clears v0's deployments while their markers still refer to a layout
that exists, then seeds the new cache from v0's vendored copies, so `link` works
offline immediately afterwards. It leaves `skillenv/` in place: check the result,
then `--prune`. To undo before that, delete `skillenv.toml` and `skillenv.lock`.

Migration refuses to guess and stops with the reason named:

| Condition | Why |
|---|---|
| `profiles/` is in use | mapping a profile onto a label would be guessing; declare them by hand |
| the same id under both `default/` and `local/` | a flat namespace cannot hold both |
| `skillenv.toml` already exists | already migrated |
| no `skillenv/` directory, or nothing in it | nothing to migrate |

Sources migrate as explicit lists, never as `"*"`. v0 recorded "follow
everything" and a hand-written enumeration in the same field, so the two are
indistinguishable, and choosing `"*"` would pull in every skill the source
currently offers, unreviewed. Change the ones you want to follow by hand.

If `skillenv/remote` was committed, `migrate` tells you to run
`git rm -r --cached skillenv/remote`: a `.gitignore` entry alone does not untrack
files.

## Shell hooks

`skillenv` can relink when you change directories.

For `zsh`, add this to `~/.zshrc`:

```bash
eval "$(skillenv hook zsh)"
```

For `bash`, add this to `~/.bashrc` or `~/.bash_profile`:

```bash
eval "$(skillenv hook bash)"
```

The zsh hook uses `add-zsh-hook chpwd`; the bash hook uses `PROMPT_COMMAND` and
only acts when the repository root changes. Both run `skillenv link --quiet` and
nothing else — they do not edit `.gitignore`. If you installed into a custom
directory such as `$HOME/.local/bin`, make sure it is in `PATH` before adding the
hook.

## The lock file

`skillenv.lock` is JSON at the manifest root, and it records intent's *result*
where `skillenv.toml` records the intent. Per skill it holds the source as
written, the `[[source]]` that contributed it, the resolved ref and revision, a
digest of the tree as fetched, and the safeguard's findings together with the
digest they were produced from — so a content change is noticed even when the
revision has not moved, and stale findings are rescanned rather than trusted.

Commit it. A fresh clone has the manifest and the lock and nothing else, and
`skillenv fetch` rebuilds the cache from them. Entries are sorted by id so a diff
shows only real changes, and a lock written by a future version is refused rather
than silently downgraded.

## Library usage

`skillenv` is also a Rust library.

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
        // reflect this in the exit code
    }
}

// Scan one SKILL.md on its own.
for finding in scan_skill_text(&text) {
    if finding.blocks_by_default() {
        // the default policy refuses to deploy this
    }
}
```

The exported operations:

- `init_manifest`, `has_manifest`
- `link_manifest`, `unlink_manifest`, `status_manifest`, `format_link_manifest_report`, `format_status_manifest_report`
- `list_manifest`, `lint_manifest`, `remove_from_manifest`
- `fetch_manifest`, `outdated_manifest`
- `doctor_manifest`
- `plan_migration`, `apply_migration`, `prune_legacy_layout`, `sweep_legacy`, `remove_legacy`
- `scan_skill_text`, returning `Vec<Finding>`
- `skill_inventory`, `format_skill_inventory_report`
- `hook_script`

See [src/lib.rs](./src/lib.rs) for the canonical API surface. Within the crate,
`src/manifest.rs` owns parsing and id validation, `src/lock.rs` the lock and
content digests, `src/catalog.rs` the flat namespace, `src/provider/` per-provider
frontmatter and target resolution, `src/source/` fetching, `src/deploy.rs`
writing and markers, `src/safeguard/` the checks, and `src/session.rs` assembles
them.

## Versioning

`skillenv` uses the crate version from `Cargo.toml` as both the CLI version and
the release version.

- `skillenv version` prints the installed version
- `skillenv --version` prints the same value
- GitHub Releases are tagged `vX.Y.Z`

## Development

```bash
cargo build
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo package --locked
sh -n install.sh
```

## Release automation

GitHub Actions runs CI on pull requests and pushes to `main`. A push to `main`,
including a merged pull request, also runs the release workflow.

The release workflow reads `version` from `Cargo.toml`, creates or refreshes the
`vX.Y.Z` GitHub Release, and uploads cross-built assets:

- `skillenv_vX.Y.Z_x86_64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_aarch64-unknown-linux-gnu.tar.gz`
- `skillenv_vX.Y.Z_x86_64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_aarch64-apple-darwin.tar.gz`
- `skillenv_vX.Y.Z_x86_64-pc-windows-msvc.tar.gz`
