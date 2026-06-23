# Correlation study — slop density vs OSS production failures

> **Status:** anecdotal alignment at n = 5 hand-labelled cases.
> **Not statistical proof.** Confidence interval undefined.
> Methodology pre-registered; corpus open under `sloppoke-bench/`
> so anyone can re-run.

The sloppoke FAQ claims a correlation between the density of
known LLM-coding patterns in source and downstream production
failures. This page is the public artefact backing the claim.

The honest framing matters. This is **anecdotal alignment** until
the corpus reaches n ≥ 100. The page is public anyway because:

1. The signal in the seed n = 5 cases is strong enough to be worth
   discussing without overselling.
2. The methodology, the inclusion criteria, and the named
   confounders are pre-registered. Operators can audit the work
   before the larger corpus closes.
3. The statistical correlation at n ≥ 100 with confidence
   intervals supersedes this page when it lands; the case-study
   walkthroughs underneath remain.

## What the bench measures

For every case in `sloppoke-bench/corpus/*.yml` we record:

| field | what |
|---|---|
| `introducing_commit` | the AI-attributed commit that landed the slop |
| `fix_commit` / `fix_release` | the commit / release that rolled it back |
| `sloppoke_scan` | per-bucket score sweep of the repo's history |
| `severity` | P0 (data loss, security, ≥ 1 hr downtime) → P3 (cosmetic) |

The scanner walks rolling commit windows backward through history
(HEAD, HEAD~50, HEAD~100, …, up to ~1000 commits back) via the
public scorer's `head_sha` override (added in the same PR that
ships this page; see `crates/.../sloppoke-server/src/public_score.rs`).

## Inclusion criteria (pre-registered)

A case enters the corpus only if **all three** hold:

1. **AI attribution** — explicit evidence in the introducing
   commit (trailer, PR statement, maintainer disclosure, paper
   citation). Free-text rumour does NOT count.
2. **Production impact** — reached a tagged release OR a
   downstream package OR a CVE OR a user-visible outage.
   Build/test breakage caught pre-release does NOT count.
3. **Sources extant** — repo + introducing commit + failure
   description all still publicly accessible.

Confidence per case is scored 1–5; only `confidence >= 3` enters
the main corpus. `1`–`2` candidates wait in `corpus/candidates.yml`.

## Confounders, named up front

- **Velocity bias.** High-velocity repos correlate with both AI use
  and incidents. The statistical analysis controls for it via
  commit-rate normalisation.
- **Survivor bias.** Public AI-failure write-ups exist only for
  loud breakage. The silent-slop floor is unknown.
- **Attribution drift.** Trailers fade out as AI use becomes the
  default. 2024 vs 2026 trailer rates are not comparable in
  absolute terms.
- **Catalog version.** Each scan is pinned to a catalog version.
  Trend lines re-run on every major catalog release.

## Seed corpus — five hand-labelled cases

