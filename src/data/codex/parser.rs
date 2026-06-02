//! Parse a single Codex session file. `total_token_usage` is cumulative per
//! session, so we deltaize; on a cumulative rollback we fall back to
//! `last_token_usage`. Model comes from the latest `turn_context`, cwd from
//! `session_meta`. Mirrors slopmeter's codex.ts handling.

use std::path::Path;

use chrono::{DateTime, NaiveDate, Timelike};
use serde::Deserialize;

use super::CodexDelta;

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    info: Option<Info>,
}

#[derive(Deserialize)]
struct Info {
    total_token_usage: Option<Usage>,
    last_token_usage: Option<Usage>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl Usage {
    fn sum(&self) -> u64 {
        self.input_tokens + self.cached_input_tokens + self.output_tokens + self.reasoning_output_tokens
    }
}

pub fn parse_codex_file(path: &Path) -> Vec<CodexDelta> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse_codex_str(&s, &path.file_name().unwrap_or_default().to_string_lossy()),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn parse_codex_str(raw: &str, _session_file: &str) -> Vec<CodexDelta> {
    let mut cwd: Option<String> = None;
    let mut model = String::new();
    let mut prev = Usage::default();
    let mut out = Vec::new();

    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Line>(line) else {
            continue;
        };
        let payload = rec.payload;
        match rec.kind.as_deref() {
            Some("session_meta") => {
                if let Some(p) = &payload {
                    if let Some(c) = &p.cwd {
                        cwd = Some(c.clone());
                    }
                }
            }
            Some("turn_context") => {
                if let Some(p) = &payload {
                    if let Some(c) = &p.cwd {
                        cwd.get_or_insert_with(|| c.clone());
                    }
                    if let Some(m) = &p.model {
                        model = m.clone();
                    }
                }
            }
            Some("event_msg") => {
                let Some(p) = &payload else { continue };
                if p.kind.as_deref() != Some("token_count") {
                    continue;
                }
                let Some(info) = &p.info else { continue };
                let Some(total) = info.total_token_usage else { continue };
                let rolled_back = total.total_tokens < prev.total_tokens
                    || total.input_tokens < prev.input_tokens;
                let delta = if rolled_back {
                    info.last_token_usage.unwrap_or(total)
                } else {
                    Usage {
                        input_tokens: total.input_tokens.saturating_sub(prev.input_tokens),
                        cached_input_tokens: total
                            .cached_input_tokens
                            .saturating_sub(prev.cached_input_tokens),
                        output_tokens: total.output_tokens.saturating_sub(prev.output_tokens),
                        reasoning_output_tokens: total
                            .reasoning_output_tokens
                            .saturating_sub(prev.reasoning_output_tokens),
                        total_tokens: total.total_tokens.saturating_sub(prev.total_tokens),
                    }
                };
                prev = total;

                if delta.sum() == 0 {
                    continue;
                }
                let (Some(cwd), Some(ts)) = (&cwd, rec.timestamp.as_deref()) else {
                    continue;
                };
                let Ok(parsed) = DateTime::parse_from_rfc3339(ts) else {
                    continue;
                };
                let local = parsed.with_timezone(&chrono::Local);
                let date: NaiveDate = local.date_naive();
                let minute = local.hour() as u16 * 60 + local.minute() as u16;
                out.push(CodexDelta {
                    cwd: cwd.clone(),
                    date,
                    minute,
                    model: model.clone(),
                    // Codex reports cache-inclusive input (cached ⊆ input);
                    // store the fresh, cache-exclusive portion so `input`
                    // matches Claude's already-fresh API input_tokens and the
                    // combined view isn't inflated by re-read cache each turn.
                    input: delta.input_tokens.saturating_sub(delta.cached_input_tokens),
                    cache_read: delta.cached_input_tokens,
                    output: delta.output_tokens,
                });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String { v.join("\n") }

    #[test]
    fn deltaizes_cumulative_usage_and_tracks_model_and_cwd() {
        // Codex's `input_tokens` is cache-INCLUSIVE (OpenAI convention:
        // total_tokens == input_tokens + output_tokens, cached ⊆ input). We
        // store the fresh, cache-exclusive portion so `input` means the same
        // thing as for Claude (whose API input_tokens is already fresh).
        // E1: input 110 (incl 100 cached) → fresh 10; E2 cumulative input 230
        // (incl 200 cached) → delta fresh 20. Totals: fresh 30, cache 200.
        let raw = lines(&[
            r#"{"type":"session_meta","timestamp":"2026-05-04T10:00:00.000Z","payload":{"cwd":"/proj"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-04T10:00:01.000Z","payload":{"model":"gpt-5.5"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":110,"cached_input_tokens":100,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":120}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":230,"cached_input_tokens":200,"output_tokens":30,"reasoning_output_tokens":5,"total_tokens":260}}}}"#,
        ]);
        let out = parse_codex_str(&raw, "f.jsonl");
        let total_in: u64 = out.iter().map(|d| d.input).sum();
        let total_cr: u64 = out.iter().map(|d| d.cache_read).sum();
        let total_out: u64 = out.iter().map(|d| d.output).sum();
        assert_eq!(total_in, 30, "input must be fresh (cache-exclusive)");
        assert_eq!(total_cr, 200);
        assert_eq!(total_out, 30);
        assert!(out.iter().all(|d| d.cwd == "/proj" && d.model == "gpt-5.5"));
    }

    #[test]
    fn rollback_uses_last_usage() {
        // Rollback path must also split fresh from cache. E1 input 100
        // (incl 40 cached) → fresh 60. E2 rolls back (total 22 < 150) so the
        // delta is `last_token_usage` input 20 (incl 8 cached) → fresh 12.
        let raw = lines(&[
            r#"{"type":"session_meta","timestamp":"2026-05-04T10:00:00.000Z","payload":{"cwd":"/p"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-04T10:00:01.000Z","payload":{"model":"gpt-5.5"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"cached_input_tokens":8,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":22},"last_token_usage":{"input_tokens":20,"cached_input_tokens":8,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":22}}}}"#,
        ]);
        let out = parse_codex_str(&raw, "f.jsonl");
        let total_in: u64 = out.iter().map(|d| d.input).sum();
        let total_cr: u64 = out.iter().map(|d| d.cache_read).sum();
        let total_out: u64 = out.iter().map(|d| d.output).sum();
        assert_eq!(total_in, 72, "fresh = 60 + 12");
        assert_eq!(total_cr, 48);
        assert_eq!(total_out, 52);
    }

    #[test]
    fn captures_minute_of_day_from_timestamp() {
        use chrono::Timelike;
        let ts = "2026-05-04T10:37:00.000Z";
        let raw = lines(&[
            r#"{"type":"session_meta","timestamp":"2026-05-04T10:00:00.000Z","payload":{"cwd":"/p"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-04T10:00:01.000Z","payload":{"model":"gpt-5.5"}}"#,
            &format!(r#"{{"type":"event_msg","timestamp":"{ts}","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":50,"cached_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":60}}}}}}}}"#),
        ]);
        let out = parse_codex_str(&raw, "f.jsonl");
        // Compare against the same Local conversion the parser uses so the
        // assertion is timezone-independent.
        let dt = DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&chrono::Local);
        let expected = dt.hour() as u16 * 60 + dt.minute() as u16;
        assert_eq!(out[0].minute, expected);
    }

    #[test]
    fn ignores_non_token_and_missing_cwd() {
        let raw = r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0,"total_tokens":15}}}}"#;
        assert!(parse_codex_str(raw, "f.jsonl").is_empty());
    }
}
