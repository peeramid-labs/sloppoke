






## [0.9.0] - 2026-06-23

### 🚀 Features

- *(plugin)* Migrate plugin source to peeramid-labs/plugin-marketplace
- *(cli)* UX fixes — learn panic, --json, slop bypass, .slopignore

### 🚜 Refactor

- *(cli)* Replace finding-id mute with patch-notation --disable
- *(cli)* Drop slop poke --json — file:line stderr already structured

### 📚 Documentation

- *(explanation)* Add how-sloppoke-compares + index entry
- *(readme)* List correlation-study explanation page
- *(explanation)* Mirror correlation-study with academic dataset channel

## [0.8.3] - 2026-06-13

### 🐛 Bug Fixes

- *(hooks)* MARKED tier passes the pre-commit gate

### 📚 Documentation

- Explanation/llms-are-lossy-compression — rationale page
- Hyperlink references in lossy-compression article
- Drop regex+AST mention — say deterministic only
- *(tutorials)* Add hooks install as steps 7+8 of first-poke
- *(plugin)* TODO(slop) markers are an action queue, not noise
- *(plugin)* Reframe slop as debt-marker injector, not a rewriter
- *(readme)* Add slop badge for sloppoke itself + score-public-repo guide
- *(readme)* Use .svg suffix on badge URL for camo content-type hint

### 🎨 Styling

- *(cli)* Cargo fmt — pre-existing drift CI now catches

## [0.8.2] - 2026-06-12

### 🐛 Bug Fixes

- *(cli)* Pre-commit hook drops stderr, only inspects stdout

### ⚙️ Miscellaneous Tasks

- *(plugin)* Bump to v0.2.8 to force marketplace reinstall pickup

## [0.8.1] - 2026-06-12

### 🐛 Bug Fixes

- *(plugin)* V0.2.7 — proper token-walk to detect git commit, not substring match

## [0.8.0] - 2026-06-12

### 🚀 Features

- *(plugin)* V0.2.4 — /slop:install-hook slash command
- *(plugin)* V0.2.5 — SessionStart tip nudges /slop:install-hook when git-level hook missing
- *(cli)* Slop learn auto-attaches input diff + proposed patch from last poke
- *(cli)* Slop status + defense-in-depth funnel via /slop:status
- *(cli)* Supply-chain verification — checksums, attestation, version provenance
- *(cli)* Slop learn body carries cli_version + build commit

### 🐛 Bug Fixes

- *(cli)* Accept -v / -V / --version, brand as 'slop' not 'sloppoke-cli'
- *(plugin)* V0.2.1 — hook actually fires on macOS now
- *(plugin)* V0.2.2 — trace log so 'is the hook actually firing' is answerable
- *(plugin)* V0.2.3 — chdir into 'cd X &&' prefix before slop poke

### 📚 Documentation

- Diataxis-structured usage docs in docs/
- Drop every 'regex' mention — never market the implementation
- Swap ASCII diagrams for mermaid
- *(privacy)* Add 'Does the CLI work without the server?' Q&A
- Drop diataxis meta-narrative line — let structure speak
- Rewrite Claude Code plugin how-to for v0.2.5 (2 hooks + 4 commands)

### 🧪 Testing

- *(cli)* Pair tests for cap_diff + load_plan fallback + attached_context

## [0.7.0] - 2026-06-11

### 🚀 Features

- *(cli)* Clamp HEAD~N to repo history before git diff — friendlier UX on small public repos
- *(cli)* Pleasant 402 onboarding — auto-open Stripe, inline pricing, post-purchase polling + replay
- *(claude-plugin)* Scaffold sloppoke Claude Code plugin
- *(claude-plugin)* Add marketplace.json so /plugin marketplace add resolves the repo
- *(cli)* Slop install-hook subcommand for pre-commit gate
- *(cli)* Slop news + post-command auto-ping for product announcements
- *(cli)* Offer global pre-commit hook install during slop login
- *(cli)* Slop learn auto-attaches last poke (id + patch) as context
- *(plugin)* V0.2 — PreToolUse hook auto-gates every git commit

### 🐛 Bug Fixes

- *(cli)* Count hunks from patch text, not the removed cleanup_actions vec
- *(cli)* Poke prints the patch and nothing else
- *(claude-plugin)* Rename marketplace to kebab-case 'peeramid-labs'
- *(docs)* Plugin marketplace add uses org/repo form not full URL

### 📚 Documentation

- *(readme,skill)* Cover --gh/--repo, naked-stdout output model, piping idioms
- *(readme)* Add CI workflow snippet for sloppoke in github actions
- *(readme)* Add 'how do we characterize slop' section — magic version
- *(readme)* Add Claude Code plugin install (marketplace add + install)
- *(readme)* Move Claude Code plugin install up under Install section

### 🧪 Testing

- *(cli)* Unit coverage for news cache roundtrip + install-hook script marker

### ◀️ Revert

- *(cli)* Restore verdict + apply hint on stderr — strip went too far

## [0.6.0] - 2026-06-08

### 🚀 Features

- *(cli)* Friendly version notice — checks GitHub Releases with 24h cache
- *(cli)* Poke prints the proposed patch on stdout alongside the line summary
- *(cli)* Infer clone depth from --range, cache repos, colorize patch, add --patch-only