| case | severity | scanner verdict |
|---|---|---|
| [LiteLLM 1.86.2 array-index drift](https://sloppoke.me/s/c49eb5243bfae247) | P1 | 332 hits / 100 commits |
| [OpenCode v1.15.13 NULL sub-agent](https://sloppoke.me/s/d9c5a9e28d2fea38) | P1 | 60/100 DRIFTING, 169 hits / 100c ↑ |
| [rsync 3.4.3 backup-corrupting](https://sloppoke.me/s/10833fbfc63bc162) | P0 | 42/100 SLOPPY, 99 hits / 100c ↑ |
| [Faker.js seed-determinism](https://sloppoke.me/s/bc72c3dd4c9e455f) | P2 | 83/100 CLEAN, 25 hits (commit-level jumps) |
| C23 / glibc compile-fix wave | P0 (pattern) | scanner support inbound for sourceware |

Each case in `sloppoke-bench/corpus/<id>.yml` carries the full
evidence chain.

## What the case plots show

`scripts/scan-corpus.sh` renders per-case SVGs to
`sloppoke-bench/plot/output/<case_id>.svg` and the aggregate
severity × peak-density scatter to `aggregate.svg`. The docs build
embeds them inline once the scan results JSON lands under
`sloppoke-bench/results/`:

```html
<img src="/assets/correlation-study/litellm.svg" alt="LiteLLM 1.86.2 timeline">
```

The expected shape per case:

```
hits / 100c
   ▲ failure marker
   │
   │       ▲
   │      ╱ ╲
   │     ╱   ╲___ baseline
───┴────────────────────→ commits before HEAD
```

When the spike + marker line up, the per-case caption surfaces
"slop density rose above baseline in the N-commit window preceding
the failure". When they don't, we say so — negative results are
not suppressed.

## How the corpus is built

The corpus is the gate. n = 5 is anecdote; n ≥ 100 is statistics;
n ≥ 1000 is publishable.

Crawl channels (see `sloppoke-bench/crawler/`):

1. **Trailers** — `crawler/trailers.py` queries GitHub Search for
   `Co-Authored-By: Claude`, `Generated with [Cursor]`, etc.
2. **Regression pairs** — `crawler/regression_pair_finder.py`
   walks the next N commits after each trailer-attributed commit
   looking for `Revert`, `Fixes regression in <sha>`, or
   regression-shape language.
3. **Post-mortems** — `crawler/postmortem_harvester.py` reads
   Mastodon hashtag feeds + HN search + Pivot-to-AI for AI-failure
   write-ups.
4. **Academic datasets** — `crawler/dataset_loader.py` normalises
   three published corpora into the same candidate schema:
   - **AgenticFlict** ([arXiv:2604.03551](https://arxiv.org/abs/2604.03551))
     — 142K+ AI-coding-agent PRs across 59K+ repos, 27.67%
     conflict rate. The single biggest yield source.
   - **GitHub Recent Bugs** ([arXiv:2310.13229](https://arxiv.org/abs/2310.13229))
     — 107 hand-verified Java bugs across 16 popular OSS repos.
   - **Android Build Repair** ([arXiv:2510.08640](https://arxiv.org/abs/2510.08640))
     — 1,019 build failures including an LLM-generated-errors
     subset.

After production-impact filtering (the inclusion criteria above)
the realistic statistical-corpus size is **500–1500 confirmed
cases**, not the 100–200 the trailer + press channels alone reach.
The crawlers run **only outside CI** against operator-owned PATs.
Tests exercise the parsing layer with canned fixtures under
`crawler/fixtures/`.

## Statistical correlation (when n ≥ 100)

Pre-registered metrics:

- Point-biserial correlation between
  `max(hits/100c) in the 100 commits before failure_T` and
  `failure within N commits`.
- Logistic regression: `P(failure | slop_density, velocity,
  contributor_count, repo_size)`.
- Survival analysis: time-to-failure as a function of slop score
  at commit T.
- Per-category breakdown — which catalog categories carry the
  signal?

Published when n ≥ 100 confirmed cases.

## Re-running the analysis

```sh
# Crawl candidates (operator runs against own PAT)
GH_TOKEN=ghp_xxx python -m sloppoke_bench.crawler.trailers \
  --since 2025-01-01

# Promote crawler candidates into corpus YAMLs by hand
$EDITOR sloppoke-bench/corpus/<new-case>.yml

# Scan each case at rolling windows
SLOPPOKE_BASE=https://sloppoke.me sloppoke-bench/scripts/scan-corpus.sh

# Render the plots
python sloppoke-bench/plot/render.py
```

The published page on sloppoke.me freezes the catalog version + the
corpus snapshot used. Anyone can replicate against the same inputs
and get the same SVGs.

## References

- [LiteLLM 1.86.2 — array-index hallucination scanner](https://sloppoke.me/s/c49eb5243bfae247)
- [OpenCode v1.15.13 — sub-agent NULL scanner](https://sloppoke.me/s/d9c5a9e28d2fea38)
- [rsync 3.4.3 — backup-corruption scanner](https://sloppoke.me/s/10833fbfc63bc162)
- [Faker.js — seed-determinism scanner](https://sloppoke.me/s/bc72c3dd4c9e455f)
- [Guardian / FT — AWS Kiro outage Dec 2025](https://www.theguardian.com/technology/2026/feb/20/amazon-cloud-outages-ai-tools-amazon-web-services-aws)
- [HBR · Stanford · BetterUp workslop survey (Sept 2025)](https://hbr.org/2025/09/ai-generated-workslop-is-destroying-productivity)
- [Forbes — AI slop hidden enterprise risk (Apr 2026)](https://www.forbes.com/sites/forbestechcouncil/2026/04/09/ai-slop-the-hidden-enterprise-risk-cios-cant-ignore/)
- [TechTarget — AI slop enterprise risks (Jan 2026)](https://www.techtarget.com/searchcio/feature/AI-Slop)
- [Pivot to AI — rsync goes AI slop (Jun 2026)](https://pivot-to-ai.com/2026/06/03/rsync-goes-ai-slop-breaks-your-backups/)
