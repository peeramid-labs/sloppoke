//! Payment onboarding UX for the `slop poke` 402 path.
//!
//! Server signals quota exhaustion with a structured 402 carrying the
//! Stripe Checkout URL and pricing inline. This module renders that
//! payload into a confidence-building screen — pricing block, auto-open
//! into the user's browser, optional interactive prompt, post-purchase
//! polling — and replays the original `slop poke` invocation once the
//! subscription lands.
//!
//! Three modes:
//!   - **Interactive TTY**: full UX (open, prompt, poll, replay)
//!   - **Pipeline / agent (non-TTY)**: print URL and exit 78
//!     (`EX_CONFIG`) so callers detect "needs auth" deterministically
//!   - **CI=true env**: same as non-TTY — no prompts, no polling, no
//!     browser launch
//!
//! Polling cadence: 3s × up to 100 ticks (5 minutes). Anything longer
//! than that on an interactive terminal is wasted time; the user
//! probably abandoned. They can re-run `slop poke` later.

use std::io::{self, BufRead, IsTerminal};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::api::{self, PaymentPricing, PaymentRequired, SavedConfig};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const POLL_MAX_TICKS: u32 = 100; // ~5 minutes

/// `slop poke` returned 402.
///
/// Decide between interactive onboarding (open browser + prompt +
/// poll + replay) and headless reporting (print URL and exit) based
/// on the surrounding environment. Returns `Ok(true)` when the
/// caller should re-run the original `poke` request, `Ok(false)`
/// when the flow ended cleanly (user quit), or `Err` when something
/// genuinely failed.
pub fn handle_payment_required(
    saved_config: &SavedConfig,
    payment_response: &PaymentRequired,
) -> Result<bool> {
    print_pricing_block(payment_response);

    let Some(url) = payment_response.checkout_url.as_deref() else {
        bail!(
            "server reported quota exhausted but did not return a checkout URL — \
             contact engineering@peeramid.xyz with this fingerprint"
        );
    };

    if !is_interactive() {
        eprintln!("\nSubscribe at:\n  {url}");
        eprintln!(
            "\nRe-run `slop poke` once the subscription is active. \
             Set SLOP_NO_PAYMENT_AUTOOPEN=1 to keep this behaviour on a TTY."
        );
        bail!("payment_required");
    }

    // Interactive path. Default = open browser. `c` copies URL. `q`
    // bails. Anything else also opens (cheap robustness).
    let choice = prompt_choice(url);
    match choice {
        Choice::Quit => return Ok(false),
        Choice::Copy => {
            // Print the URL again on a clean line so the user can
            // double-click select it; no actual clipboard touch since
            // pulling in arboard would bloat the binary for one knob.
            println!("{url}");
        }
        Choice::Open => {
            if let Err(e) = open_url(url) {
                eprintln!("slop: could not auto-open browser ({e}); URL: {url}");
            }
        }
    }

    eprintln!("\nslop: waiting for subscription to land…");
    let landed = poll_until_subscribed(saved_config)?;
    if landed {
        eprintln!("slop: subscribed ✓ — re-running poke");
        return Ok(true);
    }
    eprintln!(
        "slop: still waiting after 5 minutes — re-run `slop poke` once Stripe \
         confirms the subscription."
    );
    Ok(false)
}

fn print_pricing_block(payment_response: &PaymentRequired) {
    eprintln!("\n──── PAYMENT REQUIRED ────");
    if let Some(usage) = &payment_response.usage {
        eprintln!(
            "Quota: {}/{} pokes used this cycle ({}).",
            usage.calls, usage.cap, usage.period,
        );
    } else {
        eprintln!("{}", payment_response.error);
    }
    if let Some(p) = &payment_response.pricing {
        let PaymentPricing {
            tier,
            currency,
            base_dollars,
            final_dollars,
            discount_pct,
            period,
            poke_calls_cap,
            coupon_applied,
        } = p;
        let cap_short = if *poke_calls_cap >= 1000 {
            format!("{}k", poke_calls_cap / 1000)
        } else {
            poke_calls_cap.to_string()
        };
        if *coupon_applied && *discount_pct > 0 && final_dollars < base_dollars {
            eprintln!(
                "{tier}: ${final_dollars} / {period} ({currency}, launch −{discount_pct}% — was ${base_dollars}). {cap_short} pokes / cycle. Cancel anytime."
            );
            eprintln!("Coupon auto-applied at checkout — no code to enter.");
        } else {
            eprintln!(
                "{tier}: ${final_dollars} / {period} ({currency}). {cap_short} pokes / cycle. Cancel anytime."
            );
        }
    }
}

