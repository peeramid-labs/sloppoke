//! HTTP client for the sloppoke server.
//!
//! Thin: unauthenticated `discover` (used by `slop login`) +
//! SSH-signature-authed POSTs/GETs for everything else. All detection
//! logic lives server-side; the CLI never touches a regex or AST.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;

const SIG_NAMESPACE: &str = "slop.peeramid";
const CONFIG_DIR_ENV: &str = "SLOP_CONFIG_DIR";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SavedConfig {
    pub server_url: String,
    pub fingerprint: String,
    pub ssh_key_path: PathBuf,
    #[serde(default)]
    pub slop_org: String,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverResponse {
    pub fingerprint: String,
    #[serde(default)]
    pub slop_org: String,
}

#[derive(Debug, Serialize)]
struct DiscoverBody<'a> {
    ssh_pubkey: &'a str,
}

pub fn discover(server_url: &str, pubkey_line: &str) -> Result<DiscoverResponse> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build http client")?;
    let url = format!("{}/api/v1/auth/discover", server_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&DiscoverBody {
            ssh_pubkey: pubkey_line,
        })
        .send()
        .context("POST /api/v1/auth/discover")?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("discover failed ({status}): {text}");
    }
    Ok(serde_json::from_str(&text)?)
}

// ── poke ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PokeBody {
    pub patch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PokeFinding {
    pub file: String,
    pub line: usize,
    pub category: String,
    pub matched: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CleanupAction {
    pub file: String,
    pub line: usize,
    pub action: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UsageRow {
    #[serde(default)]
    pub slop_org: String,
    #[serde(default)]
    pub period: String,
    #[serde(default)]
    pub poke_calls: u32,
    #[serde(default)]
    pub review_tokens: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PokeResponse {
    /// Server stopped sending per-finding data on the wire to avoid
    /// leaking the catalog. The patch + verdict count carry every
    /// signal the CLI needs. Default keeps older CLIs talking to
    /// newer servers without an error.
    #[serde(default)]
    pub findings: Vec<PokeFinding>,
    /// Unified-diff patch the CLI prefers via `git apply`. Empty when
    /// the server has nothing actionable (LGTM or flag-only hits) or
    /// when an older server didn't ship the field.
    #[serde(default)]
    pub patch: String,
    /// Legacy structured action list. CLI falls back to this only when
    /// `patch` is empty AND the response carries actions, so an older
    /// CLI talking to a newer server keeps working too.
    #[serde(default)]
    pub cleanup_actions: Vec<CleanupAction>,
    pub elapsed_ms: u64,
    pub verdict: String,
    pub usage: UsageRow,
    pub cap: u32,
    #[serde(default)]
    pub poke_id: String,
}

pub fn poke(cfg: &SavedConfig, project: Option<&str>, patch: &str) -> Result<PokeResponse> {
    let body = PokeBody {
        patch: patch.to_string(),
        project: project.map(|p| p.to_string()),
    };
    let json = serde_json::to_vec(&body)?;
    let resp = signed_request(cfg, "POST", "/api/v1/poke", &json)?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        let parsed: PaymentRequired =
            serde_json::from_str(&text).with_context(|| format!("parse 402 body: {text}"))?;
        return Err(PaymentError(parsed).into());
    }
    if !status.is_success() {
        bail!("poke failed ({status}): {text}");
    }
    Ok(serde_json::from_str(&text)?)
}

/// Structured 402 wire shape — emitted by the server when the org's
/// monthly poke-call cap is reached. Carries the Stripe Checkout URL,
/// inline pricing (so the CLI can render a confidence-building
/// onboarding screen without a round-trip), and the usage row that
/// triggered the lockout.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // reason / slop_org are accepted on the wire for
                    // future use; allow until we surface them.
pub struct PaymentRequired {
    pub error: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub checkout_url: Option<String>,
    #[serde(default)]
    pub pricing: Option<PaymentPricing>,
    #[serde(default)]
    pub usage: Option<PaymentUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentPricing {
    pub tier: String,
    pub currency: String,
    pub base_dollars: u32,
    pub final_dollars: u32,
    pub discount_pct: u32,
    pub period: String,
    pub poke_calls_cap: u32,
    pub coupon_applied: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PaymentUsage {
    pub calls: u32,
    pub cap: u32,
    pub period: String,
    pub slop_org: String,
}

#[derive(Debug)]
pub struct PaymentError(pub PaymentRequired);

impl std::fmt::Display for PaymentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.error)
    }
}

impl std::error::Error for PaymentError {}

// ── learn ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct LearnBody<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    /// Joined against learn-row replay so the offline RL loop can
    /// drop rows produced by an older CLI surface (e.g. before
    /// `--disable` shipped, before context auto-attach landed)
    /// without re-judging them against the current catalog.
    cli_version: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct LearnResponse {
    pub entry_id: String,
    #[allow(dead_code)]
    pub fingerprint: String,
    #[allow(dead_code)]
    pub project: Option<String>,
    pub bytes: u64,
    pub queued: usize,
    pub monthly_cap: u32,
}

pub fn learn(
    cfg: &SavedConfig,
    text: &str,
    context: Option<&str>,
    project: Option<&str>,
) -> Result<LearnResponse> {
    // Concatenate package version + build commit so a row tells the
    // operator both 'which release' and 'which build of that release'.
    // Same string the user sees from `slop --version`.
    let cli_version = concat!(
        env!("CARGO_PKG_VERSION"),
        " (commit ",
        env!("SLOP_BUILD_COMMIT"),
        ")"
    );
    let body = LearnBody {
        text,
        context,
        project,
        cli_version,
    };
    let json = serde_json::to_vec(&body)?;
    let resp = signed_request(cfg, "POST", "/api/v1/learn", &json)?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("learn failed ({status}): {text}");
    }
    Ok(serde_json::from_str(&text)?)
}

