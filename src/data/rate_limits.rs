use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Timelike, Utc};
use rayon::prelude::*;
use serde::Deserialize;

/// A single rate-limit hit extracted from a JSONL line.
#[derive(Debug, Clone)]
pub struct RateLimitHit {
    pub timestamp: DateTime<Utc>,
    /// The human-readable message (e.g. "You've hit your limit · resets 6pm (Europe/Paris)").
    #[allow(dead_code)]
    pub message: String,
    /// Which source root this came from (e.g. `~/.claude/projects` or `~/.claude-pro/projects`).
    pub source_root: String,
    /// Session duration in minutes: time from first assistant message to this hit,
    /// considering only messages from the same source_root.
    pub session_duration_min: Option<f64>,
    /// Total tokens (input + output) consumed during this rate-limited session.
    /// Derived from `per_model`; kept denormalized for cheap rendering.
    pub tokens: u64,
    /// Per-model tokens + cost split observed across the rate-limited session.
    pub per_model: Vec<super::models::PerModelUsage>,
    /// Stable key used to merge duplicate detections of the same hit.
    pub dedup_key: String,
}

// ---------------------------------------------------------------------------
// Serde helpers — only what we need
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawHitLine {
    timestamp: Option<String>,
    message: Option<RawHitMessage>,
    error: Option<String>,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error: Option<bool>,
}

#[derive(Deserialize)]
struct RawHitMessage {
    content: Option<Vec<RawHitContent>>,
}

#[derive(Deserialize)]
struct RawHitContent {
    text: Option<String>,
}

#[derive(Deserialize)]
struct RawCodexLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    line_type: Option<String>,
    payload: Option<RawCodexPayload>,
}

#[derive(Deserialize)]
struct RawCodexPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    rate_limits: Option<RawCodexRateLimits>,
}

#[derive(Deserialize)]
struct RawCodexRateLimits {
    #[serde(default)]
    rate_limit_reached_type: Option<String>,
    primary: Option<RawCodexWindow>,
    secondary: Option<RawCodexWindow>,
}

#[derive(Clone, Copy, Deserialize)]
struct RawCodexWindow {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Internal: find rate-limit lines in a single file
// ---------------------------------------------------------------------------

struct RawHit {
    timestamp: DateTime<Utc>,
    message: String,
    session_duration_min: Option<f64>,
    stable_dedup_key: Option<String>,
    bucket_tag: Option<String>,
}

fn scan_file_for_hits(path: &Path) -> Vec<RawHit> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut hits = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        // Fast pre-filter to avoid parsing every line
        if !line.contains("\"rate_limit\"") || !line.contains("\"isApiErrorMessage\"") {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<RawHitLine>(&line) else {
            continue;
        };
        if raw.error.as_deref() != Some("rate_limit") || raw.is_api_error != Some(true) {
            continue;
        }
        let Some(ts_str) = raw.timestamp else {
            continue;
        };
        let Ok(ts) = DateTime::parse_from_rfc3339(&ts_str) else {
            continue;
        };

        let message = raw
            .message
            .and_then(|m| m.content)
            .and_then(|c| c.into_iter().next())
            .and_then(|b| b.text)
            .unwrap_or_default();

        hits.push(RawHit {
            timestamp: ts.with_timezone(&Utc),
            message,
            session_duration_min: None,
            stable_dedup_key: None,
            bucket_tag: None,
        });
    }
    hits
}

fn scan_file_for_codex_hits(path: &Path) -> Vec<RawHit> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut hits = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("\"rate_limits\"") || !line.contains("\"token_count\"") {
            continue;
        }
        let Some(mut line_hits) = parse_codex_hit_line(&line) else {
            continue;
        };
        hits.append(&mut line_hits);
    }

    hits
}

