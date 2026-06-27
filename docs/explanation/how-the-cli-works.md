# How sloppoke works at the CLI level

> An explanation of the load-bearing mental model behind the `slop`
> command-line: what it scans, what it returns, what the verdict
> tiers mean, why the TODO markers are an asset, how the mute and
> training flows feed back into the catalog. This page is not a
> tutorial and not a how-to — for step-by-step recipes see the
> [how-to guides](../how-to/) (including
> [Design a sloppoke demo](../how-to/design-a-sloppoke-demo.md)
> which uses everything below); for flag-level surface see
> [`reference/cli.md`](../reference/cli.md); for what the catalog
> actually pattern-matches see
> [`how-detection-works.md`](how-detection-works.md).

## The 60-second view

`slop` is a CLI that gates `git commit` against a server-hosted
catalog of LLM-introduced code-quality patterns. It is **a patch
issuer, not a rewriter** — when it flags something, the action it
proposes is splicing a `TODO(slop): <category> — <prose>` comment
above the offending line, never reformatting or rewriting the line
itself. The intent is to convert hidden debt into surface-visible,
greppable backlog so future attention is cheap.

Three commands carry the whole product:

| command | purpose |
|---|---|
| `slop poke`  | scan a diff, return a verdict + (if SLOP) a patch of TODO splices |
| `slop apply` | preflight + `git apply` the cached patch, optionally skipping specific findings |
| `slop learn` | train the server-side catalog with a feedback note + optional mute target(s) |

A pre-commit hook (the plugin variant under
`plugin-marketplace/sloppoke`) runs `slop poke --staged` before
every `git commit` and blocks the commit when the verdict is SLOP.
The same `slop` binary is what the hook calls, so everything
discussed below also describes what the hook does on the
operator's behalf.

## What `slop poke` actually does

The input is a **unified diff**. By default `slop poke` shells out
to `git diff HEAD` and captures the working-tree diff; flag
variants select other diff sources (`--staged`, `--range BASE..HEAD`,
`--patch FILE`, `--gh org/repo`, `--repo URL`).

The diff is filtered locally (the pre-send filter chain — see
below), then POSTed to the configured server's `/api/v1/poke`. The
server runs the catalog scan, returns:

- a **verdict** in one of three tiers (LGTM / MARKED / SLOP),
- a **patch** of `TODO(slop): …` splices when the verdict is SLOP,
- a **poke_id** that joins this scan to any subsequent `slop learn`
  feedback the operator submits,
- usage counters (`poke_calls`, monthly cap).

The CLI caches the patch + verdict in `.slop/last-poke.json` so
`slop apply` knows what to apply without re-scanning. Stdout
carries the patch (so `slop poke --staged | git apply` is a valid
one-liner); stderr carries the verdict, the per-finding summary,
and the apply hint.

The verdict line is the only stable contract: hooks and CI
configurations grep for `slop poke: LGTM` / `slop poke: MARKED` /
`slop poke: SLOP` and decide whether to allow the commit.

## The three verdict tiers

```
LGTM    — no catalog hits in this diff. Nothing to apply.
MARKED  — hits exist but every TODO splice is already present in
          source (operator applied earlier, didn't address yet).
          Nothing actionable — treat as pass.
SLOP    — hits exist + a non-empty patch of TODO splices is
          proposed. The pre-commit hook BLOCKS the commit on this
          tier; CI gates can do the same.
```

MARKED exists specifically so re-running `slop poke` on a repo
with already-applied markers does not block forever — it
acknowledges "yes, the markers are still there, you are aware".
The operator clears MARKED by converting markers into fixes (which
deletes them) or into followups (which leaves them but at least
the work is logged).

## The TODO marker doctrine

This is the part most demos get wrong: **the markers are an asset,
not noise.** When `slop apply` lands the cached patch, the working
tree gains lines like:

```rust
// TODO(slop): placeholder identifier — pick a name that says what this is
pub fn handle_payment_required(payment_response: &PaymentRequired) -> Result<bool> {
```

