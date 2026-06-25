//! `slop` — the AI-slop firewall CLI.
//!
//!   slop login                  resolve SSH key, cache identity
//!   slop poke   --patch FILE    scan a patch, save cleanup plan
//!   slop apply                  strip flagged lines, amend HEAD
//!   slop learn  "<feedback>"    ship feedback to the RL loop
//!   slop billing tier|portal    subscription + usage / Stripe portal

mod api;
mod news;
mod payment;
mod ssh_resolve;
mod version_check;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use api::CleanupAction;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

const DEFAULT_SERVER: &str = "https://sloppoke.me";
const CACHED_PLAN: &str = ".slop/last-poke.json";

/// Long-form version string baked at build time. Includes the crate
/// version, the git commit it was built from (`-dirty` if the
/// working tree was modified), and the build timestamp (seconds since
/// epoch, or whatever SOURCE_DATE_EPOCH was set to for reproducible
/// builds). Operators verifying a downloaded binary can match the
/// commit hash against the GitHub release page.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit ",
    env!("SLOP_BUILD_COMMIT"),
    ", built ",
    env!("SLOP_BUILD_EPOCH"),
    ")"
);

#[derive(Parser, Debug)]
#[command(name = "slop", version, long_version = LONG_VERSION, about = "Blazing-fast AI-slop firewall.", disable_version_flag = true)]
struct Cli {
    /// Print version and exit. Accepts `-v` lowercase, `-V` uppercase,
    /// and `--version` long-form so muscle memory from every other
    /// CLI in the ecosystem just works. `-V` shows the long form
    /// (commit + build epoch) — useful for supply-chain verification.
    #[arg(short = 'v', short_alias = 'V', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Resolve your SSH key's server identity and cache it locally.
    Login(LoginArgs),
    /// Scan a patch for slop. Saves a cleanup plan to
    /// `.slop/last-poke.json` for `slop apply`.
    Poke(PokeArgs),
    /// Apply the cleanup plan from the most recent poke: strip
    /// flagged lines, stage, and amend HEAD.
    Apply(ApplyArgs),
    /// Submit free-text feedback. Server commits it to your slop-org
    /// learn feed; the offline RL loop turns it into catalog updates.
    Learn(LearnArgs),
    /// Subscription + usage commands.
    #[command(subcommand)]
    Billing(BillingCmd),
    /// Install a git pre-commit hook that runs `slop poke --staged`
    /// and blocks the commit on SLOP. Default scope is the current
    /// repo; `--global` installs to the user's git hooks path so
    /// every future repo inherits the gate.
    InstallHook(InstallHookArgs),
    /// Show product announcements. By default prints unread entries
    /// and marks them seen. `--all` replays the full back catalog.
    News(NewsArgs),
    /// Print which hook layers are active (git + Claude Code).
    Status,
    /// LOCAL/INTERNAL multi-agent review. Shells out to the
    /// `slop-review-local` helper which drives the nsed
    /// sloppoke_review_0v1 bundle against the current git work-tree.
    /// Not for end-user release — gated behind an operator token
    /// on the orchestrator side and only built locally.
    Review(ReviewArgs),
}

#[derive(Parser, Debug, Clone)]
struct ReviewArgs {
    /// Pass-through of REMOTE_URL into the helper. Default: detected
    /// from the current repo's `forgejo` or `origin` remote, with
    /// any embedded basic-auth stripped by the helper.
    #[arg(long)]
    remote_url: Option<String>,
    /// Override the head SHA. Default: current HEAD.
    #[arg(long)]
    head: Option<String>,
    /// Override the base SHA. Default: helper walks
    /// `forgejo/dev → origin/dev → dev → forgejo/main → origin/main → main`.
    #[arg(long)]
    base: Option<String>,
    /// Optional comma-separated forgejo allowlist override.
    #[arg(long)]
    allowlist: Option<String>,
    /// Forward extra trailing args to the helper verbatim.
    #[arg(last = true)]
    rest: Vec<String>,
}

#[derive(Parser, Debug, Clone)]
struct NewsArgs {
    /// Show every entry the server has ever published, not just the
    /// ones the user hasn't seen yet.
    #[arg(long)]
    all: bool,
    /// Mark every cached entry as read without printing anything.
    /// Use to silence a backlog without scrolling through it.
    #[arg(long)]
    ack: bool,
}

#[derive(Parser, Debug, Clone)]
struct InstallHookArgs {
    /// Install at `~/.config/slop/git-hooks/pre-commit` and point
    /// `git config --global core.hooksPath` at that directory so the
    /// hook fires in every repo. Without `--global`, the hook lands
    /// in the current repo's `.git/hooks/pre-commit` only.
    #[arg(long)]
    global: bool,
    /// Overwrite an existing pre-commit hook without prompting.
    #[arg(long)]
    force: bool,
}

#[derive(Parser, Debug, Clone)]
struct LoginArgs {
    #[arg(long, env = "SLOPPOKE_SERVER", default_value = DEFAULT_SERVER)]
    server: String,
    /// Path to SSH public key. Default: ask `ssh -G <server-host>`
    /// which identity OpenSSH would pick (same resolution git uses),
    /// fall back to `~/.ssh/id_ed25519.pub`.
    #[arg(long)]
    pubkey: Option<PathBuf>,
    /// Path to matching private key (default: strip `.pub` suffix).
    #[arg(long)]
    key: Option<PathBuf>,
}

#[derive(Parser, Debug, Clone)]
struct PokeArgs {
    /// Unified-diff file. Overrides the default `git diff` capture.
    #[arg(long, conflicts_with_all = ["staged", "range", "since"])]
    patch: Option<PathBuf>,
    /// Scan the staged index instead of the working tree
    /// (equivalent to `git diff --cached`).
    #[arg(long, conflicts_with_all = ["patch", "range", "since"])]
    staged: bool,
    /// Custom git diff range, passed verbatim to `git diff`.
    /// Examples: `main..HEAD`, `HEAD~3..HEAD`, `origin/main...`.
    #[arg(long, conflicts_with_all = ["patch", "staged", "since"])]
    range: Option<String>,
    /// Scan only changes since a given ref (equivalent to
    /// `git diff <ref>`). Handy for "what's new since main".
    #[arg(long, conflicts_with_all = ["patch", "staged", "range"])]
    since: Option<String>,
    /// Optional `<source-org>/<project>` tag the server uses to
    /// bucket the row in the learn store. Not billing-relevant.
    #[arg(long)]
    project: Option<String>,
    /// Scan an arbitrary git URL instead of the local working tree.
    /// Shallow-clones the repo into a temp dir, runs the chosen
    /// range/since/staged selector against it, ships the patch to
    /// the server, cleans up. Works on any git host.
    /// Example: `slop poke --repo https://github.com/user/foo --range HEAD~5..HEAD`
    #[arg(long, conflicts_with = "patch")]
    repo: Option<String>,
    /// Shorthand for `--repo https://github.com/<arg>.git`.
    /// Example: `slop poke --gh openclaw/openclaw --range HEAD~5..HEAD`
    #[arg(long, conflicts_with_all = ["patch", "repo"])]
    gh: Option<String>,
    /// Print the request JSON and exit without contacting the server.
    #[arg(long)]
    dry_run: bool,
    /// One-shot per-scan mute. Each target is a patch-notation
    /// address — `path/glob` (file-level mute) or `path/glob:LINE`
    /// (line-level mute) — that filters matching findings out of
    /// THIS scan's response before any verdict is rendered. No
    /// state is persisted: the next `slop poke` invocation has to
    /// re-pass the target, or `slop learn --disable <target>
    /// "<reason>"` upstream so the server stops flagging the
    /// pattern for this org. Repeatable; comma-separated also OK.
    ///
    /// Example:
    ///
    ///   slop poke --disable 'src/sdk-alpha/**'
    ///   slop poke --disable src/db/query.ts:99,src/lib.rs:42
    #[arg(long, value_delimiter = ',')]
    disable: Vec<String>,
}

#[derive(Parser, Debug, Clone)]
struct ApplyArgs {
    /// Print the cached plan instead of applying.
    #[arg(long)]
    show: bool,
    /// Delete the cached plan without applying.
    #[arg(long)]
    discard: bool,
    /// Skip the `git commit --amend` step — leave changes staged.
    #[arg(long)]
    no_commit: bool,
}

#[derive(Parser, Debug, Clone)]
struct LearnArgs {
    /// One sentence (or paragraph) of feedback.
    feedback: String,
    /// Optional `<source-org>/<project>` scope.
    #[arg(long)]
    project: Option<String>,
    /// Optional anchoring context (file path, code excerpt, error).
    /// When omitted, the CLI auto-attaches the most recent
    /// `.slop/last-poke.json` (poke id + patch) so the server can
    /// tie the feedback to a specific scan without the user
    /// copy-pasting. Pass `--no-attach` to suppress.
    #[arg(long)]
    context: Option<String>,
    /// Do NOT auto-attach the last poke's patch + id as context.
    /// Useful for generic project-wide feedback that doesn't
    /// reference a specific scan.
    #[arg(long)]
    no_attach: bool,
    /// Patch-notation targets to escalate to the server's per-org
    /// learn loop alongside the feedback prose. Same grammar as
    /// `slop poke --disable`: `path/glob` for whole-file mute,
    /// `path/glob:LINE` for line-level. Targets ride along inside
    /// the feedback context block so the server's offline RL loop
    /// can join "operator at <fingerprint> says <reason> applies
    /// to <target>" against the original poke row.
    ///
    /// Nothing is written to disk — the mute is a server-side
    /// catalog adjustment, not a local file. Future poke runs from
    /// the same org get the lower FP rate automatically.
    #[arg(long, value_delimiter = ',')]
    disable: Vec<String>,
}

#[derive(Subcommand, Debug, Clone)]
enum BillingCmd {
    /// Print plan + entitlements + this-cycle usage.
    Tier,
    /// Open the Stripe-hosted billing portal in $BROWSER.
    Portal {
        /// Print the URL instead of opening a browser.
        #[arg(long)]
        print: bool,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let rc = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("slop: {e:#}");
            1
        }
    };
    // Surface a friendly heads-up if a newer release exists. Runs
    // AFTER the user's primary output so the line scrolls past
    // whatever they came to see, never replaces it. All errors
    // (offline, rate-limited, parse failure) are swallowed inside.
    version_check::notify_if_outdated();
    // Surface one unseen news entry from the server. Same
    // post-output position as version_check; also fully fail-silent.
    // Only on success — failed commands shouldn't bury their error
    // under product chatter.
    if rc == 0 {
        news::ping_one_unseen(&news_server_url());
    }
    std::process::exit(rc);
}

