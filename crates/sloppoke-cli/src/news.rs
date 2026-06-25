//! `slop news` + post-command news pings.
//!
//! Distribution channel for product announcements without an email
//! list. The CLI fetches `/api/v1/news` at most once per 24 h and
//! caches the response under `~/.config/slop/news-cache.json`.
//! Per-user "I've already read this" state lives in
//! `~/.config/slop/news-seen.json` (a list of entry IDs).
//!
//! Auto-display rule: after every successful command, if there is at
//! least one fetched entry the user has NOT seen, render the newest
//! such entry (one line + body) and mark its ID seen. One news ping
//! per command keeps the surface gentle.
//!
//! `slop news --all`   shows the full back catalog
//! `slop news --ack`   marks every cached entry as seen
//! `slop news`         shows unseen entries (or "you're caught up")

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::api;

const CACHE_FILE: &str = "news-cache.json";
const SEEN_FILE: &str = "news-seen.json";
const FETCH_INTERVAL_SECS: u64 = 24 * 3600;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NewsEntry {
    pub id: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub level: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct NewsCache {
    fetched_at: u64,
    entries: Vec<NewsEntry>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct SeenList {
    seen: Vec<String>,
}

fn cache_path() -> Result<PathBuf> {
    Ok(api::config_dir()?.join(CACHE_FILE))
}

fn seen_path() -> Result<PathBuf> {
    Ok(api::config_dir()?.join(SEEN_FILE))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read cached news, refreshing from `/api/v1/news` if the cache is
/// older than `FETCH_INTERVAL_SECS`. Network failures fall back to
/// the existing cache (or empty on first run) — news is non-critical
/// surface, never error out of the calling command.
fn fetch_news(server_url: &str) -> Vec<NewsEntry> {
    let cache = read_cache().unwrap_or_default();
    let stale = now_secs().saturating_sub(cache.fetched_at) > FETCH_INTERVAL_SECS;
    if !stale && !cache.entries.is_empty() {
        return cache.entries;
    }
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return cache.entries,
    };
    let url = format!("{}/api/v1/news", server_url.trim_end_matches('/'));
    let resp = match client.get(&url).send() {
        Ok(r) => r,
        Err(_) => return cache.entries,
    };
    if !resp.status().is_success() {
        return cache.entries;
    }
    #[derive(Deserialize)]
    struct Body {
        entries: Vec<NewsEntry>,
    }
    let body: Body = match resp.json() {
        Ok(b) => b,
        Err(_) => return cache.entries,
    };
    let _ = write_cache(&NewsCache {
        fetched_at: now_secs(),
        entries: body.entries.clone(),
    });
    body.entries
}

fn read_cache() -> Result<NewsCache> {
    let p = cache_path()?;
    let s = fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&s)?)
}

fn write_cache(c: &NewsCache) -> Result<()> {
    let p = cache_path()?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&p, serde_json::to_string_pretty(c)?)?;
    Ok(())
}

fn read_seen() -> SeenList {
    let Ok(p) = seen_path() else {
        return SeenList::default();
    };
    let Ok(s) = fs::read_to_string(&p) else {
        return SeenList::default();
    };
    serde_json::from_str(&s).unwrap_or_default()
}

fn write_seen(list: &SeenList) {
    let Ok(p) = seen_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(s) = serde_json::to_string_pretty(list) {
        let _ = fs::write(&p, s);
    }
}

/// Render a single news entry to stderr in a compact box. Pinned to
/// stderr so it never pollutes `slop poke` stdout (which carries the
/// patch and is piped into `git apply`).
fn render(entry: &NewsEntry) {
    use std::io::IsTerminal;
    let colour = std::io::stderr().is_terminal();
    let (open, close, accent) = if colour {
        match entry.level.as_str() {
            "warn" => ("\x1b[33m", "\x1b[0m", "\x1b[1;33m"),
            _ => ("\x1b[36m", "\x1b[0m", "\x1b[1;36m"),
        }
    } else {
        ("", "", "")
    };
    eprintln!();
    eprintln!("{open}──── slop news ────{close}");
    eprintln!("{accent}{}{close}", entry.title.trim());
    for line in entry.body.lines() {
        if !line.is_empty() {
            eprintln!("  {line}");
        }
    }
    eprintln!("{open}(view all: `slop news --all`){close}");
}