```typescript
// TODO(slop): silent_null_coalesce — original required arg was dropped without explanation
const taskRunner = (agent?, model?) => { /* ... */ };
```

These are **deliberately greppable debt receipts**. `git grep -n
"TODO(slop)"` enumerates every queued cleanup spot in the tree.
The marker carries `(file, line, category)` — all the triage
research already done by the catalog. The agent or reviewer reads
each one and decides:

- **In-scope + small** → fix it in the same change. The marker
  disappears on the next `slop poke`.
- **Out-of-scope** → file a followup, leave the marker, reference
  the new ticket next to it.
- **False positive** → `slop learn --disable '<path>:<8hex>'
  "<reason>"` so the server catalog stops flagging the pattern.

The system trends toward zero markers as the operator and agents
work through them. Marker count = remaining work.

Markers are not warnings to silence, not test failures to ignore,
not noise to clean up. Stripping a marker without addressing it
costs trust on both sides: the next `slop poke` will re-emit it,
which means quota burned and a round-trip wasted, and the operator
loses the historical breadcrumb they would have used to track the
issue.

## Patch-notation addresses (file, line, checksum)

Every finding the operator sees has three addresses:

```
crates/sloppoke-cli/src/payment.rs:41  [fc10f7dd]  placeholder identifier
└──────── file ──────────────────────┘ └──┬───┘   └────────┬───────────┘
                                          │                │
                                  content checksum     catalog category
```

The three addresses correspond to three granularities of mute:

1. **File-level** — `--disable 'src/sdk-alpha/**'` strips the
   whole file block from the diff before it reaches the server.
   Used for known-FP directories (SDK alpha shims, generated
   bindings, `.env.example` placeholder creds). Persists in
   `.slopignore` if the operator wants repo-level coverage.
2. **Line-content checksum** — `--disable 'src/foo.rs:fc10f7dd'`
   converts the matching `+line` to a context line ` line` in the
   diff before sending. The line is invisible to the server's
   catalog. Survives line-number drift (refactors moving the line
   up or down do not break the mute) but breaks intentionally when
   the line content changes — a reformat re-prompts the operator
   on whether the new shape is still a FP.
3. **Line number** — `--disable 'src/foo.rs:42'` when no checksum
   is available yet. Drifts on every refactor; useful as an
   escape hatch but not as a long-term mute.

The checksum is **8 hex chars** of `sha256(line_content)`. The
verdict prints it in `[…]` so the operator copy-pastes; the same
8-hex value addresses the line under `slop apply --skip`,
`slop learn --disable`, and any future tooling that needs a stable
per-line address.

## Pre-send filter chain

Before the diff is POSTed, the CLI runs it through:

```
git diff
  → strip file blocks matching .slopignore globs
  → strip file blocks matching --disable globs (whole-file mutes)
  → redact + lines matching --disable globs:<8hex> (checksum mutes)
  → POST to /api/v1/poke
```

Two consequences worth understanding:

- **Quota and cost.** The server's billing is per-call, and the
  payload is what counts. A repo that mutes its 20k-line generated
  binding directory at the `.slopignore` layer sends a smaller
  diff and pays less per scan.
- **Catalog visibility.** Anything redacted at this stage is
  invisible to the catalog. The operator is deliberately choosing
  "the catalog should not see this code"; if the redacted line
  was actually slop, the system cannot tell them.

Pre-send muting is the strongest privacy posture but also the
weakest detection posture — the tradeoff is explicit.

## `slop apply` and the `--skip` flow

`slop apply` reads `.slop/last-poke.json` and runs
`git apply --check` (preflight) then `git apply --unidiff-zero
--index -` (commit-staging apply). If the user opts out of the
implicit amend, `--no-commit` stops after the staging step.

The `--skip <8hex,8hex>` flag filters the cached patch BEFORE
`git apply` sees it. Each TODO splice in the patch is walked, the
b-side source line it annotates is read, hashed, and the hunk is
dropped if the checksum is on the skip list. The patch that
actually hits `git apply` is a valid unified diff with fewer hunks.

