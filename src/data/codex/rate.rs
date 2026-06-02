//! Parse OpenAI Codex's live rate-limit snapshots. Codex writes a
//! `rate_limits` object onto each `token_count` event in the session JSONL,
//! with a `primary` (5h) and `secondary` (7d) window — the same shape as
//! Anthropic's 5h/7d usage windows, so it slots into the rate-tracking view.

use std::collections::HashMap;

use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::Deserialize;

use crate::data::rate_limits::RateLimitHit;

/// Latest rate-limit snapshot from a Codex session (percent used + reset time
/// per window). Percentages are 0..100.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexRateLimits {
    pub five_hour_percent: f64,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_percent: f64,
    pub seven_day_resets_at: Option<i64>,
    pub plan_type: Option<String>,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct RateLimits {
    primary: Option<Window>,
    secondary: Option<Window>,
    plan_type: Option<String>,
}

#[derive(Deserialize)]
struct Window {
    #[serde(default)]
    used_percent: f64,
    resets_at: Option<i64>,
}

/// Read the most recently modified active Codex session and return its latest
/// rate-limit snapshot (5h / 7d window state). Scans newest-first and returns
/// the first session that carries a snapshot. `None` if Codex isn't in use.
pub fn latest_codex_rate_limits() -> Option<CodexRateLimits> {
    let home = dirs::home_dir()?;
    let dir = home.join(".codex").join("sessions");
    let mut files = Vec::new();
    super::collect_jsonl(&dir, &mut files);
    files.sort_by_key(|p| {
        std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok())
    });
    files
        .into_iter()
        .find_map(|f| std::fs::read_to_string(&f).ok().and_then(|raw| parse_latest_rate_limits(&raw)))
}

