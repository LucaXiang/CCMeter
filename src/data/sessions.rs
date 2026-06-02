//! Per-session rollups for the "Recent sessions" detail list. A SessionSummary
//! is provider-tagged (Claude/Codex) and carries the session's human title,
//! cwd (for grouping into a project card), token + cost totals, and last date.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::config::discovery::Provider;
use crate::data::parser::Event;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub title: String,
    pub provider: Provider,
    pub cwd: String,
    pub tokens: u64, // input + output (matches the per-model "tokens" convention)
    pub cost: f64,
    pub last_date: NaiveDate,
}

/// Aggregate Claude `events` into one summary per session file. `session_map`
/// maps a session-file basename → (root, cwd); `titles` maps basename → title
/// (from `scan_ai_titles`). Sessions whose file isn't in `session_map` (cwd
/// unknown) are skipped. Title falls back to the short session id when absent.
pub fn claude_session_summaries(
    events: &[Event],
    session_map: &HashMap<String, (String, String)>,
    titles: &HashMap<String, String>,
) -> Vec<SessionSummary> {
    struct Acc {
        tokens: u64,
        cost: f64,
        last: NaiveDate,
    }
    let mut by_file: HashMap<&str, Acc> = HashMap::new();
    for e in events {
        let Some((_root, _cwd)) = session_map.get(&e.session_file) else {
            continue;
        };
        let date = e.timestamp.with_timezone(&chrono::Local).date_naive();
        let acc = by_file.entry(e.session_file.as_str()).or_insert(Acc {
            tokens: 0,
            cost: 0.0,
            last: date,
        });
        acc.tokens += e.input_tokens + e.output_tokens;
        acc.cost += e.cost_usd;
        if date > acc.last {
            acc.last = date;
        }
    }
    by_file
        .into_iter()
        .map(|(file, acc)| {
            let cwd = session_map
                .get(file)
                .map(|(_, c)| c.clone())
                .unwrap_or_default();
            let title = titles
                .get(file)
                .cloned()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| short_id(file));
            SessionSummary {
                title,
                provider: Provider::Claude,
                cwd,
                tokens: acc.tokens,
                cost: acc.cost,
                last_date: acc.last,
            }
        })
        .collect()
}

/// A short, stable label from a session file basename (strip `.jsonl`, keep the
/// leading id chunk) for sessions with no human title.
pub(crate) fn short_id(file: &str) -> String {
    let stem = file.strip_suffix(".jsonl").unwrap_or(file);
    stem.chars().take(8).collect()
}

#[derive(serde::Deserialize)]
struct TitleLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
}

/// Map each session-file basename → its `ai-title` (Claude's AI-generated title
/// `{"type":"ai-title","aiTitle":"…"}`). Reads each file line-by-line and stops
/// at the first ai-title (titles sit near the top). Files without one are absent.
pub fn scan_ai_titles(files: &[PathBuf]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for path in files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(title) = scan_one_ai_title(path) {
            out.insert(name.to_string(), title);
        }
    }
    out
}

fn scan_one_ai_title(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("ai-title") {
            continue;
        }
        if let Ok(t) = serde_json::from_str::<TitleLine>(&line)
            && t.kind.as_deref() == Some("ai-title")
            && let Some(title) = t.ai_title.filter(|s| !s.is_empty())
        {
            return Some(title);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn ev(file: &str, ymd: (i32, u32, u32), input: u64, output: u64, cost: f64) -> Event {
        Event {
            timestamp: Utc.with_ymd_and_hms(ymd.0, ymd.1, ymd.2, 12, 0, 0).unwrap(),
            model: "claude-opus-4-6".into(),
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            cost_usd: cost,
            lines_suggested: 0,
            lines_accepted: 0,
            lines_added: 0,
            lines_deleted: 0,
            session_file: file.into(),
            request_id: None,
            raw_cost_usd: None,
            line_uuid: None,
        }
    }

    #[test]
    fn aggregates_claude_events_per_session_with_title() {
        let events = vec![
            ev("s1.jsonl", (2026, 5, 4), 100, 50, 1.0),
            ev("s1.jsonl", (2026, 5, 6), 10, 5, 0.5),
            ev("s2.jsonl", (2026, 5, 5), 20, 10, 0.2),
        ];
        let mut session_map = HashMap::new();
        session_map.insert("s1.jsonl".to_string(), ("/r".to_string(), "/p/crab".to_string()));
        session_map.insert("s2.jsonl".to_string(), ("/r".to_string(), "/p/crab".to_string()));
        let mut titles = HashMap::new();
        titles.insert("s1.jsonl".to_string(), "重新部署 dev-cloud".to_string());
        let out = claude_session_summaries(&events, &session_map, &titles);
        let s1 = out.iter().find(|s| s.title == "重新部署 dev-cloud").expect("s1");
        assert_eq!(s1.provider, Provider::Claude);
        assert_eq!(s1.cwd, "/p/crab");
        assert_eq!(s1.tokens, 165, "input+output across both events");
        assert!((s1.cost - 1.5).abs() < 1e-9);
        assert_eq!(s1.last_date, NaiveDate::from_ymd_opt(2026, 5, 6).unwrap());
        // s2 has no ai-title → falls back to a non-empty id-derived label.
        let s2 = out
            .iter()
            .find(|s| s.cwd == "/p/crab" && s.title != "重新部署 dev-cloud")
            .expect("s2");
        assert!(!s2.title.is_empty());
    }

    #[test]
    fn skips_events_with_unknown_session() {
        let events = vec![ev("ghost.jsonl", (2026, 5, 4), 5, 5, 0.1)];
        let out = claude_session_summaries(&events, &HashMap::new(), &HashMap::new());
        assert!(out.is_empty());
    }

    #[test]
    fn scans_ai_title_lines() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ccmeter-aititle-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("abc.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, r#"{{"type":"user","cwd":"/p/crab"}}"#).unwrap();
        writeln!(
            f,
            r#"{{"type":"ai-title","aiTitle":"重新部署 dev-cloud","sessionId":"abc"}}"#
        )
        .unwrap();
        let titles = scan_ai_titles(&[p.clone()]);
        assert_eq!(
            titles.get("abc.jsonl").map(String::as_str),
            Some("重新部署 dev-cloud")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
