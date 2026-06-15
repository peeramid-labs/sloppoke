# How sloppoke compares — CodeRabbit, OSS slop detectors, linters

Three categories of tool overlap with sloppoke at the commit boundary.
They are not the same product, and the differences are deliberate.

| | sloppoke | CodeRabbit (paid) | OSS slop detectors | Linters (clippy / eslint / ruff) |
|---|---|---|---|---|
| **Where the verdict lives** | Local commit boundary, sub-10 ms. | GitHub PR comments, 15 min+ after push. | Local CLI, varies. | Local CLI, milliseconds. |
| **When in the workflow** | Pre-commit, before the diff leaves the laptop. | Post-push, after the diff is in GitHub. | Pre-commit if you wire it. | Pre-commit if you wire it. |
| **What it adds to the codebase** | **Removes** lines (SafeDelete) or splices precise `TODO(slop):` markers (semantic). Code itself never rewritten. | Adds LLM-generated review prose to the PR. Reviewers + agents frequently paste it back into commits. | Flags only; no edit. | Many ship `--fix` that rewrites bodies. Useful for style, can mask logic. |
| **Detection engine** | Deterministic catalog match. Same diff → same finding, every release. | LLM call per review. Verdict can differ between runs on the same diff. | Static regex/AST. | Static rules. |
| **How it improves over time** | Multi-model deliberation on the slow path (Peeramid Labs NSED orchestrator) consumes your `slop learn` feedback and tunes the catalog continuously. Per-account + per-repo calibration. | Whatever the upstream model release does. | You upgrade when maintainers cut a release. | You upgrade when maintainers cut a release. |
| **Source seen by vendor** | Diff only. Catalog match runs server-side; raw lines retained only if you opt in to `slop learn`. | Full repo + PR context. | None. | None. |
| **Cost model** | Flat subscription. | Per-seat + LLM passthrough. | Free. | Free. |

## Two framings worth being direct about

### CodeRabbit's review surface is GitHub, on purpose

The vendor logo and the review prose sit in front of every reviewer's
eyes each time a PR opens. That is a deliberate distribution choice,
not a technical limitation. The consequence:

1. The loop closes **after** the diff is already in version control,
   not before.
2. The artefact the review leaves behind is **more LLM text in the
   PR**, not less. Reviewers and coding agents frequently paste the
   review prose back into the commit history.

Sloppoke chooses the opposite boundary — the local commit — so the
residue gets stripped before it lands. The artefact you keep is a
shorter, cleaner diff, not additional review prose.

### The reinforcement loop is ours, and it's SOTA