// ── billing ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Entitlements {
    pub poke_calls_cap: u32,
    pub review_token_cap: u64,
}

#[derive(Debug, Deserialize)]
pub struct TierResponse {
    pub slop_org: String,
    pub entitlements: Entitlements,
    pub usage: UsageRow,
}

#[derive(Debug, Deserialize)]
pub struct PortalResponse {
    pub url: String,
}

pub fn billing_tier(cfg: &SavedConfig) -> Result<TierResponse> {
    let resp = signed_request(cfg, "GET", "/api/v1/billing/tier", &[])?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("billing tier failed ({status}): {text}");
    }
    Ok(serde_json::from_str(&text)?)
}

pub fn billing_portal(cfg: &SavedConfig) -> Result<PortalResponse> {
    let resp = signed_request(cfg, "POST", "/api/v1/billing/portal", &[])?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("billing portal failed ({status}): {text}");
    }
    Ok(serde_json::from_str(&text)?)
}

// ── config + auth ────────────────────────────────────────────────

pub fn config_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(custom));
    }
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".config").join("slop"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load_config() -> Result<SavedConfig> {
    let p = config_path()?;
    let s = std::fs::read_to_string(&p)
        .with_context(|| format!("read {} (run `slop login` first)", p.display()))?;
    Ok(toml::from_str(&s)?)
}

pub fn save_config(cfg: &SavedConfig) -> Result<()> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)?;
    let p = config_path()?;
    std::fs::write(&p, toml::to_string_pretty(cfg)?)?;
    Ok(())
}