fn parse_codex_hit_line(line: &str) -> Option<Vec<RawHit>> {
    let raw: RawCodexLine = serde_json::from_str(line).ok()?;
    if raw.line_type.as_deref() != Some("event_msg") {
        return None;
    }

    let payload = raw.payload?;
    if payload.payload_type.as_deref() != Some("token_count") {
        return None;
    }

    let ts = DateTime::parse_from_rfc3339(raw.timestamp.as_deref()?)
        .ok()?
        .with_timezone(&Utc);
    let limits = payload.rate_limits?;
    let reached = limits
        .rate_limit_reached_type
        .as_deref()
        .and_then(codex_reached_window);

    let mut hits = Vec::new();
    push_codex_window_hit(
        &mut hits,
        ts,
        "primary",
        limits.primary,
        reached == Some("primary"),
    );
    push_codex_window_hit(
        &mut hits,
        ts,
        "secondary",
        limits.secondary,
        reached == Some("secondary"),
    );

    if hits.is_empty()
        && limits
            .rate_limit_reached_type
            .as_deref()
            .is_some_and(|s| !s.is_empty())
    {
        hits.push(RawHit {
            timestamp: ts,
            message: format!(
                "Codex rate limit reached ({})",
                limits.rate_limit_reached_type.unwrap_or_default()
            ),
            session_duration_min: None,
            stable_dedup_key: None,
            bucket_tag: Some("codex:unknown".to_string()),
        });
    }

    Some(hits)
}

fn codex_reached_window(value: &str) -> Option<&'static str> {
    let v = value.to_ascii_lowercase();
    if v.contains("secondary") || v.contains("weekly") || v.contains("week") || v.contains("7d") {
        Some("secondary")
    } else if v.contains("primary") || v.contains("5h") || v.contains("five") {
        Some("primary")
    } else {
        None
    }
}

fn push_codex_window_hit(
    hits: &mut Vec<RawHit>,
    timestamp: DateTime<Utc>,
    label: &'static str,
    window: Option<RawCodexWindow>,
    reached: bool,
) {
    let Some(window) = window else {
        return;
    };
    let saturated = window.used_percent.is_some_and(|pct| pct >= 100.0);
    if !reached && !saturated {
        return;
    }

    let pct = window.used_percent.unwrap_or(100.0);
    let message = format!("Codex {label} limit reached ({pct:.0}%)");
    let stable_dedup_key = window
        .resets_at
        .map(|reset| format!("codex:{label}:reset:{reset}"));
    hits.push(RawHit {
        timestamp,
        message,
        session_duration_min: codex_window_duration_min(timestamp, window),
        stable_dedup_key,
        bucket_tag: Some(format!("codex:{label}")),
    });
}

fn codex_window_duration_min(timestamp: DateTime<Utc>, window: RawCodexWindow) -> Option<f64> {
    let resets_at = DateTime::<Utc>::from_timestamp(window.resets_at?, 0)?;
    let window_minutes = window.window_minutes?;
    let start = resets_at - chrono::Duration::minutes(window_minutes as i64);
    let elapsed = (timestamp - start).num_seconds().max(0) as f64 / 60.0;
    Some(elapsed.min(window_minutes as f64))
}

// ---------------------------------------------------------------------------
// Internal: find first assistant timestamp in a file
// ---------------------------------------------------------------------------