### 🐛 Bug Fixes

- *(cli)* Clippy — char-array split sugar in version parser
- *(cli)* --patch-only routes through TTY-aware emitter so terminal output stays colored
- *(cli)* Drop per-line findings dump — it leaked detector keywords
- *(cli)* Mark PokeResponse.findings as #[serde(default)]
- *(cli)* Drop --patch-only flag — redundant with stdout/stderr split

### 📚 Documentation

- *(cli)* Surface git apply --unidiff-zero in the manual-apply hint
- *(skill)* Cover --gh/--repo, --patch-only, persistent cache, color, knobs
- *(changelog)* Bootstrap CHANGELOG.md with git-cliff full history

### 🎨 Styling

- Cargo fmt — collapse parse_version chain

## [unreleased]

### 🚀 Features

- *(cli)* Poke --repo / --gh — scan any git URL without manual clone
- *(cli)* Friendly version notice — checks GitHub Releases with 24h cache
- *(cli)* Poke prints the proposed patch on stdout alongside the line summary
- *(cli)* Infer clone depth from --range, cache repos, colorize patch, add --patch-only

### 🐛 Bug Fixes

- *(cli)* Clippy — char-array split sugar in version parser
- *(cli)* --patch-only routes through TTY-aware emitter so terminal output stays colored
- *(cli)* Drop per-line findings dump — it leaked detector keywords
- *(cli)* Mark PokeResponse.findings as #[serde(default)]
- *(cli)* Drop --patch-only flag — redundant with stdout/stderr split

### 📚 Documentation

- *(magic)* Replace detector enumeration with high-level capability copy
- *(rename)* README title + repository URL → peeramid-labs/sloppoke
- *(free-tier)* Surface 50 pokes/month free in README + skill
- *(pricing)* Drop monthly quota + dollar references from README + skill
- *(cli)* Surface git apply --unidiff-zero in the manual-apply hint
- *(skill)* Cover --gh/--repo, --patch-only, persistent cache, color, knobs

### 🎨 Styling

- Cargo fmt — collapse parse_version chain

### ⚙️ Miscellaneous Tasks

- *(release)* V0.4.1
- *(domain)* Switch DEFAULT_SERVER + README badge + ssh_resolve fixtures to sloppoke.me
- *(homebrew)* Rename homepage URL slop-cli → sloppoke
- *(release)* V0.5.0
## [0.4.0] - 2026-06-08

### 🚀 Features

- *(cli)* Apply now handles insert_above CleanupAction
- *(cli)* Prefer server-rendered patch — git apply replaces in-house line editor

### 📚 Documentation

- *(readme)* List supported languages + add branch-without-test row
- *(skill)* Refresh detector list, document tier-aware apply + ssh-G login

### ⚙️ Miscellaneous Tasks

- *(release)* V0.4.0
## [0.3.0] - 2026-06-08

### 🚀 Features

- *(cli)* Ssh-config-aware key resolution via ssh -G

### 📚 Documentation

- *(readme)* Drop brew/apt install paths until release pipeline stabilises
- *(readme)* Add Homebrew install for slop v0.2.3

### ⚙️ Miscellaneous Tasks

- *(release)* V0.3.0
## [0.2.3] - 2026-06-07

### 🐛 Bug Fixes

- *(release-workflows)* Pin artifact actions to v3 for forgejo GHES compat
- *(cli)* Derive pubkey from --key, recompute fingerprint per request

### 🎨 Styling

- Cargo fmt

### ⚙️ Miscellaneous Tasks

- *(release)* V0.2.3
## [0.2.2] - 2026-06-07

### 🚜 Refactor

- *(cli)* Rename forgejo_api → api, drop dead forgejo_user + orgs fields

### ⚙️ Miscellaneous Tasks

- *(release)* V0.2.2
## [0.2.1] - 2026-06-07

### 🐛 Bug Fixes

- *(release-workflows)* Replace cross with cargo-zigbuild for linux targets

### ⚙️ Miscellaneous Tasks

- *(release)* V0.2.1
## [0.2.0] - 2026-06-07

### 🚀 Features

- *(cli)* Default poke to git diff, add --staged/--range/--since/--patch

### 🐛 Bug Fixes

- *(release-workflows)* Read version from [workspace.package], not crate Cargo.toml
- *(release-workflows)* Use forgejo REST API instead of gh CLI
- *(release-workflows)* Recover from half-failed runs by reusing branch
- *(release-workflows)* Fail loud when cargo set-version no-ops
- *(api)* Base64-encode armored SSH signature for Authorization header
- *(release-workflows)* Stage explicit paths + emit pre/post status
- *(release-workflows)* Drop Cargo.lock from staging — repo gitignores it

### 📚 Documentation

- Restore centered mascot block (HTML <p align=center> with width=500)
- ML-driven learning copy + drop implementation details
- Drop CI badge — actions disabled on github mirror, forgejo runs CI

### ⚙️ Miscellaneous Tasks

- Copy nsed release workflows 1:1 (verbatim)
- Adapt nsed workflows for slop-cli single-binary layout
- Run integration tests too — drop --bins flag, slop-cli has tests/
- *(release)* V0.2.0
