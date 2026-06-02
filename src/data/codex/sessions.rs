//! Parse `session_meta` from Codex session files for project grouping:
//! cwd + git identity (repository_url, repo_root) + the session UUID.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct CodexSessionMeta {
    pub session_id: String,
    pub cwd: String,
    pub repository_url: Option<String>,
    pub repo_root: Option<String>,
}

#[derive(Deserialize)]
struct Line { #[serde(rename = "type")] kind: Option<String>, payload: Option<Payload> }
#[derive(Deserialize)]
struct Payload { id: Option<String>, cwd: Option<String>, git: Option<Git> }
#[derive(Deserialize)]
struct Git { repository_url: Option<String>, repo_root: Option<String> }

/// Parse the FIRST `session_meta` line of a session file's contents.
pub fn parse_session_meta(raw: &str) -> Option<CodexSessionMeta> {
    for line in raw.lines() {
        if !line.contains("session_meta") { continue; }
        let Ok(rec) = serde_json::from_str::<Line>(line) else { continue };
        if rec.kind.as_deref() != Some("session_meta") { continue; }
        let p = rec.payload?;
        let cwd = p.cwd?;
        return Some(CodexSessionMeta {
            session_id: p.id.unwrap_or_default(),
            cwd,
            repository_url: p.git.as_ref().and_then(|g| g.repository_url.clone()),
            repo_root: p.git.and_then(|g| g.repo_root),
        });
    }
    None
}

/// Parse `session_meta` from every `*.jsonl` under `dir` (recurses into
/// subdirectories, like `collect_jsonl`). Test seam for the collector.
#[cfg(test)]
pub(crate) fn collect_session_meta_in(dir: &std::path::Path) -> Vec<CodexSessionMeta> {
    let mut files = Vec::new();
    super::collect_jsonl(dir, &mut files);
    files.iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|raw| parse_session_meta(&raw))
        .collect()
}

/// Parse `session_meta` from every discovered Codex session (sessions/ +
/// archived_sessions/), deduped by filename like the delta path.
pub fn collect_codex_session_meta() -> Vec<CodexSessionMeta> {
    super::discover_session_files()
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|raw| parse_session_meta(&raw))
        .collect()
}

// ── Thread-name index + session summaries ────────────────────────────────────

use std::collections::HashMap;

use crate::config::discovery::Provider;
use crate::data::models::cost_from_tokens;
use crate::data::sessions::{short_id, SessionSummary};

#[derive(serde::Deserialize)]
struct IndexLine {
    id: Option<String>,
    thread_name: Option<String>,
}

/// Parse `~/.codex/session_index.jsonl` contents → (session id → thread_name).
pub fn parse_thread_names(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in raw.lines() {
        let Ok(rec) = serde_json::from_str::<IndexLine>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (rec.id, rec.thread_name) {
            if !name.is_empty() {
                out.insert(id, name);
            }
        }
    }
    out
}

/// Read the session-name index from disk (empty if absent).
pub fn read_thread_names() -> HashMap<String, String> {
    let Some(home) = dirs::home_dir() else {
        return HashMap::new();
    };
    std::fs::read_to_string(home.join(".codex").join("session_index.jsonl"))
        .map(|raw| parse_thread_names(&raw))
        .unwrap_or_default()
}

