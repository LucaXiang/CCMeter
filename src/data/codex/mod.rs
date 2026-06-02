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
    let root = home.join(".codex").join("sessions");
    let mut files = Vec::new();
    collect_jsonl(&root, &mut files);
    files
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
        entry.cost += cost_from_tokens(&d.model, d.input, d.output, d.cache_read, 0);
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
}