fn signed_request(
    cfg: &SavedConfig,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<reqwest::blocking::Response> {
    let ts = now_rfc3339_utc();
    let body_sha = sha256_hex(body);
    let path_for_sig = path.split_once('?').map(|(p, _)| p).unwrap_or(path);
    let payload = format!("{method}\n{path_for_sig}\n{ts}\n{body_sha}");
    let signature = sign_payload(&cfg.ssh_key_path, &payload)?;
    // ssh-keygen -Y sign emits a PEM-armored multi-line blob. HTTP
    // headers can't carry \n, so base64-encode the whole armored
    // body — server decodes back to the PEM ssh-keygen verify expects.
    let signature_b64 = base64::engine::general_purpose::STANDARD.encode(signature.as_bytes());
    let pubkey_line = read_pubkey_line(&cfg.ssh_key_path)?;
    // Recompute the fingerprint from the pubkey we're actually
    // sending — `cfg.fingerprint` may belong to a different keypair
    // (e.g. user re-ran `slop login --key foo` with default --pubkey
    // pointing at ed25519). Server rejects mismatched fingerprint/
    // pubkey pairs with 401 "does not match".
    let fingerprint = fingerprint_from_pubkey_line(&pubkey_line)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("build http client")?;
    let url = format!("{}{}", cfg.server_url.trim_end_matches('/'), path);

    let mut req = match method {
        "GET" => client.get(url),
        "POST" => client.post(url).body(body.to_vec()),
        "DELETE" => client.delete(url),
        other => bail!("unsupported method {other}"),
    };
    req = req
        .header("Content-Type", "application/json")
        .header("X-Slop-Ts", ts)
        .header("X-Slop-Fingerprint", fingerprint)
        .header("X-Slop-Pubkey", pubkey_line)
        .header("Authorization", format!("Slop-SSH-Sig {signature_b64}"));
    Ok(req.send()?)
}

fn fingerprint_from_pubkey_line(line: &str) -> Result<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let b64 = parts
        .get(1)
        .ok_or_else(|| anyhow!("pubkey line missing base64 body: {line:?}"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("decode pubkey base64")?;
    let hash = sha2::Sha256::digest(&bytes);
    Ok(format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash)
    ))
}

fn read_pubkey_line(private_key_path: &Path) -> Result<String> {
    let s = private_key_path.display().to_string();
    let pub_path = if s.ends_with(".pub") {
        private_key_path.to_path_buf()
    } else {
        PathBuf::from(format!("{s}.pub"))
    };
    let line = std::fs::read_to_string(&pub_path)
        .with_context(|| format!("read pubkey {}", pub_path.display()))?
        .trim()
        .to_string();
    if line.is_empty() {
        bail!("pubkey file {} is empty", pub_path.display());
    }
    Ok(line)
}

fn sign_payload(key_path: &Path, payload: &str) -> Result<String> {
    let mut child = Command::new("ssh-keygen")
        .args([
            "-Y",
            "sign",
            "-n",
            SIG_NAMESPACE,
            "-f",
            key_path
                .to_str()
                .ok_or_else(|| anyhow!("key path not utf-8: {}", key_path.display()))?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn ssh-keygen -Y sign")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(payload.as_bytes())
            .context("write sign payload")?;
    }
    let output = child.wait_with_output().context("ssh-keygen wait")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("ssh-keygen -Y sign failed: {stderr}");
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub fn sha256_hex(body: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(body);
    let d = h.finalize();
    const T: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(d.len() * 2);
    for b in d {
        out.push(T[(b >> 4) as usize] as char);
        out.push(T[(b & 0x0F) as usize] as char);
    }
    out
}

fn now_rfc3339_utc() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let s = ms.div_euclid(1000);
    let sub = ms.rem_euclid(1000);
    let (y, mo, d, h, mi, se) = unix_to_civil(s);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}.{sub:03}Z")
}

fn unix_to_civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let (y, mo, d) = civil_from_days(days);
    let h = tod / 3600;
    let mi = (tod % 3600) / 60;
    let se = tod % 60;
    (y, mo, d, h, mi, se)
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d)
}

// ── unit tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_known_vector() {
        let h = sha256_hex(b"abc");
        assert_eq!(
            h,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_hex_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn now_rfc3339_shape() {
        let s = now_rfc3339_utc();
        assert!(s.ends_with('Z'));
        assert_eq!(s.len(), 24, "len {} for {s:?}", s.len());
        assert!(s.contains('T'));
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn unix_to_civil_known() {
        assert_eq!(unix_to_civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(unix_to_civil(86_399), (1970, 1, 1, 23, 59, 59));
        assert_eq!(unix_to_civil(86_400), (1970, 1, 2, 0, 0, 0));
    }

    #[test]
    fn config_dir_respects_env_override() {
        std::env::set_var(CONFIG_DIR_ENV, "/tmp/slop-test");
        let d = config_dir().unwrap();
        assert_eq!(d, PathBuf::from("/tmp/slop-test"));
        std::env::remove_var(CONFIG_DIR_ENV);
    }
}