**Auto-learn-on-skip.** Every `--skip` invocation that actually
dropped a hunk **also ships one batched learn entry** to the
server. The training body says "operator used --skip to drop N
splice(s)" (or whatever `--skip-reason "<text>"` provided) and
the context block lists every skipped checksum as a
`--- disable_targets ---` row. The next time the catalog's RL
loop runs, those patterns get de-ranked for this org. The
operator runs `slop apply --skip <ids>` once; tomorrow's poke
returns fewer FPs on the same shape. The training signal is the
apply event itself — no second `slop learn` call needed.

This is the loop that gives sloppoke its "fewer FPs over time"
property: every FP the operator triages becomes a server-side
catalog adjustment, and every other operator in the org benefits
from the next scan onward.

## `slop learn` — the explicit training channel

`slop learn "<note>"` ships a free-form feedback row to
`/api/v1/learn`. It is used for:

- "this is FP because postgres.js tagged templates ARE prepared
  statements"
- "we always name our top-level handler `Manager` on purpose"
- "the scan missed an obvious leaked secret in a test fixture"

With `--disable <target,...>` flags, the feedback gets the
matching checksum/path targets attached so the RL loop joins the
prose to the exact addresses the operator wants muted.

The CLI also auto-attaches the most recent
`.slop/last-poke.json` (poke_id + truncated input diff + proposed
patch) as context so the server can correlate the feedback to the
scan that produced it. `--no-attach` suppresses that for generic
"we always do X" kind of notes that do not reference a specific
scan.

Quota: ~100 learn entries / month / org by default. The
auto-learn-on-skip path counts against the same cap — designed to
fit hundreds of skip events per month without burning everything.

## Privacy at the wire

- The CLI sends only the **filtered diff** — never the full file,
  never the surrounding code (unless it is in the diff already),
  never the commit message.
- No telemetry beyond what the API itself logs (poke_id, byte
  count, verdict, fingerprint of the SSH identity that authed).
- `.slopignore` + `--disable` redactions happen client-side. If
  the operator marks `src/secrets/` as muted, the server never
  sees those bytes.
- Auth is SSH-key handshake (`slop login` cached one-shot).

See [`privacy-and-identity.md`](privacy-and-identity.md) for the
fingerprint flow and the `~/.config/slop/` paths.

## What the server does (briefly)

The server is `peeramid-labs/sloppoke`'s `sloppoke-server` binary.
It hosts:

- the **catalog** of regex + tree-sitter rules per language (TS,
  Rust, Python, Go, C, JS, etc) tagged by category
  (`placeholder_identifier`, `any_cast`, `silent_null_coalesce`,
  `what_filler_comment`, `dev_process_framing`, …),
- the **learn loop** that ingests `slop learn` rows + auto-learn
  signal from `--skip`, runs an offline RL pass that proposes
  catalog adjustments, and applies them under operator review,
- billing (Stripe) for paid tiers and the free baseline cap,
- a **public scorer** at `sloppoke.me` that anyone can POST a
  github URL to and get a hits/100c rating for the last N commits.

The CLI does not embed an LLM, does not embed the catalog, and
does not run any inference. Everything substantive is server-side.

## What's NOT in the CLI

- **No offline mode.** Every `slop poke` needs the server. Caching
  is the response cache, not an offline scanner.
- **No source rewrites.** `slop apply` only ever inserts TODO
  markers and deletes SafeDelete-tier lines. It does not
  reformat, rename, or refactor.
- **No commit-hijacking.** The CLI never runs `git commit` itself
  (apart from `git commit --amend` after the operator-initiated
  `slop apply`). The pre-commit hook returns exit codes; git
  decides whether to proceed.
- **No model selection.** There is no "pick which LLM scans my
  code" knob. The catalog is the catalog.
- **No `--strict` flag** that ignores local mutes. Mutes are
  client-side filters; if a CI run wants to ignore them, it does
  not pass `.slopignore` and it does not pass `--disable`. Simple.

The absence of those features is intentional: the value
proposition is the catalog + the training loop + the patch-issuer
shape, not the local tooling surface.
