//! Live parsing of OpenAI Codex CLI usage (~/.codex/sessions). Folded into the
//! daily cache under the `codex` root so it aggregates with Claude in "All".

pub mod parser;

use chrono::NaiveDate;

/// One per-turn token delta attributed to a (cwd, date, model).
#[derive(Debug, Clone, PartialEq)]
pub struct CodexDelta {
    pub cwd: String,
    pub date: NaiveDate,
    pub model: String,
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
}

use std::collections::HashSet;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::data::cache::Cache;
use crate::data::models::cost_from_tokens;

/// Synthetic source root holding all Codex usage.
pub const CODEX_ROOT: &str = "codex";

/// Discover + parse all Codex sessions and aggregate into a cache fragment
/// under `CODEX_ROOT`, plus the set of cwds seen (for the source selector).
pub fn load_codex_cache() -> (Cache, HashSet<String>) {
    let files = discover_session_files();
    let deltas: Vec<CodexDelta> = files
        .par_iter()
        .flat_map(|f| parser::parse_codex_file(f))
        .collect();
    aggregate(deltas)
}

fn discover_session_files() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let codex = home.join(".codex");
    let mut files = Vec::new();
    // Active sessions first, then archived. Codex moves rotated sessions into
    // `archived_sessions/`, which holds the bulk of historical usage — missing
    // it under-counts Codex by more than half.
    collect_jsonl(&codex.join("sessions"), &mut files);
    collect_jsonl(&codex.join("archived_sessions"), &mut files);
    dedup_by_filename(files)
}

/// Keep the first path for each unique file name (the session UUID). Guards
/// against a session briefly present in both `sessions/` and
/// `archived_sessions/` mid-archival; active is scanned first so it wins.
fn dedup_by_filename(files: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    files
        .into_iter()
        .filter(|p| match p.file_name() {
            Some(name) => seen.insert(name.to_os_string()),
            None => false,
        })
        .collect()
}

fn collect_jsonl(dir: &std::path::Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, files);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            files.push(path);
        }
    }
}

fn aggregate(deltas: Vec<CodexDelta>) -> (Cache, HashSet<String>) {
    let mut cache = Cache::new();
    let mut cwds = HashSet::new();
    for d in deltas {
        cwds.insert(d.cwd.clone());
        let entry = cache
            .entry_root(CODEX_ROOT.to_string())
            .entry(d.cwd.clone())
            .or_default()
            .entry(d.date.format("%Y-%m-%d").to_string())
            .or_default();
        entry.input += d.input;
        entry.output += d.output;
        entry.cache_read += d.cache_read;
        // `d.input` is fresh (cache-exclusive); cost_from_tokens wants a
        // cache-inclusive input (it derives fresh = input - cache_read), so
        // reconstruct it as fresh + cache_read. Pricing is unchanged — only
        // the stored `input` count became fresh.
        entry.cost += cost_from_tokens(&d.model, d.input + d.cache_read, d.output, d.cache_read, 0);
    }
    (cache, cwds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_deltas_into_cache_under_codex_root() {
        let deltas = vec![
            CodexDelta { cwd: "/p".into(), date: chrono::NaiveDate::from_ymd_opt(2026,5,4).unwrap(), model: "gpt-5.5".into(), input: 10, cache_read: 100, output: 5 },
            CodexDelta { cwd: "/p".into(), date: chrono::NaiveDate::from_ymd_opt(2026,5,4).unwrap(), model: "gpt-5.5".into(), input: 20, cache_read: 0, output: 15 },
        ];
        let (cache, cwds) = aggregate(deltas);
        let root = cache.get_root(CODEX_ROOT).unwrap();
        let e = &root["/p"]["2026-05-04"];
        assert_eq!(e.input, 30);
        assert_eq!(e.cache_read, 100);
        assert_eq!(e.output, 20);
        assert!(e.cost > 0.0);
        assert!(cwds.contains("/p"));
    }

    #[test]
    fn aggregate_cost_prices_input_as_fresh_plus_cache_read() {
        // CodexDelta.input is fresh (cache-exclusive). cost_from_tokens expects
        // a cache-INCLUSIVE input (it derives fresh = input - cache_read), so
        // aggregate must reconstruct input + cache_read — never pass the bare
        // fresh value (which would saturate fresh to 0 and under-charge).
        let date = chrono::NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
        let deltas = vec![CodexDelta {
            cwd: "/p".into(),
            date,
            model: "gpt-5.5".into(),
            input: 1000,
            cache_read: 4000,
            output: 200,
        }];
        let (cache, _) = aggregate(deltas);
        let e = &cache.get_root(CODEX_ROOT).unwrap()["/p"]["2026-05-04"];
        let expected = cost_from_tokens("gpt-5.5", 1000 + 4000, 200, 4000, 0);
        assert!((e.cost - expected).abs() < 1e-12, "got {}, want {}", e.cost, expected);
        assert!(e.cost > 0.0);
    }

    #[test]
    fn dedup_by_filename_keeps_first_occurrence() {
        // Same session UUID in both sessions/ and archived_sessions/: active
        // (scanned first) wins, the archived dup is dropped.
        let files = vec![
            PathBuf::from("/c/sessions/x.jsonl"),
            PathBuf::from("/c/archived_sessions/x.jsonl"),
            PathBuf::from("/c/archived_sessions/y.jsonl"),
        ];
        let out = dedup_by_filename(files);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], PathBuf::from("/c/sessions/x.jsonl"));
        assert!(out.iter().any(|p| p.ends_with("y.jsonl")));
    }
}