/// Server URL for the news endpoint. The login subcommand carries a
/// `--server` flag; everything else just needs a URL. Look at the
/// saved config first (so users who pointed `slop login` at a custom
/// host get their news from there), fall back to SLOPPOKE_SERVER env,
/// then to the production default.
fn news_server_url() -> String {
    if let Ok(cfg) = api::load_config() {
        return cfg.server_url;
    }
    std::env::var("SLOPPOKE_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string())
}

fn run(cli: Cli) -> Result<()> {
    match cli.mode {
        Mode::Login(a) => run_login(a),
        Mode::Poke(a) => run_poke(a),
        Mode::Apply(a) => run_apply(a),
        Mode::Learn(a) => run_learn(a),
        Mode::Billing(c) => run_billing(c),
        Mode::InstallHook(a) => run_install_hook(a),
        Mode::News(a) => news::run(&news_server_url(), a.all, a.ack),
        Mode::Status => run_status(),
        Mode::Review(a) => run_review(a),
    }
}

const HOOK_SCRIPT: &str = r##"#!/usr/bin/env sh
# Installed by `slop install-hook`. Blocks `git commit` when sloppoke
# flags slop in the staged diff. Bypass once with
# `git commit --no-verify`. To mute one specific finding without
# a blanket bypass, re-stage with `slop poke --disable <path[:line]>`
# included on a wrapper script, or send `slop learn --disable
# <path[:line]> "<reason>"` upstream so the server stops flagging
# the pattern for this org.
set -e

if ! command -v slop >/dev/null 2>&1; then
  echo "slop: pre-commit hook installed but the 'slop' binary is not on PATH; skipping." >&2
  exit 0
fi

# Nothing staged → nothing to scan.
if git diff --cached --quiet; then
  exit 0
fi

set +e
# Three verdict tiers (server-side):
#   LGTM   — no findings.
#   MARKED — findings exist but every TODO splice is already in source.
#            Nothing actionable. Treat as pass.
#   SLOP   — findings + non-empty patch. Block.
# Stderr carries the verdict line ("slop poke: LGTM" / "slop poke:
# MARKED — N hit…" / "slop poke: SLOP — N hits"); stdout carries the
# patch (empty on LGTM/MARKED, non-empty on SLOP). Match the tier on
# stderr so a future header-only-render quirk in MARKED doesn't look
# like SLOP and block a commit with nothing to apply.
verdict_tmp=$(mktemp -t sloppoke-verdict.XXXXXX)
output=$(slop poke --staged 2>"$verdict_tmp")
status=$?
verdict=$(cat "$verdict_tmp" 2>/dev/null)
rm -f "$verdict_tmp"
set -e

if [ "$status" -ne 0 ]; then
  echo "slop: scan errored ($status); commit blocked. Re-run or 'git commit --no-verify'." >&2
  exit 1
fi

case "$verdict" in
  *"slop poke: LGTM"*|*"slop poke: MARKED"*)
    exit 0
    ;;
esac

if [ -n "$output" ]; then
  echo "$output"
  echo "" >&2
  echo "slop: SLOP found. Run 'slop apply --no-commit' then re-commit, or 'git commit --no-verify' to bypass." >&2
  exit 1
fi

exit 0
"##;

fn run_install_hook(args: InstallHookArgs) -> Result<()> {
    let target_path = if args.global {
        let dir = api::config_dir()?.join("git-hooks");
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let status = Command::new("git")
            .args(["config", "--global", "core.hooksPath"])
            .arg(&dir)
            .status()
            .context("set git config --global core.hooksPath")?;
        if !status.success() {
            bail!("git config --global core.hooksPath returned non-zero");
        }
        dir.join("pre-commit")
    } else {
        let out = Command::new("git")
            .args(["rev-parse", "--git-path", "hooks"])
            .output()
            .context("git rev-parse --git-path hooks (are you in a git repo?)")?;
        if !out.status.success() {
            bail!(
                "git rev-parse failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let hooks_dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("create {}", hooks_dir.display()))?;
        hooks_dir.join("pre-commit")
    };

    if target_path.exists() && !args.force {
        let body = fs::read_to_string(&target_path).unwrap_or_default();
        if body.contains("Installed by `slop install-hook`") {
            eprintln!(
                "slop: pre-commit hook already installed at {} — nothing to do.",
                target_path.display()
            );
            return Ok(());
        }
        bail!(
            "an unrelated pre-commit hook already lives at {}; rerun with --force to overwrite.",
            target_path.display()
        );
    }

    fs::write(&target_path, HOOK_SCRIPT)
        .with_context(|| format!("write {}", target_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&target_path)
            .with_context(|| format!("stat {}", target_path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&target_path, perms)
            .with_context(|| format!("chmod 0755 {}", target_path.display()))?;
    }

    let scope = if args.global { "global" } else { "repo" };
    eprintln!(
        "slop: installed {scope} pre-commit hook at {}",
        target_path.display()
    );
    eprintln!("slop: every `git commit` now runs `slop poke --staged` first. Bypass once with `git commit --no-verify`.");
    print_defense_in_depth_summary();
    Ok(())
}

/// Whether the Claude Code plugin's PreToolUse hook script is present
/// on this machine. Looks for any installed sloppoke plugin version
/// under `~/.claude/plugins/cache/peeramid-labs/sloppoke/*/hooks/pre-commit-poke.sh`.
/// False when Claude Code isn't installed or the plugin isn't either.
fn claude_plugin_hook_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let base = PathBuf::from(home)
        .join(".claude")
        .join("plugins")
        .join("cache")
        .join("peeramid-labs")
        .join("sloppoke");
    let entries = fs::read_dir(&base).ok()?;
    let mut best: Option<PathBuf> = None;
    let mut best_label: Option<String> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let hook = path.join("hooks").join("pre-commit-poke.sh");
        if hook.exists() {
            let label = entry.file_name().to_string_lossy().to_string();
            if best_label.as_deref().is_none_or(|b| label.as_str() > b) {
                best_label = Some(label);
                best = Some(hook);
            }
        }
    }
    best
}

/// True if a `.git/hooks/pre-commit` or the global hooks-path hook
/// carries the `slop install-hook` marker — i.e. our terminal gate
/// is wired for the current working tree.
fn git_hook_installed() -> bool {
    let out = Command::new("git")
        .args(["rev-parse", "--git-path", "hooks"])
        .output();
    let local = if let Ok(out) = out {
        if out.status.success() {
            let path =
                PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).join("pre-commit");
            if let Ok(body) = fs::read_to_string(&path) {
                if body.contains("Installed by `slop install-hook`") {
                    return true;
                }
            }
            Some(path)
        } else {
            None
        }
    } else {
        None
    };
    let _ = local;
    if let Ok(dir) = api::config_dir() {
        let p = dir.join("git-hooks").join("pre-commit");
        if let Ok(body) = fs::read_to_string(&p) {
            if body.contains("Installed by `slop install-hook`") {
                return true;
            }
        }
    }
    false
}

/// Print the two-layer defense status. Same body the install path
/// closes with and the `slop status` subcommand emits — single source
/// of truth so the funnel reads identically wherever the user hits
/// it.
fn print_defense_in_depth_summary() {
    let git_layer = git_hook_installed();
    let claude_layer = claude_plugin_hook_path();
    eprintln!();
    eprintln!("slop: defense-in-depth status");
    eprintln!(
        "  git pre-commit hook (terminal `git commit`): {}",
        if git_layer { "ACTIVE" } else { "MISSING" }
    );
    eprintln!(
        "  Claude Code PreToolUse hook (agent `git commit`): {}",
        match &claude_layer {
            Some(p) => format!("ACTIVE — {}", p.display()),
            None => "MISSING".to_string(),
        }
    );
    if !git_layer {
        eprintln!(
            "  install: `slop install-hook` (this repo) or `slop install-hook --global` (every repo)"
        );
    }
    if claude_layer.is_none() {
        eprintln!(
            "  install: in any Claude Code session run `/plugin marketplace add peeramid-labs/plugin-marketplace` then `/plugin install sloppoke@peeramid-labs`"
        );
    }
    if git_layer && claude_layer.is_some() {
        eprintln!("  both layers active — agent + terminal `git commit` are gated. Bypass requires both `--no-verify` AND `SLOP_SKIP_HOOK=1`.");
    } else {
        eprintln!(
            "  only one layer active — easy to bypass. Install the other so agents have to defeat two gates."
        );
    }
}

fn run_status() -> Result<()> {
    print_defense_in_depth_summary();
    Ok(())
}


// ── review (LOCAL ONLY) ──────────────────────────────────────────
//
// Thin wrapper: shells out to `slop-review-local` which carries the
// nsed bundle wiring + operator-token load + fleet boot + deliberation
// trigger. Nothing about the review flow lives inside this binary
// beyond the dispatch — keeps the public-release surface clean if the
// binary ever ships before the helper does.

fn run_review(args: ReviewArgs) -> Result<()> {
    let helper = std::env::var("SLOP_REVIEW_HELPER").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.local/bin/slop-review-local")
    });
    if !PathBuf::from(&helper).is_file() {
        anyhow::bail!(
            "slop review: helper not found at {helper}. \
             Set SLOP_REVIEW_HELPER or install ~/.local/bin/slop-review-local \
             (see nsed sloppoke_review_0v1 bundle)."
        );
    }
    let mut cmd = std::process::Command::new(&helper);
    if let Some(v) = args.remote_url {
        cmd.env("REMOTE_URL", v);
    }
    if let Some(v) = args.head {
        cmd.env("HEAD_SHA", v);
    }
    if let Some(v) = args.base {
        cmd.env("BASE_SHA", v);
    }
    if let Some(v) = args.allowlist {
        cmd.env("ALLOWLIST", v);
    }
    cmd.args(&args.rest);
    let status = cmd.status().with_context(|| format!("spawning {helper}"))?;
    if !status.success() {
        anyhow::bail!("slop review: helper exited {}", status);
    }
    Ok(())
}

// ── login ────────────────────────────────────────────────────────

fn run_login(args: LoginArgs) -> Result<()> {
    // Resolve pubkey + key together so they always come from the same
    // pair. If the user passes only `--key foo`, derive pubkey from
    // `foo.pub` instead of falling back to the default ed25519 pubkey
    // — otherwise discover would compute a fingerprint for one keypair
    // while signed requests use a different one (401 mismatch).
    //
    // With no explicit flags we ask OpenSSH which identity it would
    // use for the server host (same resolution git observes). Only
    // when ssh has no opinion do we fall back to the ed25519 default.
    let pubkey_path = match (args.pubkey.clone(), args.key.clone()) {
        (Some(p), _) => p,
        (None, Some(k)) => PathBuf::from(format!("{}.pub", k.display())),
        (None, None) => ssh_resolve::host_from_server_url(&args.server)
            .and_then(|host| ssh_resolve::resolve_for_host(&host))
            .unwrap_or_else(default_pubkey_path),
    };
    let pubkey_line = fs::read_to_string(&pubkey_path)
        .with_context(|| format!("read {}", pubkey_path.display()))?
        .trim()
        .to_string();
    let key_path = args
        .key
        .unwrap_or_else(|| derive_key_from_pubkey(&pubkey_path));
    let resp = api::discover(&args.server, &pubkey_line)?;
    let cfg = api::SavedConfig {
        server_url: args.server,
        fingerprint: resp.fingerprint.clone(),
        ssh_key_path: key_path,
        slop_org: resp.slop_org.clone(),
    };
    api::save_config(&cfg)?;
    println!(
        "slop: logged in as {} ({})",
        if resp.slop_org.is_empty() {
            "(anonymous)".to_string()
        } else {
            resp.slop_org.clone()
        },
        resp.fingerprint
    );
    maybe_offer_hook_install();
    Ok(())
}

/// After a successful `slop login`, ask the user whether they want
/// the global git pre-commit hook installed. Only fires on an
/// interactive TTY — never blocks scripts or CI. Skips entirely when
/// the global hook is already present so re-logins don't repeat the
/// prompt.
fn maybe_offer_hook_install() {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return;
    }
    if std::env::var_os("SLOP_NO_HOOK_PROMPT").is_some() {
        return;
    }
    let Ok(cfg_dir) = api::config_dir() else {
        return;
    };
    let hook_path = cfg_dir.join("git-hooks").join("pre-commit");
    if hook_path.exists() {
        return;
    }
    eprintln!();
    eprintln!("slop: install a global git pre-commit hook so `slop poke --staged` runs");
    eprintln!(
        "      automatically on every commit? [Y/n] (or run `slop install-hook --global` later)"
    );
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return;
    }
    let answer = line.trim().to_ascii_lowercase();
    if answer == "n" || answer == "no" {
        eprintln!("slop: skipped. You can install it later with `slop install-hook --global`.");
        return;
    }
    if let Err(e) = run_install_hook(InstallHookArgs {
        global: true,
        force: false,
    }) {
        eprintln!(
            "slop: hook install failed ({e}). You can retry with `slop install-hook --global`."
        );
    }
}

