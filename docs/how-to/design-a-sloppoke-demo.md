# How to design a sloppoke demo

> Recipe for someone (human or agent) preparing a demo / video /
> onboarding deck around the `slop` CLI. Assumes the reader has
> already read
> [How sloppoke works at the CLI level](../explanation/how-the-cli-works.md)
> — verdict tiers, the TODO marker doctrine, mute/learn loop. This
> page is the mapping from that conceptual model to a concrete
> sequence the audience can follow.

## Pick the demo shape

Three shapes work; pick by audience:

| shape | audience | length | core scene |
|---|---|---|---|
| **Hook → guarded commit → learn** | dev / IC | 90s | watch a `git commit` block, then watch it pass |
| **Onboarding loop** | platform / DX | 5 min | install hook → first scan → triage cycle → score climbs |
| **Catalog learns overnight** | prospect / decision-maker | 3 min | run `--skip` today, run `slop poke` tomorrow, FP is gone |

Default to the third unless the audience asked for something
specific. It is the only one of the three that demos the loop
that makes sloppoke different from a pre-commit linter.

## The end-to-end scene the demo is built around

Run this against any real repo with a recent diff. The verdict
shape is what the audience needs to see:

```
$ slop poke --staged
slop poke: SLOP — 3 hits (47 ms, 17/100000 this cycle)
  src/db/queries.ts:99           [a1b2c3d4]  what_filler_comment
  src/sdk-alpha/types.ts:42      [e5f67890]  any_cast
  src/lib.rs:200                 [11223344]  placeholder identifier

diff --git a/src/db/queries.ts b/src/db/queries.ts
@@ -98,0 +99,1 @@
+// TODO(slop): what_filler_comment — restating the next line in prose

[… patch continues …]

Run `slop apply` to apply, `slop apply --discard` to drop, or
`git apply --unidiff-zero` if applying manually. To mute a specific
finding inline use `--disable <path[:line]>`; to train the server
use `slop learn --disable <path[:line]> "<reason>"`.
```

Two findings are real, one is a FP (the SDK alpha types are
intentionally `any`-cast). The operator triages by applying the
patch with `--skip` on the FP, which **also auto-ships a training
signal** so the next-day catalog drops the FP:

```
$ slop apply --skip e5f67890 \
             --skip-reason "Zama SDK 3.1.0-alpha.15 types churn weekly; any-cast is the operator-intended workaround"
slop: --skip dropped 1 hunk(s) from the cached patch before git apply
slop: applied server patch (verdict: SLOP — 3 hits)
slop: staged. Commit when ready.
slop: shipped 1 skip signal(s) to the learn loop (17/100 this cycle, 412 bytes) — tomorrow's catalog will de-rank these patterns for your org
```

Triage the remaining markers:

```
$ git grep -n "TODO(slop)"
src/db/queries.ts:99: // TODO(slop): what_filler_comment — …
src/lib.rs:200:       // TODO(slop): placeholder identifier — …
```

`what_filler_comment` rewrites in scope. `placeholder identifier`
becomes a followup ticket. Both markers eventually disappear.

Re-scan shows the improved state. The SDK alpha FP does not come
back because the server-side catalog learned from the `--skip`
event:

```
$ slop poke --staged
slop poke: LGTM (12 ms, 18/100000 this cycle)
```

## What to lead with

In order, the points the audience needs to absorb:

1. **The verdict tiers.** LGTM is silent good. SLOP blocks the
   commit and gives a backlog. MARKED is "we already told you,
   get back to work". Show the three-tier shape early — it is
   what makes the gate humane.
2. **The patch is the product.** The TODO markers are not noise;
   they are the deliverable. `git grep -n "TODO(slop)"` IS the
   action queue. Do not apologise for marker count.
3. **A FP being muted, catalog updating overnight.**
   `slop apply --skip <8hex>` → next day's poke is cleaner. This
   is the WOW moment: the system learns from operator behavior
   without explicit training data engineering on their side.
4. **The privacy story when you mute a file.** `.slopignore` gets
   `src/secrets/**` added → the server never sees those bytes.
   This is what makes the SaaS posture acceptable for enterprise.
5. **Marker count over time goes down.** That is the thesis:
   hidden debt becomes visible work becomes done work. Trending
   the count down is the value.

## What to avoid

- **Do not demo a rewriter.** Slop is not a rewriter. Showing
  someone expecting `clippy --fix` an output that just adds TODO
  comments will land flat unless the rewriter framing is killed
  in the first 30 seconds.
- **Do not show a model-selection UI.** There is none. If the
  question comes up: "the catalog is the catalog; the value is
  the patterns, not the model that scans for them".
- **Do not silence findings without a reason.** Every `--skip`
  the demo shows should have a `--skip-reason "<text>"` so the
  audience sees the training signal getting strengthened, not just
  a mute happening.
- **Do not start with a repo that scores well.** A clean repo
  hides the product. Pick a repo with real findings — or seed a
  prepared branch.
- **Do not let "score 41/100 SLOPPY" sit on screen without a
  triage path.** Always pair "here is what flagged" with "here
  is how the operator addresses it". The score alone reads as
  criticism; paired with the workflow it reads as visibility.

## Repos suitable for the demo

- **`peeramid-labs/sloppoke`** itself — runs sloppoke against its
  own code. Run
  `slop poke --gh peeramid-labs/sloppoke --range HEAD~30..HEAD`
  and the verdict will have real findings to triage on screen.
- **Any pre-prepared branch on the prospect's own repo.** Most
  trust-building. Pre-scan offline to make sure the findings hit
  the categories you planned to demo; if the categories are wrong
  for the audience (e.g. a Rust shop scoring well on JS-only
  rules), pick the next repo.
- **A small public LLM-adjacent repo** — e.g. one of the cases
  from `sloppoke-bench/corpus/`. Cited findings in the
  [correlation study](../explanation/correlation-study.md) are
  good fallback material.

## Producing the artefacts

For each demo recording:

- Run the scenario once dry, no recording, to make sure the
  verdict + counts land where the demo expects. Cache state in
  `.slop/last-poke.json` will affect re-runs, so `slop apply
  --discard` between takes.
- Capture stderr alongside stdout (both channels carry the story;
  the per-finding summary is on stderr).
- Have a paired browser tab on `sloppoke.me/s/<share_id>` of the
  same repo so the audience can see the public-scorer card
  matching the local scan.

## What to write up afterwards

If the demo is for a prospect:

- Their org's likely top-3 FP categories (from the pre-scan).
- The `slop learn --disable` lines they would need to ship to
  train the catalog away from those FPs.
- An estimate of monthly quota use given their commit cadence.

If the demo is for internal training:

- The triage decisions you made on screen (fix vs followup vs FP)
  and the reasoning, so the audience has a model to mirror on
  their own.
- A pointer back to
  [How sloppoke works at the CLI level](../explanation/how-the-cli-works.md)
  for the conceptual material the demo skipped over.