/// Return the LAST rate-limit snapshot in a session file's contents (the most
/// recent window state for that session). `None` if the file carries none.
pub fn parse_latest_rate_limits(raw: &str) -> Option<CodexRateLimits> {
    let mut latest = None;
    for line in raw.lines() {
        if !line.contains("rate_limits") {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Line>(line) else {
            continue;
        };
        if rec.kind.as_deref() != Some("event_msg") {
            continue;
        }
        let Some(rl) = rec.payload.and_then(|p| p.rate_limits) else {
            continue;
        };
        latest = Some(CodexRateLimits {
            five_hour_percent: rl.primary.as_ref().map(|w| w.used_percent).unwrap_or(0.0),
            five_hour_resets_at: rl.primary.as_ref().and_then(|w| w.resets_at),
            seven_day_percent: rl.secondary.as_ref().map(|w| w.used_percent).unwrap_or(0.0),
            seven_day_resets_at: rl.secondary.as_ref().and_then(|w| w.resets_at),
            plan_type: rl.plan_type,
        });
    }
    latest
}

/// Percent at/above which a window is treated as a (near-)rate-limit event.
/// Codex never sets `rate_limit_reached_type`, so the percentage is the only
/// signal that the user was throttled / about to be.
const NEAR_LIMIT_PERCENT: f64 = 95.0;

/// Discover Codex rate-limit events from the active sessions: points where the
/// 5h or 7d window crossed [`NEAR_LIMIT_PERCENT`]. Deduplicated to one event
/// per (local day, window) at that day's peak, so a sustained near-limit week
/// doesn't flood the list.
pub fn discover_codex_rate_limit_hits() -> Vec<RateLimitHit> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let dir = home.join(".codex").join("sessions");
    let mut files = Vec::new();
    super::collect_jsonl(&dir, &mut files);

    // (local day, window) -> (timestamp, percent) of the day's peak.
    let mut by_day: HashMap<(NaiveDate, &'static str), (DateTime<Utc>, f64)> = HashMap::new();
    for f in files {
        let Ok(raw) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (ts, window, pct) in session_window_peaks(&raw) {
            if pct < NEAR_LIMIT_PERCENT {
                continue;
            }
            let day = ts.with_timezone(&Local).date_naive();
            let slot = by_day.entry((day, window)).or_insert((ts, pct));
            if pct >= slot.1 {
                *slot = (ts, pct);
            }
        }
    }

    by_day
        .into_iter()
        .map(|((_, window), (ts, pct))| make_hit(ts, window, pct))
        .collect()
}

/// Peak `used_percent` (with its timestamp) of each window within one session.
fn session_window_peaks(raw: &str) -> Vec<(DateTime<Utc>, &'static str, f64)> {
    let mut peak_5h: Option<(DateTime<Utc>, f64)> = None;
    let mut peak_7d: Option<(DateTime<Utc>, f64)> = None;
    for line in raw.lines() {
        if !line.contains("rate_limits") {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Line>(line) else {
            continue;
        };
        if rec.kind.as_deref() != Some("event_msg") {
            continue;
        }
        let Some(ts) = rec
            .timestamp
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.with_timezone(&Utc))
        else {
            continue;
        };
        let Some(rl) = rec.payload.and_then(|p| p.rate_limits) else {
            continue;
        };
        if let Some(w) = &rl.primary {
            bump(&mut peak_5h, ts, w.used_percent);
        }
        if let Some(w) = &rl.secondary {
            bump(&mut peak_7d, ts, w.used_percent);
        }
    }
    let mut out = Vec::new();
    if let Some((ts, pct)) = peak_5h {
        out.push((ts, "5h", pct));
    }
    if let Some((ts, pct)) = peak_7d {
        out.push((ts, "7d", pct));
    }
    out
}

fn bump(peak: &mut Option<(DateTime<Utc>, f64)>, ts: DateTime<Utc>, pct: f64) {
    if peak.is_none_or(|(_, p)| pct > p) {
        *peak = Some((ts, pct));
    }
}

fn make_hit(ts: DateTime<Utc>, window: &str, pct: f64) -> RateLimitHit {
    RateLimitHit {
        timestamp: ts,
        message: format!("Codex {window} usage {pct:.0}% (near limit)"),
        source_root: super::CODEX_ROOT.to_string(),
        session_duration_min: None,
        tokens: 0,
        per_model: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_latest_rate_limits_snapshot() {
        let raw = [
            r#"{"type":"event_msg","timestamp":"2026-06-02T01:00:00Z","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":1.0,"window_minutes":300,"resets_at":1780379434},"secondary":{"used_percent":12.0,"window_minutes":10080,"resets_at":1780846302},"plan_type":"pro"}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-06-02T01:57:54Z","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1780379999},"secondary":{"used_percent":29.0,"window_minutes":10080,"resets_at":1780846302},"plan_type":"pro"}}}"#,
        ]
        .join("\n");
        let rl = parse_latest_rate_limits(&raw).expect("snapshot present");
        // Latest event wins.
        assert_eq!(rl.five_hour_percent, 5.0);
        assert_eq!(rl.five_hour_resets_at, Some(1780379999));
        assert_eq!(rl.seven_day_percent, 29.0);
        assert_eq!(rl.seven_day_resets_at, Some(1780846302));
        assert_eq!(rl.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn detects_window_peaks_for_hits() {
        let raw = [
            r#"{"type":"event_msg","timestamp":"2026-06-02T01:00:00Z","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":10.0,"resets_at":1},"secondary":{"used_percent":80.0,"resets_at":2}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-06-02T01:30:00Z","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":12.0,"resets_at":1},"secondary":{"used_percent":97.0,"resets_at":2}}}}"#,
        ]
        .join("\n");
        let peaks = session_window_peaks(&raw);
        assert_eq!(peaks.iter().find(|(_, w, _)| *w == "7d").unwrap().2, 97.0);
        assert_eq!(peaks.iter().find(|(_, w, _)| *w == "5h").unwrap().2, 12.0);
    }

    #[test]
    fn none_when_no_rate_limits() {
        let raw = r#"{"type":"event_msg","timestamp":"2026-06-02T01:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1}}}}"#;
        assert!(parse_latest_rate_limits(raw).is_none());
    }
}