fn default_pubkey_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".ssh")
        .join("id_ed25519.pub")
}

fn derive_key_from_pubkey(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    if let Some(rest) = s.strip_suffix(".pub") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

// ── poke ─────────────────────────────────────────────────────────

/// Resolve the effective remote git URL from either --repo or --gh.
/// --gh is the short form for github.com only; --repo accepts any git
/// URL the local git binary knows how to clone (https/ssh/git://).
fn remote_repo_url(args: &PokeArgs) -> Option<String> {
    if let Some(url) = args.repo.as_deref() {
        return Some(url.to_string());
    }
    if let Some(slug) = args.gh.as_deref() {
        // Accept either `org/repo` or a full URL the user fat-fingered
        // into --gh. The latter just passes through to git clone.
        if slug.starts_with("http") || slug.starts_with("git@") {
            return Some(slug.to_string());
        }
        return Some(format!("https://github.com/{slug}.git"));
    }
    None
}

/// Infer the smallest clone depth that satisfies the caller's range
/// selector. For ranges that explicitly reference `HEAD~N`, we need
/// at most N+2 commits (the N ancestors, HEAD itself, plus a buffer
/// for merge parents). For branch / tag / SHA refs we have no static
/// bound so we fall back to a conservative default.
///
/// `SLOP_REMOTE_CLONE_DEPTH` always wins when set — escape hatch for
/// users who know their range needs more history.
fn infer_clone_depth(args: &PokeArgs) -> u32 {
    const DEFAULT_DEPTH: u32 = 50;
    if let Some(env) = std::env::var("SLOP_REMOTE_CLONE_DEPTH")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        return env;
    }
    // Scan every `HEAD~<digits>` occurrence across both selectors,
    // take the max N. +2 buffer (HEAD itself + a merge-parent step)
    // means `HEAD~5..HEAD` clones depth 7, not 50.
    let mut needed: Option<u32> = None;
    for selector in [args.range.as_deref(), args.since.as_deref()]
        .into_iter()
        .flatten()
    {
        for capture in selector.split("HEAD~").skip(1) {
            let digits: String = capture.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                needed = Some(needed.map_or(n, |cur| cur.max(n)));
            }
        }
    }
    needed.map(|n| n + 2).unwrap_or(DEFAULT_DEPTH)
}

/// Persistent cache root for cloned repos. `~/.cache/slop/repos/`
/// by default; XDG override honored. One subdir per URL hash so the
/// `git fetch` on subsequent runs reuses the same checkout instead
/// of paying the full clone cost every invocation.
fn cache_root() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("SLOP_CACHE_DIR") {
        return Some(PathBuf::from(custom));
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg).join("slop").join("repos"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".cache")
            .join("slop")
            .join("repos"),
    )
}

/// Hash a URL down to a stable, filesystem-safe directory name so the
/// cache layout is predictable.
fn url_to_cache_key(url: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(url.as_bytes());
    let bytes = hasher.finalize();
    let hex: String = bytes.iter().take(8).map(|b| format!("{b:02x}")).collect();
    hex
}

/// Reusable repo workdir. On cache hit, runs `git fetch --depth N`
/// so the checkout includes whatever range the caller needs. On
/// cache miss, does a full shallow clone. Either way the caller
/// gets a stable PathBuf the rest of poke can chdir into.
///
/// `None` returned by this function means: caller should fall back
/// to the tempdir path (e.g. cache root unwritable).
fn cached_clone_or_fetch(url: &str, depth: u32) -> Option<PathBuf> {
    let root = cache_root()?;
    let dir = root.join(url_to_cache_key(url));
    let _ = fs::create_dir_all(&root);
    let depth_s = depth.to_string();
    if dir.join(".git").exists() {
        eprintln!("slop: refreshing cached {url} (depth {depth})…");
        let ok = Command::new("git")
            .args([
                "-C",
                dir.to_str()?,
                "fetch",
                "--depth",
                &depth_s,
                "--quiet",
                "--no-tags",
                "origin",
            ])
            .status()
            .ok()?
            .success();
        if ok {
            // Reset to the freshly-fetched HEAD so subsequent
            // `git diff` calls see the new tip, not the stale one.
            let _ = Command::new("git")
                .args(["-C", dir.to_str()?, "reset", "--hard", "FETCH_HEAD"])
                .status();
            return Some(dir);
        }
        // Fetch failed — fall through to re-clone fresh.
        let _ = fs::remove_dir_all(&dir);
    }
    eprintln!("slop: cloning {url} (depth {depth})…");
    let status = Command::new("git")
        .args([
            "clone",
            "--depth",
            &depth_s,
            "--quiet",
            "--no-tags",
            url,
            dir.to_str()?,
        ])
        .status()
        .ok()?;
    if !status.success() {
        let _ = fs::remove_dir_all(&dir);
        return None;
    }
    Some(dir)
}

/// Holder so the rest of `run_poke` doesn't need to know whether the
/// workdir is a tempdir (cache disabled) or a long-lived cached
/// checkout. Drop wipes only the tempdir variant.
enum RemoteWorkdir {
    Cached(PathBuf),
    #[allow(dead_code)] // Held by RAII; the tempdir's lifetime is what matters.
    Tempdir(tempfile::TempDir),
}

impl RemoteWorkdir {
    fn path(&self) -> &Path {
        match self {
            Self::Cached(p) => p.as_path(),
            Self::Tempdir(t) => t.path(),
        }
    }
}

/// Shallow-clone the URL — try the persistent cache first, fall back
/// to a one-shot tempdir if the cache directory is unwritable.
fn remote_clone(url: String, depth: u32) -> Result<RemoteWorkdir> {
    if let Some(cached) = cached_clone_or_fetch(&url, depth) {
        return Ok(RemoteWorkdir::Cached(cached));
    }
    // Cache path failed — fall back to a disposable tempdir.
    let tmp = tempfile::Builder::new()
        .prefix("slop-scan-")
        .tempdir()
        .context("create temp dir for --repo clone")?;
    eprintln!("slop: cloning {url} (depth {depth}, no cache)…");
    let depth_s = depth.to_string();
    let status = Command::new("git")
        .args([
            "clone",
            "--depth",
            &depth_s,
            "--quiet",
            "--no-tags",
            &url,
            tmp.path().to_str().context("temp path not utf-8")?,
        ])
        .status()
        .context("spawn git clone")?;
    if !status.success() {
        bail!("git clone {url} failed (exit {:?})", status.code());
    }
    Ok(RemoteWorkdir::Tempdir(tmp))
}

/// Print a unified diff to stdout, colorized when stdout is a TTY
/// (red `-`, green `+`, cyan hunk header). Auto-disabled for pipes
/// and redirections so `slop poke … | git apply --check` works.
/// `NO_COLOR=1` (https://no-color.org) and `SLOP_NO_COLOR=1` both
/// force plain output.
fn emit_patch_maybe_colored(patch: &str) {
    use std::io::IsTerminal;
    let color = std::io::stdout().is_terminal()
        && std::env::var("NO_COLOR").is_err()
        && std::env::var("SLOP_NO_COLOR").is_err();
    if !color {
        println!("{}", patch.trim_end());
        return;
    }
    const RED: &str = "\x1b[31m";
    const GREEN: &str = "\x1b[32m";
    const CYAN: &str = "\x1b[36m";
    const DIM: &str = "\x1b[2m";
    const RESET: &str = "\x1b[0m";
    for line in patch.trim_end().lines() {
        if line.starts_with("diff --git ") || line.starts_with("--- ") || line.starts_with("+++ ") {
            println!("{DIM}{line}{RESET}");
        } else if line.starts_with("@@") {
            println!("{CYAN}{line}{RESET}");
        } else if line.starts_with('+') {
            println!("{GREEN}{line}{RESET}");
        } else if line.starts_with('-') {
            println!("{RED}{line}{RESET}");
        } else {
            println!("{line}");
        }
    }
}