enum Choice {
    Open,
    Copy,
    Quit,
}

fn prompt_choice(url: &str) -> Choice {
    eprintln!("\nCheckout: {url}");
    eprintln!("[Enter] open in browser   [c] copy URL   [q] quit");
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return Choice::Open;
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "q" | "quit" | "n" => Choice::Quit,
        "c" | "copy" => Choice::Copy,
        _ => Choice::Open,
    }
}

/// Stdin AND stderr must be a real terminal. We also respect explicit
/// non-interactive flags so an agent can force the headless path even
/// when stdio is faked into looking like a TTY (some terminal-wrapper
/// libraries do this).
fn is_interactive() -> bool {
    if std::env::var_os("SLOP_NO_PAYMENT_AUTOOPEN").is_some() {
        return false;
    }
    if std::env::var_os("CI").is_some() {
        return false;
    }
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Cross-platform URL open. macOS = `open`, Linux = `xdg-open`,
/// Windows = `cmd /c start`. No `webbrowser` crate dependency keeps
/// the binary small.
fn open_url(url: &str) -> Result<()> {
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/c", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let status = Command::new(program).args(&args).status()?;
    if !status.success() {
        bail!("{program} exited {status}");
    }
    Ok(())
}

/// Poll `/api/v1/billing/tier` until the user's effective cap rises
/// above the free baseline OR we time out. Free tier has zero pokes
/// (FREE_POKE_CALLS=0), so any nonzero cap means a subscription
/// landed.
fn poll_until_subscribed(saved_config: &SavedConfig) -> Result<bool> {
    use std::io::Write as _;
    let stderr = io::stderr();
    let mut handle = stderr.lock();
    for tick in 0..POLL_MAX_TICKS {
        thread::sleep(POLL_INTERVAL);
        match api::billing_tier(saved_config) {
            Ok(t) => {
                if t.entitlements.poke_calls_cap > 0 {
                    return Ok(true);
                }
            }
            Err(_) => {
                // Transient failures during the Stripe → webhook
                // window are expected. Keep polling.
            }
        }
        if tick % 3 == 0 {
            let _ = write!(handle, ".");
            let _ = handle.flush();
        }
    }
    let _ = writeln!(handle);
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::PaymentUsage;

    fn mk_payment(usage: Option<PaymentUsage>, pricing: Option<PaymentPricing>) -> PaymentRequired {
        PaymentRequired {
            error: "Payment required".into(),
            reason: None,
            checkout_url: Some("https://checkout.example.test/test-session".into()),
            usage,
            pricing,
        }
    }

    #[test]
    fn is_interactive_respects_slop_no_payment_autoopen() {
        std::env::set_var("SLOP_NO_PAYMENT_AUTOOPEN", "1");
        assert!(!is_interactive(), "env knob must force headless");
        std::env::remove_var("SLOP_NO_PAYMENT_AUTOOPEN");
    }

    #[test]
    fn is_interactive_respects_ci_env() {
        std::env::set_var("CI", "true");
        assert!(!is_interactive(), "CI=true must force headless");
        std::env::remove_var("CI");
    }

    #[test]
    fn print_pricing_block_handles_payment_with_no_usage_or_pricing() {
        // Coverage smoke test: the formatting branches do not panic
        // on minimal input (no usage, no pricing). Real wire format
        // is covered by integration tests against the live server.
        let payment_response = mk_payment(None, None);
        print_pricing_block(&payment_response);
    }
}
