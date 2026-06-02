//! Parse `session_meta` from Codex session files for project grouping:
//! cwd + git identity (repository_url, repo_root) + the session UUID.

use serde::Deserialize;
use std::path::Path;

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
pub(crate) fn collect_session_meta_in(dir: &Path) -> Vec<CodexSessionMeta> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