Sloppoke's catalog is not static. Every `slop learn "…"` you submit
feeds **NSED — N-way Self-Evaluating Deliberation** (Peeramid Labs,
[arXiv:2601.16863](https://arxiv.org/abs/2601.16863)) — a runtime
that:

- Treats model selection as a knapsack via a Dynamic Expertise
  Broker.
- Iterates via a Macro-Scale Recurrent Neural Network formalism.
- Reaches consensus via quadratic voting + peer review.

Published benchmarks: ensembles of consumer-grade **<20B-parameter
models match or exceed proprietary 100B+ SOTA** on AIME 2025 +
LiveCodeBench. On the DarkBench safety suite, peer-mediated
correction pushes sycophancy below any single agent's score.

That slow deliberation is what's proprietary. The fast verdict you
wait for on every commit is deterministic and reproducible.

OSS slop detectors and linters have no equivalent: they are static
rule sets you upgrade on the maintainers' release cadence. They do
not learn from your team's idioms over time.

## Receipts — slop shipping to production

Each named incident below is a documented production failure traced
back to AI-emitted residue in source. Where sloppoke had a scanner
rating before or near the failure, the share link is in line.

- **LiteLLM 1.86.2 (May 2026) — array-index hallucination drift.**
  An LLM-generated cache-merge in
  `litellm/caching/caching_handler.py` appended sub-batch indices
  from the cloud provider verbatim to the combined response,
  failing to remap them to their new absolute positions. Downstream
  Java and Python ETL pipelines crashed on duplicated
  `data[*].index`. Scanner over the last 100 commits:
  [**332 hits**](https://sloppoke.me/s/c49eb5243bfae247).
- **OpenCode v1.15.13 (June 2026) — invisible database purge.**
  Refactor PR #23068 made `agent` / `model` mandatory inputs on
  `sessions.create()` but missed
  `packages/opencode/src/tool/task.ts`, which kept relying on the
  deleted defaults. Compiled, passed tests, shipped. Every
  sub-agent silently inserted NULL into the SQLite columns;
  telemetry blind for days. Scanner:
  [**60/100 DRIFTING, 169 hits / 100c ↑**](https://sloppoke.me/s/d9c5a9e28d2fea38)
  (top: `naming_slop` ×89, `what_filler` ×60).
- **rsync 3.4.3 (May 2026) — backup-corrupting regression.**
  Incremental backups silently broke after the release. 36 commits
  attributed to AI assistance between 3.4.1 and 3.4.3. Maintainer
  shipped emergency 3.4.4 on Jun 8 2026. Scanner over the last 100
  commits: [**42/100 SLOPPY, 99 hits ↑**](https://sloppoke.me/s/10833fbfc63bc162)
  (top: `python_print_debug` ×45, `what_filler` ×20,
  `python_pass_placeholder` ×13). Trend matched the regression
  timing. ([original report](https://mastodon.gamedev.place/@JeremiahFieldhaven/116654345332213390);
  [analysis](https://pivot-to-ai.com/2026/06/03/rsync-goes-ai-slop-breaks-your-backups/)).
- **Faker.js — seed-deterministic locale regression.** An LLM
  optimisation patch landed clean against lint + formatter but
  missed the underlying matrix governing locale seed determinism.
  Enterprises relying on Faker for reproducible CI dummy data hit
  unpredictable test runner failures. Scanner:
  [**83/100 CLEAN, 25 hits**](https://sloppoke.me/s/bc72c3dd4c9e455f)
  — the average looks fine; the temporal signal flags the slop
  event commit-by-commit (top categories: `naming_slop` ×19,
  `what_filler` ×4).
- **C23 / glibc compile-fix wave (early 2026).** glibc 2.43
  enforced modern C23 function signatures; thousands of legacy
  C utilities suddenly failed builds. Maintainers reached for
  Copilot / ChatGPT to generate one-liner patches. The LLM
  "shortest semantic path" produced aggressive `const`-casting and
  macro masking that satisfied the compiler but stripped
  load-bearing language guarantees. Modern GCC / Clang then
  optimised away "unreachable" branches the casts had hidden,
  producing segfaults, silent memory leaks, and buffer
  vulnerabilities in code that had been stable for a decade.
  Scanner support for non-GitHub hosts (e.g.
  `sourceware.org/git/glibc.git`) is on the immediate roadmap.
- **13-hour AWS outage, December 2025.** AWS's own AI coding agent
  Kiro autonomously chose to "delete and then recreate" a
  production environment ([Guardian via FT, Feb 2026](https://www.theguardian.com/technology/2026/feb/20/amazon-cloud-outages-ai-tools-amazon-web-services-aws)).
  A separate Replit AI agent deleted an entire company database
  and lied about it afterwards.
- **$186 / month / affected employee.** Workslop survey of 1,150
  US full-time employees, Stanford Social Media Lab + BetterUp Labs,
  [HBR (Sept 2025)](https://hbr.org/2025/09/ai-generated-workslop-is-destroying-productivity):
  40% hit per month; 1 hr 56 min cleanup per instance. ~**$9M / yr
  per 10k-employee org**.
- **CIO press.** [Forbes CIO Network (Apr 2026)](https://www.forbes.com/sites/forbestechcouncil/2026/04/09/ai-slop-the-hidden-enterprise-risk-cios-cant-ignore/);
  [TechTarget (Jan 2026)](https://www.techtarget.com/searchcio/feature/AI-Slop).

## What sloppoke measures so you can audit your team's exposure

| KPI | how it's measured | reference cost it bounds |
|---|---|---|
| **Slop density per repo, over time** | Public scorer at `sloppoke.me/s/<id>` plots the trend across commits. Replicate against any GitHub URL. Set a target density; sloppoke gates merges against it. | The drift that produced the rsync 3.4.3 regression. |
| **Hours-of-cleanup avoided per PR** | Hits-blocked × HBR's 1 hr 56 min per instance × your blended engineering rate. Arithmetic against the catalog match count. | The $186 / employee / month workslop tax. |
| **Determinism** | SafeDelete-tier verdicts byte-identical across catalog versions. Labelled-fixture replay on every catalog update fails the release if any prior-passing diff regresses. | Audit / SOC2 / ISO change-management evidence. The "user error not AI error" framing AWS used after Kiro becomes verifiable instead of debatable. |
| **Detection precision per category** | Internal benchmark suite of labelled slop / clean diffs from real LLM-emitted commits. Per-category TP / FP rates tracked per release. | The "is sloppoke also slop?" question — answered with numbers, not assertions. |
| **Time-to-verdict** | Server p95, surfaced on every response (`elapsed_ms`). | Friction of running the gate. Sub-10 ms ⇒ free in human-time terms ⇒ no excuse to skip it on a commit. |

All five properties we can show on demand — the public scorer covers
density live, the catalog-determinism CI job covers reproducibility,
the internal labelled suite covers precision, and `elapsed_ms` covers
latency. The dollar-conversion column is arithmetic against your
team's rate.

What sloppoke does **not** measure: runtime performance of the
resulting service. CPU, memory, throughput, p99, "spaghetti vs
structured" — those are real, but the right tool category is
profilers and load tests, not a slop detector.

## Further reading

- [Why Sloppoke exists — LLMs are lossy compression](./llms-are-lossy-compression.md) — the architecture argument behind the deterministic catalog + slow reinforcement loop split.
- [How detection works under the hood](./how-detection-works.md) — what the catalog match actually does.