fn first_assistant_timestamp(path: &Path) -> Option<DateTime<Utc>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("\"assistant\"") {
            continue;
        }
        #[derive(Deserialize)]
        struct Stub {
            timestamp: Option<String>,
            #[serde(rename = "type")]
            line_type: Option<String>,
            message: Option<StubMsg>,
        }
        #[derive(Deserialize)]
        struct StubMsg {
            usage: Option<serde_json::Value>,
        }
        let Ok(stub) = serde_json::from_str::<Stub>(&line) else {
            continue;
        };
        if stub.line_type.as_deref() != Some("assistant") {
            continue;
        }
        // Only count lines that have actual usage (real API calls)
        if stub.message.and_then(|m| m.usage).is_none() {
            continue;
        }
        let ts_str = stub.timestamp?;
        let ts = DateTime::parse_from_rfc3339(&ts_str).ok()?;
        return Some(ts.with_timezone(&Utc));
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Discover all rate-limit hits across all JSONL files in the given source roots.
/// Each root is a path like `~/.claude/projects` or `~/.claude-pro/projects`.
/// Returns hits sorted by timestamp (most recent first), de-duplicated by minute.
pub fn discover_rate_limit_hits(source_roots: &[PathBuf]) -> Vec<RateLimitHit> {
    // Collect all (file, source_root) pairs
    let mut file_root_pairs: Vec<(PathBuf, String)> = Vec::new();
    for root in source_roots {
        let root_str = root.to_string_lossy().to_string();
        collect_jsonl_recursive(root, &root_str, &mut file_root_pairs);
    }

    // Scan all files for rate-limit hits in parallel
    let raw_hits: Vec<(RawHit, String)> = file_root_pairs
        .par_iter()
        .flat_map(|(path, root_str)| {
            scan_file_for_hits(path)
                .into_iter()
                .chain(scan_file_for_codex_hits(path))
                .map(|h| (h, root_str.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    // De-duplicate by source + stable hit key. Claude falls back to a
    // 15-minute bucket; Codex windows use the reset timestamp when available.
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<(RawHit, String, String)> = Vec::new();
    // Sort chronologically first so we keep the earliest per bucket
    let mut sorted = raw_hits;
    sorted.sort_by_key(|(h, _)| h.timestamp);
    for pair in sorted {
        let key = raw_hit_dedup_key(&pair.0, &pair.1);
        if seen.insert(key.clone()) {
            deduped.push((pair.0, pair.1, key));
        }
    }

    // Only keep hits from the last 30 days
    let cutoff = Utc::now() - chrono::Duration::days(30);
    deduped.retain(|(h, _, _)| h.timestamp >= cutoff);

    // Pre-cache first assistant timestamp per file (avoids reopening files per-hit)
    let file_first_ts: std::collections::HashMap<PathBuf, DateTime<Utc>> = file_root_pairs
        .par_iter()
        .filter_map(|(path, _)| first_assistant_timestamp(path).map(|ts| (path.clone(), ts)))
        .collect();

    // Group cached timestamps by source root
    let ts_by_root: std::collections::HashMap<&str, Vec<DateTime<Utc>>> = {
        let mut map: std::collections::HashMap<&str, Vec<DateTime<Utc>>> =
            std::collections::HashMap::new();
        for (path, root_str) in &file_root_pairs {
            if let Some(&ts) = file_first_ts.get(path) {
                map.entry(root_str.as_str()).or_default().push(ts);
            }
        }
        map
    };

    let mut hits: Vec<RateLimitHit> = deduped
        .into_iter()
        .map(|(raw, root_str, dedup_key)| {
            let duration = raw.session_duration_min.or_else(|| {
                ts_by_root.get(root_str.as_str()).and_then(|timestamps| {
                    let window_start = raw.timestamp - chrono::Duration::hours(5);
                    timestamps
                        .iter()
                        .filter(|ts| **ts >= window_start && **ts <= raw.timestamp)
                        .min()
                        .map(|first| (raw.timestamp - *first).num_seconds() as f64 / 60.0)
                })
            });
            RateLimitHit {
                timestamp: raw.timestamp,
                message: raw.message,
                source_root: root_str,
                session_duration_min: duration,
                tokens: 0,
                per_model: Vec::new(),
                dedup_key,
            }
        })
        .collect();

    // Sort most recent first
    hits.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    hits
}

fn raw_hit_dedup_key(hit: &RawHit, source_root: &str) -> String {
    if let Some(stable) = &hit.stable_dedup_key {
        return format!("{source_root}-{stable}");
    }

    let bucket = format!(
        "{}-{:02}-{}",
        hit.timestamp.format("%Y-%m-%dT%H"),
        hit.timestamp.minute() / 15 * 15,
        source_root,
    );
    match &hit.bucket_tag {
        Some(tag) => format!("{bucket}-{tag}"),
        None => bucket,
    }
}

fn collect_jsonl_recursive(dir: &Path, root_str: &str, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file() {
            out.push((path, root_str.to_string()));
        } else if path.is_dir() {
            collect_jsonl_recursive(&path, root_str, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_tmp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ccmeter_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_jsonl<S: AsRef<str>>(dir: &Path, name: &str, lines: &[S]) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", line.as_ref()).unwrap();
        }
        path
    }

    #[test]
    fn detects_rate_limit_hit() {
        let tmp = make_tmp_dir("detect");
        let first_ts = (Utc::now() - chrono::Duration::minutes(120)).to_rfc3339();
        let hit_ts = Utc::now().to_rfc3339();
        write_jsonl(
            &tmp,
            "session.jsonl",
            &[
                format!(
                    r#"{{"type":"assistant","timestamp":"{first_ts}","message":{{"model":"claude-opus-4-6","usage":{{"input_tokens":100,"output_tokens":50}}}}}}"#
                ),
                format!(
                    r#"{{"type":"assistant","timestamp":"{hit_ts}","message":{{"content":[{{"type":"text","text":"You've hit your limit · resets 6pm"}}]}},"error":"rate_limit","isApiErrorMessage":true}}"#
                ),
            ],
        );
        let hits = discover_rate_limit_hits(std::slice::from_ref(&tmp));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.contains("hit your limit"));
        let dur = hits[0].session_duration_min.unwrap();
        assert!((dur - 120.0).abs() < 1.0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ignores_non_api_error() {
        let tmp = make_tmp_dir("ignore");
        write_jsonl(
            &tmp,
            "session.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.000Z","message":{"content":[{"type":"text","text":"rate_limit mentioned in conversation"}]}}"#,
            ],
        );
        let hits = discover_rate_limit_hits(std::slice::from_ref(&tmp));
        assert!(hits.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn deduplicates_same_minute() {
        let tmp = make_tmp_dir("dedup");
        let sub = tmp.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let hit_ts = Utc::now().to_rfc3339();
        let rl_line = format!(
            r#"{{"type":"assistant","timestamp":"{hit_ts}","message":{{"content":[{{"type":"text","text":"limit hit"}}]}},"error":"rate_limit","isApiErrorMessage":true}}"#
        );
        write_jsonl(&tmp, "a.jsonl", std::slice::from_ref(&rl_line));
        write_jsonl(&sub, "b.jsonl", std::slice::from_ref(&rl_line));
        let hits = discover_rate_limit_hits(std::slice::from_ref(&tmp));
        assert_eq!(hits.len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detects_codex_saturated_windows() {
        let tmp = make_tmp_dir("codex_saturated");
        let ts = Utc::now();
        let reset = (ts + chrono::Duration::minutes(60)).timestamp();
        let line = format!(
            r#"{{"type":"event_msg","timestamp":"{}","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":100.0,"window_minutes":300,"resets_at":{reset}}},"secondary":{{"used_percent":100.0,"window_minutes":10080,"resets_at":{reset}}},"rate_limit_reached_type":null}}}}}}"#,
            ts.to_rfc3339()
        );
        write_jsonl(&tmp, "codex.jsonl", &[line]);

        let hits = discover_rate_limit_hits(std::slice::from_ref(&tmp));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.message.contains("primary")));
        assert!(hits.iter().any(|h| h.message.contains("secondary")));
        assert!(hits.iter().all(|h| h.session_duration_min.is_some()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn deduplicates_codex_saturated_window_by_reset() {
        let tmp = make_tmp_dir("codex_saturated_dedup");
        let ts = Utc::now();
        let reset = (ts + chrono::Duration::minutes(60)).timestamp();
        let line1 = format!(
            r#"{{"type":"event_msg","timestamp":"{}","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":100.0,"window_minutes":300,"resets_at":{reset}}}}}}}}}"#,
            ts.to_rfc3339()
        );
        let line2 = format!(
            r#"{{"type":"event_msg","timestamp":"{}","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":101.0,"window_minutes":300,"resets_at":{reset}}}}}}}}}"#,
            (ts + chrono::Duration::minutes(20)).to_rfc3339()
        );
        write_jsonl(&tmp, "codex.jsonl", &[line1, line2]);

        let hits = discover_rate_limit_hits(std::slice::from_ref(&tmp));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].dedup_key.contains(&format!("reset:{reset}")));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detects_codex_explicit_reached_type() {
        let tmp = make_tmp_dir("codex_reached_type");
        let ts = Utc::now();
        let reset = (ts + chrono::Duration::minutes(60)).timestamp();
        let line = format!(
            r#"{{"type":"event_msg","timestamp":"{}","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":42.0,"window_minutes":300,"resets_at":{reset}}},"secondary":{{"used_percent":8.0,"window_minutes":10080,"resets_at":{reset}}},"rate_limit_reached_type":"primary"}}}}}}"#,
            ts.to_rfc3339()
        );
        write_jsonl(&tmp, "codex.jsonl", &[line]);

        let hits = discover_rate_limit_hits(std::slice::from_ref(&tmp));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.contains("primary"));
        assert!(hits[0].message.contains("42%"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