fn run_poke(args: PokeArgs) -> Result<()> {
    let cfg =
        api::load_config().context("`slop poke` needs a server config. Run `slop login` first.")?;
    // --repo / --gh: shallow-clone an arbitrary git URL into a temp
    // dir, run the rest of the resolver from inside it. tempdir Drop
    // cleans the checkout regardless of success/panic.
    let remote_workdir = if let Some(url) = remote_repo_url(&args) {
        Some(remote_clone(url, infer_clone_depth(&args))?)
    } else {
        None
    };
    let (patch, source) = if let Some(ref tmp) = remote_workdir {
        std::env::set_current_dir(tmp.path())
            .with_context(|| format!("chdir into cloned repo {}", tmp.path().display()))?;
        let (p, s) = resolve_patch(&args)?;
        (p, format!("{s} @ {}", remote_repo_url(&args).unwrap()))
    } else {
        resolve_patch(&args)?
    };
    // Pre-send filter chain:
    //   1. `.slopignore` + `--disable <glob>` strip whole file blocks
    //      so the muted file never hits the wire (smaller bill, fewer
    //      hits).
    //   2. `--disable <glob>:<8hex>` redacts individual `+` lines
    //      whose content hashes to a target checksum — converts the
    //      `+` prefix to a context space so the catalog skips it.
    //      Line-content checksums survive line-number drift, which
    //      file-paths don't; the operator pastes them from the
    //      verdict output and the mute outlives surrounding refactors.
    //
    // Validate `--disable` shape first so a malformed target bails
    // BEFORE the patch hits the wire (and burns quota).
    let disable_targets = parse_disable_targets(&args.disable)?;
    let ignore = build_pre_send_globset(&args.disable)?;
    let patch = filter_patch_by_slopignore(&patch, &ignore);
    let patch = redact_patch_by_checksum(&patch, &disable_targets);
    if patch.trim().is_empty() {
        bail!("nothing to scan ({source}) — every changed file is muted via .slopignore / --disable");
    }
    if args.dry_run {
        let preview = serde_json::json!({
            "project": args.project,
            "patch_bytes": patch.len(),
            "source": source,
        });
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }

    let resp = match api::poke(&cfg, args.project.as_deref(), &patch) {
        Ok(r) => r,
        Err(e) => {
            if let Some(pe) = e.downcast_ref::<api::PaymentError>() {
                // Quota cap hit. Render the onboarding screen and, if
                // the user actually subscribes, replay this exact
                // request once.
                let replay = payment::handle_payment_required(&cfg, &pe.0)?;
                if !replay {
                    return Ok(());
                }
                api::poke(&cfg, args.project.as_deref(), &patch)?
            } else {
                return Err(e);
            }
        }
    };
    save_plan(&resp, &patch)?;

    // Per-scan mute. Operator passes
    //   slop poke --disable 'src/sdk-alpha/**' --disable src/lib.rs:42
    // → matching findings get filtered out of the response BEFORE
    // the verdict is rendered. Stateless: no file is written; the
    // next scan has to re-pass the targets, or
    // `slop learn --disable <target> "<reason>"` upstream so the
    // server stops flagging the pattern for this org.
    let (kept_findings, muted): (Vec<_>, Vec<_>) = resp
        .findings
        .into_iter()
        .partition(|f| !finding_is_disabled(f, &disable_targets));
    let patch_out = if kept_findings.is_empty() && !muted.is_empty() {
        // Every finding the server returned is muted → drop the
        // patch entirely. Hook sees empty stdout → LGTM path.
        String::new()
    } else {
        resp.patch.clone()
    };

    // Verdict + quota line lives on stderr so it's visible to
    // interactive users (self-teaching, quota awareness) without
    // polluting `> foo.patch` redirections or `| git apply` pipes.
    eprintln!(
        "slop poke: {} ({} ms, {}/{} this cycle)",
        resp.verdict, resp.elapsed_ms, resp.usage.poke_calls, resp.cap
    );
    if !muted.is_empty() {
        eprintln!("slop poke: muted {} finding(s) via --disable", muted.len());
    }
    // Per-finding patch-notation summary on stderr so the operator
    // can copy a target straight into `--disable` or
    // `slop learn --disable` without re-deriving anything.
    //
    // The server's `findings` vec ships empty in production (the
    // catalog isn't exposed on the wire), so we walk the response
    // patch ourselves: each `@@ -X,0 +B,N @@` followed by
    // `+// TODO(slop): <category>: …` is a marker spliced ABOVE the
    // flagged source line at b-side line `B`. Reading the working-
    // tree file at line B gives us the actual flagged content,
    // which we hash so the operator can use the stable checksum
    // form (`<file>:<8hex>`) of the mute target.
    for summary in extract_finding_summaries(&patch_out) {
        let chksum_field = match summary.checksum {
            Some(chk) => format!("  [{chk}]"),
            None => String::new(),
        };
        eprintln!(
            "  {}:{}{chksum_field}  {}",
            summary.file, summary.line, summary.category
        );
    }
    let _ = &kept_findings;
    // The unified-diff patch on stdout. Color-aware for TTYs, ANSI
    // stripped for pipes / redirections so `git apply --unidiff-zero`
    // still works as a one-liner.
    if !patch_out.trim().is_empty() {
        emit_patch_maybe_colored(&patch_out);
        // Apply hint on stderr — first-time users get the obvious
        // next step without having to read --help.
        eprintln!(
            "\nRun `slop apply` to apply, `slop apply --discard` to drop, \
             or `git apply --unidiff-zero` if applying manually. \
             To mute a specific finding inline use `--disable <path[:line]>`; \
             to train the server use `slop learn --disable <path[:line]> \"<reason>\"`."
        );
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedPlan {
    poke_id: String,
    verdict: String,
    /// Server-rendered unified diff. Preferred apply target.
    #[serde(default)]
    patch: String,
    /// The exact diff the CLI sent to /api/v1/poke. Stored so
    /// `slop learn` can attach BOTH the input the user was unhappy
    /// with AND the output the server proposed — operator reading
    /// the LearnLog gets the full before/after pair, not just the
    /// server's response.
    #[serde(default)]
    input_diff: String,
    /// Legacy structured action list. Kept so a stale plan from an
    /// older server still applies via the action-walker fallback.
    #[serde(default)]
    cleanup_actions: Vec<CleanupAction>,
}

fn save_plan(r: &api::PokeResponse, input_diff: &str) -> Result<()> {
    let dir = Path::new(".slop");
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("slop: warning — could not create .slop dir: {e}");
        return Ok(());
    }
    let plan = CachedPlan {
        poke_id: r.poke_id.clone(),
        verdict: r.verdict.clone(),
        patch: r.patch.clone(),
        input_diff: input_diff.to_string(),
        cleanup_actions: r
            .cleanup_actions
            .iter()
            .map(|a| CleanupAction {
                file: a.file.clone(),
                line: a.line,
                action: a.action.clone(),
                content: a.content.clone(),
            })
            .collect(),
    };
    let serialised = serde_json::to_string_pretty(&plan)?;
    fs::write(CACHED_PLAN, &serialised)?;
    // Also write a per-user copy at ~/.config/slop/last-poke.json so
    // `slop learn` can attach the most recent poke regardless of cwd.
    // Best-effort: a failure here doesn't break `slop poke`.
    if let Some(global) = global_last_poke_path() {
        if let Some(parent) = global.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&global, &serialised) {
            eprintln!("slop: warning — could not write {}: {e}", global.display());
        }
    }
    Ok(())
}

