# Sloppoke documentation

The `slop` CLI catches AI-generated noise — defensive bloat, narrative
comments, untested branches, hallucinated imports — at the commit
boundary, before it lands in your repo.

## Tutorials — learn by doing

For new users. Single linear path. Every step verified.

- [Your first slop poke](tutorials/01-first-poke.md) — install, log in,
  scan your first staged diff, apply the cleanup. ~5 minutes.

## How-to — recipes for real tasks

For users who already know the basics. Pick the goal, follow the
recipe.

- [Install the pre-commit hook](how-to/install-pre-commit-hook.md)
- [Gate a pull request in CI](how-to/gate-a-pull-request-in-ci.md)
- [Use the Claude Code plugin](how-to/use-claude-code-plugin.md)
- [Score a public GitHub repo](how-to/score-a-public-repo.md)
- [Submit feedback to the learning loop](how-to/submit-feedback.md)
- [Install from source](how-to/install-from-source.md)
- [Verify the binary](how-to/verify-the-binary.md) — supply-chain checks (SHA-256, SHA256SUMS, Sigstore attestation)

## Reference — facts

For users with the docs page open while working. Authoritative,
predictable.

- [CLI commands and flags](reference/cli.md)
- [Configuration (env vars + files)](reference/config.md)
- [Exit codes](reference/exit-codes.md)
- [Detection categories](reference/catalog.md)

## Explanation — understanding

For users who want to know *why*, not just *how*.

- [What "AI slop" actually means](explanation/what-is-slop.md)
- [Why Sloppoke exists: LLMs are lossy compression](explanation/llms-are-lossy-compression.md)
- [How sloppoke compares — CodeRabbit, OSS slop detectors, linters](explanation/how-sloppoke-compares.md)
- [Why pre-commit is the right boundary](explanation/why-pre-commit.md)
- [How detection works under the hood](explanation/how-detection-works.md)
- [Privacy + identity model](explanation/privacy-and-identity.md)

## Pointers outside these docs

- Product page: [sloppoke.me](https://sloppoke.me)
- Enterprise ROI model: [sloppoke.me/enterprise](https://sloppoke.me/enterprise)
- Source + issue tracker: <https://github.com/peeramid-labs/sloppoke>
- News + release notes inside the CLI: `slop news`