/// Hook called from `main()` after every successful command. Shows
/// ONE unseen entry, marks it seen. Silent when caught up or when
/// the network / cache is empty.
pub fn ping_one_unseen(server_url: &str) {
    let entries = fetch_news(server_url);
    if entries.is_empty() {
        return;
    }
    let mut seen = read_seen();
    if let Some(entry) = entries
        .iter()
        .find(|e| !seen.seen.iter().any(|id| id == &e.id))
    {
        render(entry);
        seen.seen.push(entry.id.clone());
        write_seen(&seen);
    }
}

/// `slop news` subcommand body.
pub fn run(server_url: &str, all: bool, ack: bool) -> Result<()> {
    let entries = fetch_news(server_url);
    if entries.is_empty() {
        eprintln!("slop news: nothing to show yet.");
        return Ok(());
    }
    if ack {
        let seen = SeenList {
            seen: entries.iter().map(|e| e.id.clone()).collect(),
        };
        write_seen(&seen);
        eprintln!("slop news: marked {} entries as read.", seen.seen.len());
        return Ok(());
    }
    let seen = read_seen();
    let pool: Vec<&NewsEntry> = if all {
        entries.iter().collect()
    } else {
        entries
            .iter()
            .filter(|e| !seen.seen.iter().any(|id| id == &e.id))
            .collect()
    };
    if pool.is_empty() {
        eprintln!("slop news: you're caught up.");
        return Ok(());
    }
    for entry in &pool {
        render(entry);
    }
    if !all {
        // Mark everything we just rendered as seen.
        let mut s = seen;
        for entry in pool {
            if !s.seen.iter().any(|id| id == &entry.id) {
                s.seen.push(entry.id.clone());
            }
        }
        write_seen(&s);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-test serialization: news tests mutate the process-global
    /// `SLOP_CONFIG_DIR` env var, so they can't run in parallel —
    /// and they share that env knob with
    /// `main::tests::cached_plan_roundtrip_and_attach_context`, so
    /// the lock has to live at crate root (test_globals).
    #[test]
    fn news_cache_and_seen_roundtrip() {
        let _guard = crate::test_globals::lock_cwd();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("SLOP_CONFIG_DIR", tmp.path());

        // 1. seen list defaults to empty when file missing.
        let empty = read_seen();
        assert!(empty.seen.is_empty());

        // 2. write + read seen roundtrips.
        let s = SeenList {
            seen: vec!["a".into(), "b".into()],
        };
        write_seen(&s);
        let back = read_seen();
        assert_eq!(back.seen, vec!["a".to_string(), "b".to_string()]);

        // 3. cache roundtrips.
        let cache = NewsCache {
            fetched_at: 1_780_000_000,
            entries: vec![NewsEntry {
                id: "x".into(),
                published_at: "2026-06-11T00:00:00Z".into(),
                level: "info".into(),
                title: "t".into(),
                body: "b".into(),
            }],
        };
        write_cache(&cache).unwrap();
        let read_back = read_cache().unwrap();
        assert_eq!(read_back.fetched_at, 1_780_000_000);
        assert_eq!(read_back.entries.len(), 1);
        assert_eq!(read_back.entries[0].id, "x");

        // 4. fetch_news with a fresh cache + unreachable server
        //    returns the cached entries (graceful offline mode).
        let offline = fetch_news("http://127.0.0.1:1");
        assert_eq!(offline.len(), 1);
        assert_eq!(offline[0].id, "x");

        std::env::remove_var("SLOP_CONFIG_DIR");
    }
}