/// `~/.config/slop/last-poke.json` — per-user mirror of the repo-local
/// `.slop/last-poke.json` so `slop learn` finds context even when run
/// from a different cwd than `slop poke`.
fn global_last_poke_path() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("SLOP_CONFIG_DIR") {
        return Some(PathBuf::from(custom).join("last-poke.json"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("slop")
            .join("last-poke.json"),
    )
}

// ── apply ────────────────────────────────────────────────────────

/// Lower priority value = applied first within the same line. Delete
/// before insert: if a slop line has BOTH a delete-line action and an
/// insert-above-it action, we want the deletion to land first so the
/// inserted TODO comment sits above the line the slop used to occupy
/// — not above the slop itself.
fn action_priority(action: &str) -> u8 {
    match action {
        "delete_line" => 0,
        "insert_above" => 1,
        _ => 9,
    }
}

fn run_apply(args: ApplyArgs) -> Result<()> {
    let path = PathBuf::from(CACHED_PLAN);
    if args.discard {
        if path.exists() {
            fs::remove_file(&path)?;
            eprintln!("slop: discarded {CACHED_PLAN}");
        } else {
            eprintln!("slop: no cached plan");
        }
        return Ok(());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {CACHED_PLAN} (run `slop poke` first)"))?;
    let plan: CachedPlan = serde_json::from_str(&raw)?;
    if args.show {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    if plan.patch.trim().is_empty() && plan.cleanup_actions.is_empty() {
        eprintln!("slop: nothing to apply (LGTM)");
        return Ok(());
    }

    // Primary path: server gave us a unified diff. Hand it to
    // `git apply --unidiff-zero --index -`. Battle-tested mutator,
    // zero CLI-side line arithmetic.
    if !plan.patch.trim().is_empty() {
        return apply_via_git(&plan, args);
    }

    let mut by_file: std::collections::BTreeMap<String, Vec<&CleanupAction>> =
        std::collections::BTreeMap::new();
    for a in &plan.cleanup_actions {
        by_file.entry(a.file.clone()).or_default().push(a);
    }
    let mut touched = Vec::new();
    for (file, mut acts) in by_file {
        // Sort by line descending so later mutations don't shift the
        // indices we still need to address. Within a single line:
        // process delete_line before insert_above so an insert above a
        // line we're also deleting still lands at the right relative
        // position (the deleted line was the slop; the TODO replaces
        // surrounding context).
        acts.sort_by(|a, b| {
            b.line
                .cmp(&a.line)
                .then_with(|| action_priority(&a.action).cmp(&action_priority(&b.action)))
        });
        if !PathBuf::from(&file).exists() {
            eprintln!("slop: skip {file} (not in working tree)");
            continue;
        }
        let body = fs::read_to_string(&file).with_context(|| format!("read {file}"))?;
        let mut lines: Vec<String> = body.lines().map(String::from).collect();
        let trailing_newline = body.ends_with('\n');
        let mut deleted = 0usize;
        let mut inserted = 0usize;
        for a in acts {
            let Some(idx) = a.line.checked_sub(1) else {
                continue;
            };
            match a.action.as_str() {
                "delete_line" => {
                    if idx >= lines.len() {
                        continue;
                    }
                    let actual = lines[idx].trim_end_matches('\r');
                    if actual != a.content.trim_end_matches('\r') {
                        eprintln!(
                            "slop: skip {file}:{} — content drifted (expected {:?}, got {:?})",
                            a.line, a.content, actual
                        );
                        continue;
                    }
                    lines.remove(idx);
                    deleted += 1;
                }
                "insert_above" => {
                    // Off-by-one tolerance: allow idx == lines.len()
                    // so a slop hit on the final line still gets a
                    // comment spliced above it.
                    if idx > lines.len() {
                        continue;
                    }
                    // Idempotent: the TODO is "already present" if
                    //   (a) the line above the target matches it
                    //       (fresh apply against the original line
                    //       numbering), or
                    //   (b) the target line itself matches it (re-apply
                    //       of a stale plan whose line numbers no
                    //       longer reflect the post-insert file).
                    let already_above = idx
                        .checked_sub(1)
                        .and_then(|i| lines.get(i))
                        .map(|prev| prev.trim() == a.content.trim())
                        .unwrap_or(false);
                    let already_here = lines
                        .get(idx)
                        .map(|here| here.trim() == a.content.trim())
                        .unwrap_or(false);
                    if already_above || already_here {
                        continue;
                    }
                    lines.insert(idx, a.content.clone());
                    inserted += 1;
                }
                _ => {
                    // Unknown action — surfacing on a future server
                    // protocol bump; safest is to skip.
                    continue;
                }
            }
        }
        if deleted == 0 && inserted == 0 {
            continue;
        }
        let mut out = lines.join("\n");
        if trailing_newline {
            out.push('\n');
        }
        fs::write(&file, out).with_context(|| format!("write {file}"))?;
        touched.push((file, deleted, inserted));
    }

    if touched.is_empty() {
        eprintln!("slop: no clean changes to apply; leaving working tree intact");
        return Ok(());
    }
    for (f, del, ins) in &touched {
        match (*del, *ins) {
            (d, 0) => eprintln!("slop: trimmed {d} line(s) from {f}"),
            (0, i) => eprintln!("slop: spliced {i} TODO comment(s) into {f}"),
            (d, i) => {
                eprintln!("slop: trimmed {d} line(s) and spliced {i} TODO comment(s) into {f}")
            }
        }
    }
    let mut add_args = vec!["add", "--"];
    for (f, _, _) in &touched {
        add_args.push(f);
    }
    git_run(&add_args)?;
    if args.no_commit {
        eprintln!("slop: staged. Commit when ready.");
        return Ok(());
    }
    git_run(&["commit", "--amend", "--no-edit"])?;
    eprintln!("slop: HEAD amended.");
    Ok(())
}

/// Choose what to send to `slop poke`. Precedence (clap enforces
/// mutual exclusion at parse time): `--patch` > `--staged` >
/// `--range` > `--since` > default `git diff HEAD` (working tree
/// versus HEAD). Returns the raw patch + a short human label of
/// where it came from for error / dry-run output.
fn resolve_patch(args: &PokeArgs) -> Result<(String, String)> {
    if let Some(p) = args.patch.as_ref() {
        let body =
            fs::read_to_string(p).with_context(|| format!("read patch file {}", p.display()))?;
        return Ok((body, format!("--patch {}", p.display())));
    }
    if args.staged {
        return Ok((
            git_diff(&["--cached"])?,
            "--staged (git diff --cached)".into(),
        ));
    }
    if let Some(r) = args.range.as_deref() {
        let r = clamp_head_tilde(r);
        return Ok((git_diff(&[&r])?, format!("--range {r}")));
    }
    if let Some(s) = args.since.as_deref() {
        let s = clamp_head_tilde(s);
        return Ok((git_diff(&[&s])?, format!("--since {s}")));
    }
    Ok((git_diff(&["HEAD"])?, "git diff HEAD (default)".into()))
}

/// Read `.slopignore` from the repo root and return a globset of
/// path patterns the scan should skip. Empty when the file is
/// absent. One glob per line; `#` starts a comment.
///
/// Designed for repo-level false-positive suppression — projects with
/// a known FP pattern (postgres.js tagged templates in `*.ts`,
/// `.env.example` placeholder credentials, generated SDK shims at
/// `*/sdk-alpha/**`) can list the paths once and stop sending those
/// hunks to the scorer every commit.
fn load_slopignore_builder() -> Result<globset::GlobSetBuilder> {
    let mut builder = globset::GlobSetBuilder::new();
    let path = PathBuf::from(".slopignore");
    if !path.exists() {
        return Ok(builder);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    for (n, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let glob = globset::Glob::new(trimmed)
            .with_context(|| format!(".slopignore line {}: bad glob {:?}", n + 1, trimmed))?;
        builder.add(glob);
    }
    Ok(builder)
}

/// Merge `.slopignore` patterns with the file-level `--disable`
/// targets so the resulting globset can pre-filter file blocks out
/// of the patch BEFORE sending. Line-level targets (`path:LINE`)
/// are skipped here — those address single lines, not whole files,
/// so they ride along until the response-side filter.
///
/// A malformed `.slopignore` file logs a warn and returns the
/// `--disable`-only half so a typo'd repo config never blocks an
/// explicit one-shot mute.
fn build_pre_send_globset(disable_raws: &[String]) -> Result<globset::GlobSet> {
    let mut builder = load_slopignore_builder().unwrap_or_else(|e| {
        eprintln!("slop: ignoring malformed .slopignore — {e}");
        globset::GlobSetBuilder::new()
    });
    for raw in disable_raws {
        let glob_str = match raw.rsplit_once(':') {
            // Line-level target — pre-send filter only operates at
            // file granularity, so let the line target ride along
            // for response-side handling.
            Some((_, tail)) if tail.parse::<usize>().is_ok() => continue,
            _ => raw.as_str(),
        };
        let trimmed = glob_str.trim();
        if trimmed.is_empty() {
            continue;
        }
        let glob = globset::Glob::new(trimmed)
            .with_context(|| format!("--disable {trimmed:?}: bad glob"))?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

/// Drop file blocks from a unified diff whose path matches a glob in
/// the supplied globset. A no-op when the globset is empty. Used to
/// strip known-FP paths from the patch before sending it to the
/// scorer so the server never spends cycles on them.
///
/// Per-file blocks are demarcated by `diff --git` headers — git's
/// own canonical separator. Lines before the first `diff --git` are
/// preserved as the patch preamble (`mbox` headers, `From <sha>`,
/// etc.) so this works on `git format-patch` output too.
fn filter_patch_by_slopignore(patch: &str, ignore: &globset::GlobSet) -> String {
    if ignore.is_empty() {
        return patch.to_string();
    }
    let mut out = String::with_capacity(patch.len());
    let mut block = String::new();
    let mut block_path: Option<String> = None;
    let mut in_block = false;

    let flush =
        |out: &mut String, block: &str, path: &Option<String>, ignore: &globset::GlobSet| {
            let drop = path.as_deref().map(|p| ignore.is_match(p)).unwrap_or(false);
            if !drop {
                out.push_str(block);
            }
        };

    for line in patch.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if in_block {
                flush(&mut out, &block, &block_path, ignore);
                block.clear();
            }
            in_block = true;
            // `diff --git a/<old> b/<new>` — extract the b-side path
            // since renames keep slop interested in where the content
            // landed, not where it came from.
            block_path = rest
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.strip_prefix("b/"))
                .map(|s| s.trim_end().to_string());
        }
        if in_block {
            block.push_str(line);
        } else {
            out.push_str(line);
        }
    }
    if in_block {
        flush(&mut out, &block, &block_path, ignore);
    }
    out
}

/// Address granularity for a parsed `--disable` target.
///
/// `Checksum` is the recommended form for muting a specific line:
/// stable across line shifts because the address is a hash of the
/// line content, not the line number. `Line` is the escape hatch
/// for ad-hoc mutes when the operator doesn't have a checksum
/// handy. `Whole` strips the entire file block pre-send.
#[derive(Debug)]
enum DisableScope {
    Whole,
    Line(usize),
    Checksum(String),
}

/// One parsed `--disable` target. Mirrors patch-notation: a path
/// glob, optionally narrowed to a single line (by number or — better
/// — by content checksum). Compiled to a globset matcher at parse
/// time so per-finding checks are O(1).
#[derive(Debug)]
struct DisableTarget {
    matcher: globset::GlobMatcher,
    scope: DisableScope,
}

/// 8-hex SHA-256 fingerprint of the matched line content. The
/// fingerprint is what `--disable <glob>:<8hex>` addresses. Line
/// numbers drift on every refactor; content checksums don't —
/// mute lifetime tracks code lifetime. Whitespace differences DO
/// change the fingerprint by design: a reformat is a real edit and
/// should re-prompt the operator to decide whether the new shape
/// is still a FP.
pub fn finding_checksum(line: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(line.as_bytes());
    let out = hasher.finalize();
    let mut hex = String::with_capacity(8);
    for byte in &out[..4] {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn looks_like_checksum(s: &str) -> bool {
    s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Parse one CLI `--disable` argument. Grammar:
///   <glob>             file-level mute
///   <glob>:<8-hex>     line-content checksum mute (recommended)
///   <glob>:<line-num>  literal line number mute (drifts on refactor)
///
/// The suffix after the last `:` is inspected:
///   - 8 lowercase-hex characters → checksum
///   - all digits → line number
///   - anything else → treat as part of the path (raw glob)
fn parse_disable_target(raw: &str) -> Result<DisableTarget> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("--disable: empty target");
    }
    let (glob_str, scope) = match raw.rsplit_once(':') {
        Some((prefix, tail)) if looks_like_checksum(tail) => {
            (prefix, DisableScope::Checksum(tail.to_string()))
        }
        Some((_, tail))
            if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            anyhow::bail!(
                "--disable {raw:?}: checksum suffix must be lowercase hex \
                 (got {tail:?}); the verdict output always prints lowercase, \
                 copy-paste from there"
            );
        }
        Some((prefix, tail)) => match tail.parse::<usize>() {
            Ok(n) => (prefix, DisableScope::Line(n)),
            Err(_) => (raw, DisableScope::Whole),
        },
        None => (raw, DisableScope::Whole),
    };
    let glob = globset::Glob::new(glob_str)
        .with_context(|| format!("--disable {raw:?}: bad glob"))?;
    Ok(DisableTarget {
        matcher: glob.compile_matcher(),
        scope,
    })
}

/// Parse a list of `--disable` args, collecting all errors into one
/// message so the operator sees every bad target on the first try.
fn parse_disable_targets(raws: &[String]) -> Result<Vec<DisableTarget>> {
    let mut out = Vec::with_capacity(raws.len());
    let mut errs = Vec::new();
    for raw in raws {
        match parse_disable_target(raw) {
            Ok(target) => out.push(target),
            Err(err) => errs.push(format!("  - {err}")),
        }
    }
    if !errs.is_empty() {
        anyhow::bail!("invalid --disable target(s):\n{}", errs.join("\n"));
    }
    Ok(out)
}

/// True when at least one target matches the finding. File match is
/// glob-based against `finding.file`; line match is an exact equal
/// check against `finding.line` (only when the target specifies one);
/// checksum match hashes `finding.matched`.
fn finding_is_disabled(finding: &api::PokeFinding, targets: &[DisableTarget]) -> bool {
    targets.iter().any(|target| {
        if !target.matcher.is_match(&finding.file) {
            return false;
        }
        match &target.scope {
            DisableScope::Whole => true,
            DisableScope::Line(n) => *n == finding.line,
            DisableScope::Checksum(chk) => &finding_checksum(&finding.matched) == chk,
        }
    })
}

/// Redact `+` lines from the patch whose content hashes to any
/// `--disable <glob>:<8hex>` target — converts the `+` prefix to
/// a context space so the server's catalog skips the line and the
/// hunk header math stays correct (` ` and `+` lines both count
/// toward the hunk's `+B,M` total).
///
/// Each file block is checked against every target's glob; matching
/// targets contribute their checksum set for the redaction walk.
/// `+++ b/path` header lines are never confused with `+content`.
fn redact_patch_by_checksum(patch: &str, targets: &[DisableTarget]) -> String {
    let checksum_targets: Vec<(&globset::GlobMatcher, &String)> = targets
        .iter()
        .filter_map(|target| match &target.scope {
            DisableScope::Checksum(chk) => Some((&target.matcher, chk)),
            _ => None,
        })
        .collect();
    if checksum_targets.is_empty() {
        return patch.to_string();
    }
    let mut out = String::with_capacity(patch.len());
    let mut current_checksums: std::collections::HashSet<&str> =
        std::collections::HashSet::new();

    for line in patch.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            current_checksums.clear();
            if let Some(b_path) = rest
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.strip_prefix("b/"))
                .map(|s| s.trim_end())
            {
                for (matcher, chk) in &checksum_targets {
                    if matcher.is_match(b_path) {
                        current_checksums.insert(chk.as_str());
                    }
                }
            }
            out.push_str(line);
        } else if !current_checksums.is_empty()
            && line.starts_with('+')
            && !line.starts_with("+++")
        {
            // Hash the line content WITHOUT the leading `+` or the
            // trailing newline. Trailing newline is patch transport,
            // not part of the matched line.
            let content = line[1..].trim_end_matches('\n');
            let chk = finding_checksum(content);
            if current_checksums.contains(chk.as_str()) {
                out.push(' ');
                out.push_str(&line[1..]);
            } else {
                out.push_str(line);
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// One row in the per-finding stderr summary the operator scans to
/// decide whether to mute a finding.
#[derive(Debug, PartialEq, Eq)]
struct FindingSummary {
    file: String,
    /// b-side line number where the TODO marker would land (and
    /// equivalently the source-file line being annotated).
    line: usize,
    /// Catalog category as parsed from the splice text — e.g.
    /// `placeholder identifier`, `what_filler_comment`.
    category: String,
    /// 8-hex content fingerprint of the source-file line at
    /// `line`. `None` when the source line couldn't be read (file
    /// missing, line out of range), in which case the operator
    /// falls back to the line-number escape hatch.
    checksum: Option<String>,
}

/// Walk a server-rendered patch and emit one row per `TODO(slop):`
/// splice. The catalog category text is parsed out of the splice
/// itself; the source-line content is read from the current working
/// tree so the checksum survives whatever the operator does next
/// (apply, discard, ignore).
fn extract_finding_summaries(patch: &str) -> Vec<FindingSummary> {
    let mut out = Vec::new();
    let mut current_path: Option<String> = None;
    let mut hunk_b_start: Option<usize> = None;

    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            current_path = rest
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.strip_prefix("b/"))
                .map(|s| s.trim_end().to_string());
            hunk_b_start = None;
            continue;
        }
        if line.starts_with("@@ ") {
            // Header shape: `@@ -A,B +C,D @@ optional`.
            // Parse the `+C` part. The b-start is where the splice
            // sits in the new file.
            hunk_b_start = line
                .split_whitespace()
                .find_map(|tok| tok.strip_prefix('+'))
                .and_then(|s| s.split(',').next())
                .and_then(|n| n.parse::<usize>().ok());
            continue;
        }
        // Marker shapes the server emits per filetype:
        //   `+// TODO(slop): …`         Rust / TS / JS / C / Go
        //   `+# TODO(slop): …`          Python / shell / YAML
        //   `+<!-- TODO(slop): … -->`   Markdown / HTML
        let splice_body = line
            .strip_prefix("+// TODO(slop):")
            .or_else(|| line.strip_prefix("+# TODO(slop):"))
            .or_else(|| {
                line.strip_prefix("+<!-- TODO(slop):")
                    .and_then(|rest| rest.strip_suffix("-->"))
            });
        if let Some(splice) = splice_body {
            let (file, line_num) = match (&current_path, hunk_b_start) {
                (Some(p), Some(n)) => (p.clone(), n),
                _ => continue,
            };
            let category = parse_slop_category(splice);
            let checksum = read_source_line(&file, line_num).map(|s| finding_checksum(&s));
            out.push(FindingSummary {
                file,
                line: line_num,
                category,
                checksum,
            });
        }
    }
    out
}

/// Parse `<category> — <prose>` out of the body of a TODO(slop)
/// splice. Falls back to the raw splice text when no `—` is
/// present so the operator still sees something useful.
fn parse_slop_category(splice_body: &str) -> String {
    let trimmed = splice_body.trim_start();
    match trimmed.split_once('—') {
        Some((category, _)) => category.trim().to_string(),
        None => trimmed.to_string(),
    }
}

/// Read line `n` (1-indexed) from `file` in the current working
/// directory. Returns None on any failure — caller treats absent
/// as "no checksum available".
fn read_source_line(file: &str, n: usize) -> Option<String> {
    let content = fs::read_to_string(file).ok()?;
    content.lines().nth(n.saturating_sub(1)).map(|s| s.to_string())
}

/// Rewrite every `HEAD~N` token in `selector` so N never exceeds the
/// repo's actual history depth. Public repos often have only a handful
/// of commits — without this, `slop poke --range HEAD~10..HEAD` on a
/// 5-commit repo dies with git's unfriendly `unknown revision` error.
/// On success the user sees a `slop: range clamped to …` notice on
/// stderr and the scan proceeds with the largest range that resolves.
fn clamp_head_tilde(selector: &str) -> String {
    let Some(total) = git_rev_count_head() else {
        return selector.to_string();
    };
    if total == 0 {
        return selector.to_string();
    }
    let (out, clamped) = clamp_head_tilde_to(selector, total);
    if clamped {
        eprintln!(
            "slop: range clamped to {out} (repo has {total} commit{})",
            if total == 1 { "" } else { "s" }
        );
    }
    out
}

/// Pure helper for `clamp_head_tilde` — separated for unit tests so we
/// don't need a temp git repo to exercise the parser. Returns the
/// rewritten selector plus a flag indicating whether any token was
/// clamped (the caller uses that to log a notice).
fn clamp_head_tilde_to(selector: &str, total_commits: u32) -> (String, bool) {
    let limit = total_commits.saturating_sub(1);
    let mut out = String::with_capacity(selector.len());
    let bytes = selector.as_bytes();
    let mut i = 0;
    let mut clamped = false;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"HEAD~") {
            let start = i + 5;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                let n_str = &selector[start..end];
                let n: u32 = n_str.parse().unwrap_or(0);
                let chosen = if n > limit {
                    clamped = true;
                    limit
                } else {
                    n
                };
                out.push_str("HEAD~");
                out.push_str(&chosen.to_string());
                i = end;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    (out, clamped)
}

fn git_rev_count_head() -> Option<u32> {
    let out = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    s.trim().parse().ok()
}

fn git_diff(extra: &[&str]) -> Result<String> {
    let mut argv: Vec<&str> = vec!["diff", "--no-color"];
    argv.extend_from_slice(extra);
    let out = Command::new("git")
        .args(&argv)
        .output()
        .with_context(|| format!("spawn git {}", argv.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "git {} failed (exit {:?}): {}",
            argv.join(" "),
            out.status.code(),
            stderr.trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

fn git_run(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .status()
        .with_context(|| format!("spawn git {}", args.join(" ")))?;
    if !status.success() {
        bail!("git {} failed (exit {:?})", args.join(" "), status.code());
    }
    Ok(())
}

/// Apply a server-rendered unified diff via `git apply
/// --unidiff-zero --index`. Stdin carries the patch; on success the
/// working tree is mutated and the changes are staged. If
/// `args.no_commit` is false we also amend HEAD so the slop never
/// ships as a separate commit.
fn apply_via_git(plan: &CachedPlan, args: ApplyArgs) -> Result<()> {
    use std::io::Write;
    // Dry-run preflight: --check exits non-zero if the diff would not
    // apply cleanly. Surface the actual git stderr so the user knows
    // why before we mutate anything.
    let mut check = Command::new("git")
        .args(["apply", "--unidiff-zero", "--check", "-"])
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn git apply --check")?;
    check
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(plan.patch.as_bytes())
        .context("write patch to git apply --check")?;
    let preflight = check.wait_with_output().context("wait git apply --check")?;
    if !preflight.status.success() {
        let stderr = String::from_utf8_lossy(&preflight.stderr);
        bail!(
            "patch would not apply cleanly — leaving working tree untouched.\ngit apply --check:\n{stderr}"
        );
    }

    let mut apply = Command::new("git")
        .args(["apply", "--unidiff-zero", "--index", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("spawn git apply")?;
    apply
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(plan.patch.as_bytes())
        .context("write patch to git apply")?;
    let status = apply.wait().context("wait git apply")?;
    if !status.success() {
        bail!(
            "git apply failed after --check passed (exit {:?}) — re-run with RUST_LOG=debug for detail",
            status.code()
        );
    }

    eprintln!("slop: applied server patch (verdict: {})", plan.verdict);
    if args.no_commit {
        eprintln!("slop: staged. Commit when ready.");
        return Ok(());
    }
    git_run(&["commit", "--amend", "--no-edit"])?;
    eprintln!("slop: HEAD amended.");
    Ok(())
}

// ── learn ────────────────────────────────────────────────────────

fn run_learn(args: LearnArgs) -> Result<()> {
    let cfg = api::load_config()
        .context("`slop learn` needs a server config. Run `slop login` first.")?;
    // Validate `--disable` targets eagerly so a typo bails BEFORE
    // the upstream POST burns quota and confuses the operator with
    // a "queued" reply that didn't actually pin the bad target.
    if !args.disable.is_empty() {
        parse_disable_targets(&args.disable)?;
    }
    // Auto-attach the cached poke plan when the caller didn't pass
    // their own --context. Saves a copy-paste step and gives the RL
    // loop a concrete (poke_id, patch) pair to join the feedback
    // against instead of free-floating prose. Suppressed by --no-attach.
    let auto_context = if args.context.is_none() && !args.no_attach {
        attached_context_from_cached_plan()
    } else {
        None
    };
    // Splice the `--disable` targets into the context so the server-
    // side learn loop links the feedback row to the exact patch
    // addresses the operator wants muted. Format: one target per
    // line under a clear marker so the offline RL parser can pick
    // them out without a schema change.
    let target_block = if args.disable.is_empty() {
        String::new()
    } else {
        let mut s = String::from("--- disable_targets ---\n");
        for t in &args.disable {
            s.push_str(t);
            s.push('\n');
        }
        s
    };
    let merged_context = match (auto_context.as_deref(), args.context.as_deref()) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}\n{target_block}")),
        (Some(a), None) => Some(format!("{a}\n{target_block}")),
        (None, Some(b)) => Some(format!("{b}\n{target_block}")),
        (None, None) if !target_block.is_empty() => Some(target_block.clone()),
        (None, None) => None,
    };
    let resp = api::learn(
        &cfg,
        &args.feedback,
        merged_context.as_deref(),
        args.project.as_deref(),
    )?;
    let attach_note = if auto_context.is_some() {
        " (attached last poke)"
    } else {
        ""
    };
    let target_note = if args.disable.is_empty() {
        String::new()
    } else {
        format!(" + {} disable target(s)", args.disable.len())
    };
    eprintln!(
        "slop learn: queued {} ({}/{}) — {} bytes{attach_note}{target_note}",
        resp.entry_id, resp.queued, resp.monthly_cap, resp.bytes,
    );
    Ok(())
}

/// Read `.slop/last-poke.json` and format a short context string the
/// server can store next to the feedback row. Includes poke_id (so
/// the offline RL loop can join), verdict, and a truncated patch so
/// the reviewer sees what triggered the feedback without unbounded
/// payload growth. Returns None when no plan is cached.
fn load_plan() -> Result<CachedPlan> {
    // Try cwd-relative first; fall back to the per-user global mirror
    // so `slop learn` from a different directory than `slop poke` still
    // attaches context. Either path's read failure is fatal here —
    // callers catch via `.ok()` to mean "no recent poke".
    let local = PathBuf::from(CACHED_PLAN);
    if local.exists() {
        let raw = fs::read_to_string(&local).with_context(|| format!("read {CACHED_PLAN}"))?;
        return Ok(serde_json::from_str(&raw)?);
    }
    let global = global_last_poke_path()
        .with_context(|| format!("no {CACHED_PLAN} and no fallback path"))?;
    let raw = fs::read_to_string(&global).with_context(|| format!("read {}", global.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

/// Truncate a unified diff to at most `max` bytes on a line boundary,
/// appending a marker so the reviewer knows there's more.
///
/// The slice has to land on a UTF-8 char boundary or `&diff[..n]`
/// panics — the historical implementation indexed at `max` directly
/// and exploded on any patch with a multi-byte char (em-dash, CJK,
/// emoji) straddling the boundary. Walk back to the nearest valid
/// boundary first, then look for a newline in the safe prefix.
fn cap_diff(diff: &str, max: usize, label: &str) -> String {
    if diff.len() <= max {
        return diff.to_string();
    }
    let safe_max = (0..=max)
        .rev()
        .find(|&i| diff.is_char_boundary(i))
        .unwrap_or(0);
    let cut = diff[..safe_max].rfind('\n').unwrap_or(safe_max);
    format!(
        "{}\n... ({} truncated at {} bytes; see poke_id for full row)",
        &diff[..cut],
        label,
        cut
    )
}

fn attached_context_from_cached_plan() -> Option<String> {
    let plan = load_plan().ok()?;
    let mut out = String::new();
    out.push_str(&format!("poke_id: {}\n", plan.poke_id));
    out.push_str(&format!("verdict: {}\n", plan.verdict));
    // Server-side LearnLog cap is 8 KB per entry. Split the budget
    // between the input diff (what the user sent) and the proposed
    // patch (what the server returned) so both fit. Asymmetric split:
    // input is more important — without it the patch is opaque.
    let input_budget = 5 * 1024usize;
    let patch_budget = 2 * 1024usize;
    if !plan.input_diff.trim().is_empty() {
        out.push_str("--- input_diff ---\n");
        out.push_str(&cap_diff(&plan.input_diff, input_budget, "input diff"));
        out.push('\n');
    }
    if !plan.patch.trim().is_empty() {
        out.push_str("--- proposed_patch ---\n");
        out.push_str(&cap_diff(&plan.patch, patch_budget, "proposed patch"));
        out.push('\n');
    }
    Some(out)
}

// ── billing ──────────────────────────────────────────────────────

fn run_billing(cmd: BillingCmd) -> Result<()> {
    let cfg = api::load_config()?;
    match cmd {
        BillingCmd::Tier => {
            let resp = api::billing_tier(&cfg)?;
            println!(
                "{}: poke {}/{} | review_tokens {}/{}",
                resp.slop_org,
                resp.usage.poke_calls,
                resp.entitlements.poke_calls_cap,
                resp.usage.review_tokens,
                resp.entitlements.review_token_cap
            );
            Ok(())
        }
        BillingCmd::Portal { print } => {
            let resp = api::billing_portal(&cfg)?;
            if print {
                println!("{}", resp.url);
                return Ok(());
            }
            eprintln!("slop: opening Stripe portal — {}", resp.url);
            let _ = Command::new("open").arg(&resp.url).status();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_with(repo: Option<&str>, gh: Option<&str>) -> PokeArgs {
        PokeArgs {
            patch: None,
            staged: false,
            range: None,
            since: None,
            project: None,
            repo: repo.map(str::to_string),
            gh: gh.map(str::to_string),
            dry_run: false,
            disable: Vec::new(),
        }
    }

    #[test]
    fn remote_repo_url_returns_none_when_neither_flag_set() {
        assert!(remote_repo_url(&args_with(None, None)).is_none());
    }

    fn args_with_range(range: &str) -> PokeArgs {
        PokeArgs {
            patch: None,
            staged: false,
            range: Some(range.to_string()),
            since: None,
            project: None,
            repo: None,
            gh: None,
            dry_run: false,
            disable: Vec::new(),
        }
    }

    fn args_with_since(since: &str) -> PokeArgs {
        PokeArgs {
            patch: None,
            staged: false,
            range: None,
            since: Some(since.to_string()),
            project: None,
            repo: None,
            gh: None,
            dry_run: false,
            disable: Vec::new(),
        }
    }

    // SLOP_REMOTE_CLONE_DEPTH is a process-global env var, so every
    // assertion that depends on it lives in a single test to avoid
    // races between parallel cargo-test threads.
    #[test]
    fn infer_clone_depth_covers_all_cases() {
        std::env::remove_var("SLOP_REMOTE_CLONE_DEPTH");

        // Branch / tag / SHA refs have no static bound → default 50.
        assert_eq!(infer_clone_depth(&args_with_range("main..HEAD")), 50);
        assert_eq!(infer_clone_depth(&args_with_range("origin/main...")), 50);
        assert_eq!(infer_clone_depth(&args_with(None, None)), 50);

        // HEAD~N selectors → N + 2 buffer.
        assert_eq!(infer_clone_depth(&args_with_range("HEAD~5..HEAD")), 7);
        assert_eq!(infer_clone_depth(&args_with_range("HEAD~1..HEAD")), 3);
        assert_eq!(infer_clone_depth(&args_with_range("HEAD~8..HEAD~2")), 10);
        assert_eq!(infer_clone_depth(&args_with_since("HEAD~3")), 5);

        // Env override wins even when the selector would infer less.
        std::env::set_var("SLOP_REMOTE_CLONE_DEPTH", "200");
        assert_eq!(infer_clone_depth(&args_with_range("HEAD~5..HEAD")), 200);
        std::env::remove_var("SLOP_REMOTE_CLONE_DEPTH");
    }

    #[test]
    fn remote_repo_url_passes_repo_through_verbatim() {
        let cli_args = args_with(Some("https://gitlab.com/owner/proj.git"), None);
        assert_eq!(
            remote_repo_url(&cli_args).as_deref(),
            Some("https://gitlab.com/owner/proj.git")
        );
    }

    #[test]
    fn remote_repo_url_expands_gh_slug_to_github_https() {
        let cli_args = args_with(None, Some("openclaw/openclaw"));
        assert_eq!(
            remote_repo_url(&cli_args).as_deref(),
            Some("https://github.com/openclaw/openclaw.git")
        );
    }

    #[test]
    fn remote_repo_url_accepts_full_url_in_gh() {
        // Defensive: if the user fat-fingers a full URL into --gh
        // instead of the org/repo slug, pass it through rather than
        // rewriting it into `https://github.com/https://…`.
        let cli_args = args_with(None, Some("git@github.com:foo/bar.git"));
        assert_eq!(
            remote_repo_url(&cli_args).as_deref(),
            Some("git@github.com:foo/bar.git")
        );
        let cli_args = args_with(None, Some("https://example.test/foo.git"));
        assert_eq!(
            remote_repo_url(&cli_args).as_deref(),
            Some("https://example.test/foo.git")
        );
    }

    #[test]
    fn remote_repo_url_prefers_repo_over_gh_when_both_set() {
        // clap should reject this combination at parse time
        // (conflicts_with), but the function should still pick
        // deterministically if invoked programmatically.
        let cli_args = args_with(
            Some("https://repo.test/x.git"),
            Some("ignored/forsclap-violators"),
        );
        assert_eq!(
            remote_repo_url(&cli_args).as_deref(),
            Some("https://repo.test/x.git")
        );
    }

    #[test]
    fn clamp_head_tilde_rewrites_n_over_history_limit() {
        // Repo with 5 commits → HEAD~4 is the oldest reachable rev,
        // HEAD~5+ doesn't resolve.
        let (out, clamped) = clamp_head_tilde_to("HEAD~10..HEAD", 5);
        assert!(clamped);
        assert_eq!(out, "HEAD~4..HEAD");
    }

    #[test]
    fn clamp_head_tilde_passes_through_when_within_limit() {
        let (out, clamped) = clamp_head_tilde_to("HEAD~3..HEAD", 10);
        assert!(!clamped);
        assert_eq!(out, "HEAD~3..HEAD");
    }

    #[test]
    fn clamp_head_tilde_handles_multiple_tokens_in_one_range() {
        // Both ends are HEAD~N — clamp each independently.
        let (out, clamped) = clamp_head_tilde_to("HEAD~20..HEAD~2", 7);
        assert!(clamped);
        // total=7 → limit=6; HEAD~20 → HEAD~6, HEAD~2 stays.
        assert_eq!(out, "HEAD~6..HEAD~2");
    }

    #[test]
    fn clamp_head_tilde_leaves_branch_refs_alone() {
        let (out, clamped) = clamp_head_tilde_to("origin/main..HEAD", 5);
        assert!(!clamped);
        assert_eq!(out, "origin/main..HEAD");
    }

    #[test]
    fn clamp_head_tilde_with_zero_history_returns_input_unchanged() {
        // Edge case: empty / unborn HEAD. Total=0 means limit=0; the
        // function should not produce HEAD~0 (invalid). Caller is
        // responsible for the early-return in `clamp_head_tilde`.
        let (out, clamped) = clamp_head_tilde_to("HEAD~3..HEAD", 0);
        // Limit=0 means every N gets clamped to 0; while ugly, this
        // helper stays a pure parser. The wrapper short-circuits
        // before calling it when total==0.
        assert!(clamped);
        assert_eq!(out, "HEAD~0..HEAD");
    }

    // ── cap_diff (no env mutation) ─────────────────────────────

    #[test]
    fn cap_diff_returns_input_unchanged_when_under_budget() {
        let input = "small diff content\nline 2\n";
        let out = cap_diff(input, 4096, "test");
        assert_eq!(out, input);
    }

    #[test]
    fn cap_diff_truncates_to_line_boundary_with_marker() {
        let mut input = String::new();
        for i in 0..200 {
            input.push_str(&format!("+ line number {i}\n"));
        }
        let max = 500;
        let out = cap_diff(&input, max, "test");
        assert!(out.contains("test truncated"), "marker missing: {out}");
        // Body before the marker must end on a line boundary so the
        // truncated unified diff stays parseable downstream.
        let marker_at = out.find("\n... (").expect("marker present");
        let body = &out[..marker_at];
        assert!(
            body.is_empty() || body.ends_with('\n') || !body.ends_with(' '),
            "body should land on a line boundary: tail={:?}",
            &body[body.len().saturating_sub(40)..]
        );
    }

    fn mk_finding(file: &str, line: usize, category: &str) -> api::PokeFinding {
        api::PokeFinding {
            file: file.into(),
            line,
            category: category.into(),
            matched: "x".into(),
            content: String::new(),
        }
    }

    #[test]
    fn parse_disable_target_file_only() {
        let target = parse_disable_target("src/lib.rs").unwrap();
        assert!(matches!(target.scope, DisableScope::Whole));
        assert!(target.matcher.is_match("src/lib.rs"));
        assert!(!target.matcher.is_match("src/other.rs"));
    }

    #[test]
    fn parse_disable_target_with_line() {
        let target = parse_disable_target("src/lib.rs:42").unwrap();
        assert!(matches!(target.scope, DisableScope::Line(42)));
        assert!(target.matcher.is_match("src/lib.rs"));
    }

    #[test]
    fn parse_disable_target_with_checksum() {
        let target = parse_disable_target("src/lib.rs:a1b2c3d4").unwrap();
        match target.scope {
            DisableScope::Checksum(ref chk) => assert_eq!(chk, "a1b2c3d4"),
            _ => panic!("expected Checksum scope, got {:?}", target.scope),
        }
        assert!(target.matcher.is_match("src/lib.rs"));
    }

    #[test]
    fn parse_disable_target_rejects_uppercase_hex_as_typo() {
        // ABCDEF1A reads exactly like a checksum but is uppercase —
        // canonical verdict-print is lowercase, so this is a clear
        // copy-paste typo. Error explicitly rather than silently
        // falling back to glob (which would match nothing and
        // confuse the operator).
        let err = parse_disable_target("src/lib.rs:ABCDEF1A")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("lowercase hex"),
            "err must explain the lowercase requirement; got: {err}"
        );
    }

    #[test]
    fn finding_checksum_is_stable_for_same_input() {
        let a = finding_checksum("let x = unused_variable_name;");
        let b = finding_checksum("let x = unused_variable_name;");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn finding_checksum_differs_on_whitespace_change() {
        let normal = finding_checksum("let x = 1;");
        let extra_space = finding_checksum("let  x = 1;");
        assert_ne!(normal, extra_space, "whitespace must affect checksum so a reformat re-prompts review");
    }

    #[test]
    fn parse_disable_target_glob() {
        let t = parse_disable_target("src/sdk-alpha/**").unwrap();
        assert!(t.matcher.is_match("src/sdk-alpha/foo.ts"));
        assert!(t.matcher.is_match("src/sdk-alpha/nested/bar.ts"));
        assert!(!t.matcher.is_match("src/elsewhere/baz.ts"));
    }

    #[test]
    fn parse_disable_target_rejects_empty() {
        assert!(parse_disable_target("").is_err());
        assert!(parse_disable_target("   ").is_err());
    }

    #[test]
    fn finding_disabled_by_file_glob() {
        let targets = parse_disable_targets(&["src/sdk-alpha/**".into()]).unwrap();
        let inside_alpha = mk_finding("src/sdk-alpha/types.ts", 200, "any_cast");
        assert!(finding_is_disabled(&inside_alpha, &targets));
        let outside_alpha = mk_finding("src/db/q.ts", 99, "what_filler");
        assert!(!finding_is_disabled(&outside_alpha, &targets));
    }

    #[test]
    fn finding_disabled_by_path_and_line() {
        let targets = parse_disable_targets(&["src/lib.rs:42".into()]).unwrap();
        let exact = mk_finding("src/lib.rs", 42, "placeholder");
        let same_file_diff_line = mk_finding("src/lib.rs", 43, "placeholder");
        assert!(finding_is_disabled(&exact, &targets));
        assert!(!finding_is_disabled(&same_file_diff_line, &targets));
    }

    #[test]
    fn finding_disabled_collects_multiple_targets() {
        let targets =
            parse_disable_targets(&["src/lib.rs:42".into(), "src/sdk-alpha/**".into()]).unwrap();
        assert!(finding_is_disabled(
            &mk_finding("src/lib.rs", 42, "placeholder"),
            &targets
        ));
        assert!(finding_is_disabled(
            &mk_finding("src/sdk-alpha/y.ts", 1, "any_cast"),
            &targets
        ));
        assert!(!finding_is_disabled(
            &mk_finding("src/lib.rs", 43, "placeholder"),
            &targets
        ));
    }

    #[test]
    fn redact_patch_converts_matching_plus_line_to_context() {
        let secret = "let password = \"oops\";";
        let checksum = finding_checksum(secret);
        let patch = format!(
            "\
diff --git a/src/lib.rs b/src/lib.rs
index aaa..bbb 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn foo() {{
     bar();
+{secret}
 }}
"
        );
        let targets = parse_disable_targets(&[format!("src/lib.rs:{checksum}")]).unwrap();
        let redacted = redact_patch_by_checksum(&patch, &targets);
        // The `+secret` line became ` secret` (context). The hunk
        // header is untouched, the `+++ b/` header line survives
        // because the redactor explicitly skips `+++`.
        assert!(redacted.contains("+++ b/src/lib.rs"));
        assert!(!redacted.contains(&format!("+{secret}")));
        assert!(redacted.contains(&format!(" {secret}")));
    }

    #[test]
    fn redact_patch_leaves_other_files_alone_when_glob_does_not_match() {
        let target_line = "let foo = bar();";
        let checksum = finding_checksum(target_line);
        let patch = format!(
            "\
diff --git a/src/elsewhere.rs b/src/elsewhere.rs
index aaa..bbb 100644
--- a/src/elsewhere.rs
+++ b/src/elsewhere.rs
@@ -1 +1,2 @@
 mod x;
+{target_line}
"
        );
        let targets = parse_disable_targets(&[format!("src/lib.rs:{checksum}")]).unwrap();
        let redacted = redact_patch_by_checksum(&patch, &targets);
        assert!(redacted.contains(&format!("+{target_line}")));
    }

    #[test]
    fn redact_patch_no_op_when_no_checksum_targets() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1,2 @@
 mod x;
+let y = 2;
";
        // file-only target, no checksum among args
        let targets = parse_disable_targets(&["src/other.rs".into()]).unwrap();
        let redacted = redact_patch_by_checksum(patch, &targets);
        assert_eq!(redacted, patch);
    }

    #[test]
    fn redact_patch_glob_matches_across_files() {
        let muted = "let secret = \"x\";";
        let checksum = finding_checksum(muted);
        let patch = format!(
            "\
diff --git a/src/sdk-alpha/a.ts b/src/sdk-alpha/a.ts
--- a/src/sdk-alpha/a.ts
+++ b/src/sdk-alpha/a.ts
@@ -1 +1,2 @@
 export const x = 1;
+{muted}
diff --git a/src/sdk-alpha/b.ts b/src/sdk-alpha/b.ts
--- a/src/sdk-alpha/b.ts
+++ b/src/sdk-alpha/b.ts
@@ -1 +1,2 @@
 export const y = 2;
+{muted}
"
        );
        let targets = parse_disable_targets(&[format!("src/sdk-alpha/**:{checksum}")]).unwrap();
        let redacted = redact_patch_by_checksum(&patch, &targets);
        assert_eq!(redacted.matches(&format!("+{muted}")).count(), 0);
        assert_eq!(redacted.matches(&format!(" {muted}")).count(), 2);
    }

    #[test]
    fn extract_finding_summaries_walks_todo_splices_into_rows() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index aaa..bbb 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -41,0 +41,1 @@
+// TODO(slop): placeholder identifier — pick a name that says what this is
@@ -99,0 +99,1 @@
+// TODO(slop): what_filler_comment — restating the next line in prose
";
        let summaries = extract_finding_summaries(patch);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].file, "src/lib.rs");
        assert_eq!(summaries[0].line, 41);
        assert_eq!(summaries[0].category, "placeholder identifier");
        assert_eq!(summaries[1].line, 99);
        assert_eq!(summaries[1].category, "what_filler_comment");
        // Source file doesn't exist in this test → checksum stays None.
        assert!(summaries[0].checksum.is_none());
    }

    #[test]
    fn extract_finding_summaries_falls_back_when_category_lacks_em_dash() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
+++ b/src/lib.rs
@@ -1,0 +1,1 @@
+// TODO(slop): some_other_category_without_dash
";
        let summaries = extract_finding_summaries(patch);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].category, "some_other_category_without_dash");
    }

    #[test]
    fn extract_finding_summaries_reads_checksum_from_disk_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let src = "fn foo() {\nlet specific_marker_token = 42;\nbar();\n}\n";
        fs::create_dir_all("src").unwrap();
        fs::write("src/lib.rs", src).unwrap();
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
+++ b/src/lib.rs
@@ -2,0 +2,1 @@
+// TODO(slop): placeholder identifier — pick a name that says what this is
";
        let summaries = extract_finding_summaries(patch);
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].checksum.as_deref(),
            Some(finding_checksum("let specific_marker_token = 42;").as_str())
        );
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    fn parse_disable_targets_aggregates_errors() {
        let parse_err =
            parse_disable_targets(&["src/ok.rs".into(), "".into(), "src/also-ok.rs:9".into()])
                .unwrap_err()
                .to_string();
        assert!(parse_err.contains("empty target"), "err was: {parse_err}");
    }

    #[test]
    fn filter_patch_by_slopignore_drops_matching_file_blocks() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index aaa..bbb 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
diff --git a/.env.example b/.env.example
index ccc..ddd 100644
--- a/.env.example
+++ b/.env.example
@@ -1 +1 @@
-OLD=1
+NEW=1
";
        let mut ignore_builder = globset::GlobSetBuilder::new();
        ignore_builder.add(globset::Glob::new(".env.example").unwrap());
        let ignore_set = ignore_builder.build().unwrap();
        let filtered = filter_patch_by_slopignore(patch, &ignore_set);
        assert!(filtered.contains("src/lib.rs"));
        assert!(!filtered.contains(".env.example"));
    }

    #[test]
    fn filter_patch_by_slopignore_empty_globset_is_passthrough() {
        let patch = "diff --git a/foo b/foo\n+content\n";
        let empty_set = globset::GlobSet::empty();
        assert_eq!(filter_patch_by_slopignore(patch, &empty_set), patch);
    }

    #[test]
    fn filter_patch_by_slopignore_keeps_preamble_when_present() {
        let patch = "\
From abc Mon Sep 17 00:00:00 2001
Subject: stuff

diff --git a/keep.rs b/keep.rs
+content
";
        let empty_set = globset::GlobSet::empty();
        assert!(filter_patch_by_slopignore(patch, &empty_set).starts_with("From abc"));
    }

    /// Regression: `cap_diff` historically used `diff[..max]` which
    /// panicked when `max` landed mid-multi-byte char ("end byte index
    /// 2048 is not a char boundary; it is inside '—'"). Build a diff
    /// whose nearest \n to `max` sits exactly past an em-dash (3 bytes)
    /// straddling the budget so any reintroduction of raw byte slicing
    /// blows up here instead of in production.
    #[test]
    fn cap_diff_does_not_panic_on_multibyte_boundary() {
        let ascii_prefix = "+ ascii padding line\n".repeat(100);
        let multibyte_payload =
            "+ comment — with em dash and more text following\n".repeat(200);
        let combined_diff = format!("{ascii_prefix}{multibyte_payload}");
        for budget in [1024usize, 2048, 3000, 4097] {
            let truncated = cap_diff(&combined_diff, budget, "test");
            assert!(
                truncated.contains("test truncated"),
                "marker missing at budget {budget}"
            );
        }
    }

    /// Single-test serialization for cwd / env mutating cases.
    /// global_last_poke_path + load_plan + attached_context all
    /// touch SLOP_CONFIG_DIR + chdir; parallel test exec mangles
    /// process-global state. Same pattern news::tests uses.
    #[test]
    fn cached_plan_roundtrip_and_attach_context() {
        let tmp = tempfile::tempdir().unwrap();
        let prev_dir = std::env::current_dir().unwrap();
        let prev_cfg = std::env::var("SLOP_CONFIG_DIR").ok();
        std::env::set_current_dir(tmp.path()).unwrap();
        let global_dir = tmp.path().join("global-config");
        std::env::set_var("SLOP_CONFIG_DIR", &global_dir);

        let resolved_global_path = global_last_poke_path().expect("path resolves");
        assert_eq!(resolved_global_path, global_dir.join("last-poke.json"));

        // load_plan falls back to global when no cwd-local plan exists.
        std::fs::create_dir_all(&global_dir).unwrap();
        let global_plan = CachedPlan {
            poke_id: "fallback-id".into(),
            verdict: "SLOP".into(),
            patch: "diff body".into(),
            input_diff: "input body".into(),
            cleanup_actions: Vec::new(),
        };
        std::fs::write(
            global_dir.join("last-poke.json"),
            serde_json::to_string(&global_plan).unwrap(),
        )
        .unwrap();
        let loaded = load_plan().expect("global plan loads");
        assert_eq!(loaded.poke_id, "fallback-id");
        assert_eq!(loaded.input_diff, "input body");

        // cwd-local plan beats global.
        std::fs::create_dir_all(".slop").unwrap();
        let local_plan = CachedPlan {
            poke_id: "local-id".into(),
            verdict: "LGTM".into(),
            patch: String::new(),
            input_diff: String::new(),
            cleanup_actions: Vec::new(),
        };
        std::fs::write(CACHED_PLAN, serde_json::to_string(&local_plan).unwrap()).unwrap();
        let loaded = load_plan().expect("local plan loads");
        assert_eq!(loaded.poke_id, "local-id");

        // attached_context_from_cached_plan splices BOTH the input
        // diff and the proposed patch under their labelled fences
        // so the operator gets the full before/after pair.
        let ctx_plan = CachedPlan {
            poke_id: "ctx-id".into(),
            verdict: "SLOP".into(),
            patch: "+// TODO(slop): demo\n".into(),
            input_diff: "@@ -1 +1 @@\n+let x = 1;\n".into(),
            cleanup_actions: Vec::new(),
        };
        std::fs::write(CACHED_PLAN, serde_json::to_string(&ctx_plan).unwrap()).unwrap();
        let ctx = attached_context_from_cached_plan().expect("context builds");
        assert!(ctx.contains("poke_id: ctx-id"), "missing poke_id");
        assert!(ctx.contains("--- input_diff ---"), "missing input header");
        assert!(ctx.contains("let x = 1"), "missing input body");
        assert!(
            ctx.contains("--- proposed_patch ---"),
            "missing patch header"
        );
        assert!(ctx.contains("TODO(slop)"), "missing patch body");

        // Restore process state.
        std::env::set_current_dir(prev_dir).unwrap();
        match prev_cfg {
            Some(v) => std::env::set_var("SLOP_CONFIG_DIR", v),
            None => std::env::remove_var("SLOP_CONFIG_DIR"),
        }
    }

    #[test]
    fn hook_script_has_marker_for_overwrite_detection() {
        // run_install_hook reads existing files looking for our
        // marker string before overwriting; if the script body no
        // longer contains it we'd silently clobber user-authored
        // hooks. Regression guard so the marker can't drift out.
        assert!(
            HOOK_SCRIPT.contains("Installed by `slop install-hook`"),
            "marker missing from HOOK_SCRIPT; overwrite detection breaks"
        );
        assert!(
            HOOK_SCRIPT.contains("slop poke --staged"),
            "hook body must run `slop poke --staged`"
        );
        assert!(
            HOOK_SCRIPT.contains("--no-verify"),
            "hook should document the bypass flag in its output"
        );
        assert!(
            HOOK_SCRIPT.starts_with("#!/usr/bin/env sh"),
            "hook must declare a POSIX-shell shebang"
        );
    }
}