/// Aggregate Codex deltas into one summary per session_id. `tokens` is
/// input+output (matching the per-model breakdown); cost reconstructs the
/// cache-inclusive input like the cache/index path. Title = thread_name, else
/// a short id fallback. Deltas with an empty session_id are skipped.
pub fn codex_session_summaries(
    deltas: &[crate::data::codex::CodexDelta],
    names: &HashMap<String, String>,
) -> Vec<SessionSummary> {
    struct Acc {
        tokens: u64,
        cost: f64,
        last: chrono::NaiveDate,
        cwd: String,
    }
    let mut by_sid: HashMap<&str, Acc> = HashMap::new();
    for d in deltas {
        if d.session_id.is_empty() {
            continue;
        }
        let cost = cost_from_tokens(&d.model, d.input + d.cache_read, d.output, d.cache_read, 0);
        let acc = by_sid.entry(d.session_id.as_str()).or_insert(Acc {
            tokens: 0,
            cost: 0.0,
            last: d.date,
            cwd: d.cwd.clone(),
        });
        acc.tokens += d.input + d.output;
        acc.cost += cost;
        if d.date > acc.last {
            acc.last = d.date;
        }
    }
    by_sid
        .into_iter()
        .map(|(sid, acc)| SessionSummary {
            title: names
                .get(sid)
                .cloned()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| short_id(sid)),
            provider: Provider::Codex,
            cwd: acc.cwd,
            tokens: acc.tokens,
            cost: acc.cost,
            last_date: acc.last,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thread_names_from_index() {
        let raw = [
            r#"{"id":"uuid-1","thread_name":"了解项目","updated_at":"2026-05-02T14:25:04Z"}"#,
            r#"{"id":"uuid-2","thread_name":"修复金额类型","updated_at":"2026-05-02T16:49:41Z"}"#,
        ]
        .join("\n");
        let names = parse_thread_names(&raw);
        assert_eq!(names.get("uuid-1").map(String::as_str), Some("了解项目"));
        assert_eq!(names.get("uuid-2").map(String::as_str), Some("修复金额类型"));
    }

    #[test]
    fn summarizes_codex_sessions_with_thread_name() {
        use crate::data::codex::CodexDelta;
        use chrono::NaiveDate;
        let d = |sid: &str, day: u32, input: u64, out: u64| CodexDelta {
            cwd: "/p/crab".into(),
            session_id: sid.into(),
            date: NaiveDate::from_ymd_opt(2026, 5, day).unwrap(),
            minute: 0,
            model: "gpt-5.5".into(),
            input,
            cache_read: 1000,
            output: out,
        };
        let deltas = vec![d("uuid-1", 4, 100, 50), d("uuid-1", 6, 10, 5), d("uuid-2", 5, 20, 10)];
        let mut names = std::collections::HashMap::new();
        names.insert("uuid-1".to_string(), "了解项目".to_string());
        let out = codex_session_summaries(&deltas, &names);
        let s1 = out.iter().find(|s| s.title == "了解项目").expect("named");
        assert_eq!(s1.provider, crate::config::discovery::Provider::Codex);
        assert_eq!(s1.cwd, "/p/crab");
        assert_eq!(s1.tokens, 165, "input+output across deltas");
        assert!(s1.cost > 0.0, "priced via cost_from_tokens (cache-inclusive)");
        assert_eq!(s1.last_date, NaiveDate::from_ymd_opt(2026, 5, 6).unwrap());
        // uuid-2 has no thread name → non-empty fallback label.
        assert!(out
            .iter()
            .any(|s| s.cwd == "/p/crab" && s.title != "了解项目" && !s.title.is_empty()));
    }

    #[test]
    fn parses_cwd_and_git_repository_url() {
        let raw = r#"{"timestamp":"2026-05-02T16:02:20Z","type":"session_meta","payload":{"id":"019de96d-2d14-7f40-a333-ead58534fe57","cwd":"/Users/xzy/workspace/crab","git":{"branch":"main","repository_url":"git@github.com:LucaXiang/Crab.git"}}}"#;
        let m = parse_session_meta(raw).expect("meta");
        assert_eq!(m.session_id, "019de96d-2d14-7f40-a333-ead58534fe57");
        assert_eq!(m.cwd, "/Users/xzy/workspace/crab");
        assert_eq!(m.repository_url.as_deref(), Some("git@github.com:LucaXiang/Crab.git"));
    }

    #[test]
    fn handles_session_with_no_git() {
        let raw = r#"{"type":"session_meta","payload":{"id":"abc","cwd":"/Users/xzy/.claude-mem/observer-sessions"}}"#;
        let m = parse_session_meta(raw).expect("meta");
        assert_eq!(m.cwd, "/Users/xzy/.claude-mem/observer-sessions");
        assert_eq!(m.repository_url, None);
    }

    #[test]
    fn none_when_no_session_meta() {
        let raw = r#"{"type":"event_msg","payload":{"type":"token_count"}}"#;
        assert!(parse_session_meta(raw).is_none());
    }

    #[test]
    fn collects_meta_from_a_dir_of_sessions() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ccmeter-codexmeta-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut f = std::fs::File::create(dir.join("s1.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"session_meta","payload":{{"id":"id1","cwd":"/p/crab","git":{{"repository_url":"git@github.com:LucaXiang/Crab.git"}}}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"token_count"}}}}"#).unwrap();
        let metas = collect_session_meta_in(&dir);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].cwd, "/p/crab");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
