# Phase B+C: Codex provider (live) + combined provider view — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Parse OpenAI Codex CLI usage (`~/.codex/sessions/**/*.jsonl`) live on every load, fold it into CCMeter's daily cache under a `codex` source root so it aggregates with Claude Code in the "All" view, and make the source selector provider-aware ("All / Claude Code / Codex").

**Architecture:** A new `src/data/codex/` module discovers + parses Codex sessions into per-(cwd, date) `DayEntry` totals under a synthetic `codex` root, wired into `app.rs::load_data` alongside the Claude pipeline (live; re-parsed each reload; merged + persisted like Claude). Cost uses a new OpenAI pricing table. The source selector is made provider-aware: cache filtering accepts a SET of roots (`Option<&[String]>`) so "Claude Code" = all non-codex roots (real + `backfill:*`) and "Codex" = the codex root.

**Tech Stack:** Rust 2024, serde_json, chrono, rayon (existing). Reuses Phase A's `cache`, `DayEntry`, `models::cost_from_tokens`.

**Decisions (locked with user):** live integration (not CLI backfill); estimate Codex cost via a best-effort OpenAI pricing table; reuse the source selector for the provider toggle.

---

## Codex data format (verified on disk)

Each `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`:
- line 1 `{"type":"session_meta","payload":{"cwd":"…","model_provider":"openai",…}}` — gives the session cwd.
- `{"type":"turn_context","payload":{"model":"gpt-5.5","cwd":"…","current_date":"…",…}}` — model for subsequent turns (track latest).
- `{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{input_tokens,cached_input_tokens,output_tokens,reasoning_output_tokens,total_tokens}, "last_token_usage":{…}}}}` — `total_token_usage` is CUMULATIVE per session.

Models seen: `gpt-5.5`, `gpt-5.3-codex`, `codex-auto-review`.

**Token mapping → `DayEntry`:** `input += input_tokens`, `cache_read += cached_input_tokens`, `output += output_tokens + reasoning_output_tokens`, `cache_creation = 0`. Deltaize `total_token_usage` (subtract previous cumulative; on rollback use `last_token_usage`), mirroring slopmeter's `codex.ts`.

## File structure

| File | Responsibility |
|---|---|
| `src/data/models.rs` (mod) | OpenAI/Codex pricing patterns + a `is_codex_model`/normalize helper |
| `src/data/codex/mod.rs` (new) | `CODEX_ROOT` const; `load_codex_cache()` → `(Cache fragment, HashSet<cwd>)`; discovery of session files |
| `src/data/codex/parser.rs` (new) | parse one session file → `Vec<CodexDelta{cwd,date,model,input,cache_read,output}>` |
| `src/data/mod.rs` (mod) | register `pub mod codex;` |
| `src/app.rs` (mod) | wire codex into `load_data`; thread codex cwds into the source list; provider-aware source entries |
| `src/data/cache.rs` (mod) | `iter_filtered`/`to_daily_tokens_filtered` accept `Option<&[String]>` (set of roots) |

---

## Task 1: OpenAI/Codex pricing (models.rs)

**Files:** Modify `src/data/models.rs`.

- [ ] **Step 1 — failing test** (add to the existing `#[cfg(test)] mod tests` in models.rs):
```rust
    #[test]
    fn codex_models_are_priced_and_detected() {
        assert!(is_codex_model("gpt-5.5"));
        assert!(is_codex_model("gpt-5.3-codex"));
        assert!(!is_codex_model("claude-opus-4-6"));
        // gpt-5 priced (non-fallback): a pure-output cost is nonzero and
        // distinct from the claude fallback for the same tokens.
        let c = cost_from_tokens("gpt-5.5", 0, 1_000_000, 0, 0);
        assert!(c > 0.0);
    }
```

- [ ] **Step 2 — run, verify FAIL**: `cargo test models::tests::codex_models_are_priced_and_detected` → `cannot find function 'is_codex_model'`.

- [ ] **Step 3 — implement.** In `src/data/models.rs`, ADD `gpt`/`codex` entries to `PRICING_TABLE` (best-effort OpenAI estimates, per million tokens, `(input, output, cache_read)`) — insert these rows at the TOP of the table so they match before any claude pattern:
```rust
    ("gpt-5", (1.25, 10.0, 0.125)),
    ("codex", (1.25, 10.0, 0.125)),
```
(Note: `model_pricing` matches by `model.contains(pattern)`; "gpt-5.5"/"gpt-5.3-codex" contain "gpt-5"; "codex-auto-review" contains "codex".) Then ADD a detector after `model_pricing`:
```rust
/// True for OpenAI Codex CLI models (priced via the OpenAI estimate rows).
pub(crate) fn is_codex_model(model: &str) -> bool {
    model.contains("gpt-") || model.contains("codex")
}
```

- [ ] **Step 4 — run, verify PASS**: `cargo test models::tests`.
- [ ] **Step 5 — commit**: `git add src/data/models.rs && git commit -m "feat(codex): add OpenAI pricing estimates + is_codex_model"`

---

## Task 2: Codex session parser (codex/parser.rs)

**Files:** Create `src/data/codex/parser.rs`; register module in Task 3's mod.rs (Task 3 creates mod.rs — for THIS task, also create a minimal `src/data/codex/mod.rs` containing `pub mod parser;` + the shared `CodexDelta` struct, and register `pub mod codex;` in `src/data/mod.rs`).

- [ ] **Step 1 — scaffold + failing test.**
Create `src/data/mod.rs` addition: `pub mod codex;`.
Create `src/data/codex/mod.rs`:
```rust
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
```
Create `src/data/codex/parser.rs` with this test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String { v.join("\n") }

    #[test]
    fn deltaizes_cumulative_usage_and_tracks_model_and_cwd() {
        let raw = lines(&[
            r#"{"type":"session_meta","timestamp":"2026-05-04T10:00:00.000Z","payload":{"cwd":"/proj"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-04T10:00:01.000Z","payload":{"model":"gpt-5.5"}}"#,
            // cumulative: in10 cached100 out5 reason5
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":100,"output_tokens":5,"reasoning_output_tokens":5,"total_tokens":120}}}}"#,
            // cumulative grows: in30 cached100 out20 reason10  -> delta in20 cached0 out15(=10+5)
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"cached_input_tokens":100,"output_tokens":20,"reasoning_output_tokens":10,"total_tokens":160}}}}"#,
        ]);
        let out = parse_codex_str(&raw, "f.jsonl");
        // two token_count events -> two deltas, same (cwd,date,model)
        let total_in: u64 = out.iter().map(|d| d.input).sum();
        let total_cr: u64 = out.iter().map(|d| d.cache_read).sum();
        let total_out: u64 = out.iter().map(|d| d.output).sum();
        assert_eq!(total_in, 30);       // 10 + 20
        assert_eq!(total_cr, 100);      // 100 + 0
        assert_eq!(total_out, 30);      // (5+5) + (15+5)? -> out: 10 then 20 = 30
        assert!(out.iter().all(|d| d.cwd == "/proj" && d.model == "gpt-5.5"));
    }

    #[test]
    fn rollback_uses_last_usage() {
        let raw = lines(&[
            r#"{"type":"session_meta","timestamp":"2026-05-04T10:00:00.000Z","payload":{"cwd":"/p"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-04T10:00:01.000Z","payload":{"model":"gpt-5.5"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#,
            // rollback (new session within file): cumulative resets below prev -> use last_token_usage
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":7},"last_token_usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":7}}}}"#,
        ]);
        let out = parse_codex_str(&raw, "f.jsonl");
        let total_in: u64 = out.iter().map(|d| d.input).sum();
        let total_out: u64 = out.iter().map(|d| d.output).sum();
        assert_eq!(total_in, 105); // 100 + 5(from last)
        assert_eq!(total_out, 52); // 50 + 2
    }

    #[test]
    fn ignores_non_token_and_missing_cwd() {
        // no session_meta cwd -> skip (cannot attribute to a project)
        let raw = r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0,"total_tokens":15}}}}"#;
        assert!(parse_codex_str(raw, "f.jsonl").is_empty());
    }
}
```

- [ ] **Step 2 — run, verify FAIL**: `cargo test codex::parser` → `cannot find function 'parse_codex_str'`.

- [ ] **Step 3 — implement** `src/data/codex/parser.rs` (above the test module):
```rust
//! Parse a single Codex session file. `total_token_usage` is cumulative per
//! session, so we deltaize; on a cumulative rollback we fall back to
//! `last_token_usage`. Model comes from the latest `turn_context`, cwd from
//! `session_meta`. Mirrors slopmeter's codex.ts handling.

use std::path::Path;

use chrono::{DateTime, NaiveDate};
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
                // Delta vs previous cumulative; rollback -> last_token_usage.
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
                let date: NaiveDate = parsed.with_timezone(&chrono::Local).date_naive();
                out.push(CodexDelta {
                    cwd: cwd.clone(),
                    date,
                    model: model.clone(),
                    input: delta.input_tokens,
                    cache_read: delta.cached_input_tokens,
                    output: delta.output_tokens + delta.reasoning_output_tokens,
                });
            }
            _ => {}
        }
    }
    out
}
```

- [ ] **Step 4 — run, verify PASS**: `cargo test codex::parser` (3 tests).
- [ ] **Step 5 — commit**: `git add src/data/mod.rs src/data/codex/ && git commit -m "feat(codex): parse session files into per-turn token deltas"`

---

## Task 3: Codex aggregation → cache fragment (codex/mod.rs)

**Files:** Modify `src/data/codex/mod.rs`.

- [ ] **Step 1 — failing test** (add a test module to `src/data/codex/mod.rs`):
```rust
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
```

- [ ] **Step 2 — run, verify FAIL**: `cargo test codex::tests::aggregates_deltas_into_cache_under_codex_root` → `cannot find function 'aggregate'`.

- [ ] **Step 3 — implement.** Add to `src/data/codex/mod.rs`:
```rust
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
```

- [ ] **Step 4 — run, verify PASS**: `cargo test codex::tests`.
- [ ] **Step 5 — commit**: `git add src/data/codex/mod.rs && git commit -m "feat(codex): aggregate deltas into a cache fragment under the codex root"`

---

## Task 4: Live wire into `load_data` (app.rs)

**Files:** Modify `src/app.rs`.

CONTEXT for the implementer: `load_data(raw_groups, session_map) -> (cache::Cache, EventIndex, cache::CacheLoad)` builds the Claude cache: parses session files, `cache::from_events`, loads on-disk cache, `cache::merge(outcome.cache, &fresh_cache)`, `cache::save(&merged)`, builds the index. You will fold the Codex cache into `merged` BEFORE saving, so Codex persists and aggregates. Read the current `load_data` (around app.rs:902-920) and `cache::merge` signature first.

- [ ] **Step 1 — implement.** In `src/app.rs::load_data`, after `let fresh_cache = cache::from_events(&events, session_map);` and the existing `let merged = cache::merge(outcome.cache, &fresh_cache);`, insert Codex folding before `cache::save(&merged)`:
```rust
    let (codex_cache, _codex_cwds) = crate::data::codex::load_codex_cache();
    let merged = cache::merge(merged, &codex_cache);
```
So the sequence becomes: load on-disk → merge fresh Claude → merge Codex → save. (Codex lives under the `codex` root; `merge` is high-water-mark per (root,cwd,date), so re-runs converge. Codex never collides with Claude/backfill roots.)

- [ ] **Step 2 — build + manual verify.** `cargo build`. Then run the binary's backfill-free path can't show this; instead verify via a tiny temporary check OR trust the merge + existing aggregation: `to_daily_tokens_filtered(None, None)` already sums all roots, so the `codex` root flows into the "All" heatmap/KPI/cost. Confirm the build is clean and that `load_codex_cache` is invoked (no dead_code warning for it).

- [ ] **Step 3 — commit**: `git add src/app.rs && git commit -m "feat(codex): fold live Codex usage into the cache on every load"`

---

## Task 5: Cache filter accepts a set of roots (cache.rs)

**Files:** Modify `src/data/cache.rs` and all call sites.

CONTEXT: today `iter_filtered(source_root: Option<&str>, project_cwds: Option<&[String]>)` and `to_daily_tokens_filtered(cache, source_root: Option<&str>, project_cwds)` filter by a SINGLE root string. To support a provider = a SET of roots ("Claude Code" = every non-codex root), change the root parameter to `Option<&[String]>` (None = all; Some(list) = root must be in list). The implementer MUST read the current signatures and update EVERY call site (in `app.rs`: `compute_daily_and_thresholds`, `to_daily_tokens_filtered` calls; anywhere passing `source_root`).

- [ ] **Step 1 — change `iter_filtered`.** In `src/data/cache.rs`, change the `source_root: Option<&'a str>` parameter of `iter_filtered` to `source_roots: Option<&'a [String]>`, and the match from `source_root.is_none_or(|sr| sr == root)` to `source_roots.is_none_or(|rs| rs.iter().any(|r| r == root))`. Update `to_daily_tokens_filtered`'s signature the same way and pass through.

- [ ] **Step 2 — update call sites in app.rs.** `compute_daily_and_thresholds(cache, source_root: Option<&str>, ...)` → change to `Option<&[String]>` and thread through. At the App callers, where `self.config.source_roots[self.source_index].as_deref()` produced `Option<&str>`, adapt to produce `Option<&[String]>` (Task 6 defines the new source-entry type; for THIS task, keep it compiling by wrapping the existing single root in a slice or temporarily adjust — coordinate with Task 6). To keep this task self-contained and green, update `compute_daily_and_thresholds` + `to_daily_tokens_filtered` signatures and fix callers to pass `source_root.map(std::slice::from_ref)` where a single `String` is available.

- [ ] **Step 2b — keep the existing cache tests green.** Update `to_daily_tokens_filtered(&cache, Some("root_a"), None)` style tests in cache.rs to `Some(&["root_a".to_string()][..])` (or a local binding). Run `cargo test cache`.

- [ ] **Step 3 — build + test**: `cargo build` and `cargo test cache` green.
- [ ] **Step 4 — commit**: `git add src/data/cache.rs src/app.rs && git commit -m "refactor(cache): filter daily totals by a set of roots (provider-ready)"`

---

## Task 6: Provider-aware source selector (app.rs)

**Files:** Modify `src/app.rs` (`build_source_list` and the source-filter plumbing).

CONTEXT: `build_source_list(root_map) -> (Vec<String> names, Vec<Option<String>> root_keys)` builds the ⇧Tab source selector. Today entries are All + one per Claude root. Make it provider-aware. The provider of a root: `codex` → "Codex"; everything else (real Claude roots + `backfill:*`) → "Claude Code". The `index` source-root parameter stays `Option<&str>` (the index is Claude-only).

- [ ] **Step 1 — change the source-entry representation.** Replace `source_roots: Vec<Option<String>>` with a richer per-entry filter. Define near the top of app.rs:
```rust
/// A selectable source: which cache roots it includes, and the index root
/// filter (the EventIndex holds Claude data only).
#[derive(Clone)]
pub(crate) struct SourceEntry {
    pub(crate) name: String,
    pub(crate) cache_roots: Option<Vec<String>>, // None = all roots
    pub(crate) index_root: Option<String>,       // for index-derived stats
}
```
Replace `source_names: Vec<String>` + `source_roots: Vec<Option<String>>` in `AppConfig` with `sources: Vec<SourceEntry>` (update all references: the selector uses `self.config.sources[self.source_index].name` for display, `.cache_roots` for cache filtering, `.index_root` for index filtering). `source_names.len()` usages become `self.config.sources.len()`.

- [ ] **Step 2 — rebuild `build_source_list`.** Replace it with a function that, given the Claude `root_map` AND the codex cwds (thread `codex_cwds` from `load_codex_cache`; simplest: have `App::new`/`apply_discovery_result` call `crate::data::codex::load_codex_cache()` once for the cwd set, or detect codex presence by checking the merged cache for the `codex` root), returns `Vec<SourceEntry>`:
  - Always: `SourceEntry { name: "All", cache_roots: None, index_root: None }`.
  - If any Claude data: `SourceEntry { name: "Claude Code", cache_roots: Some(<all non-codex roots present>), index_root: None }`. Compute non-codex roots from `merged_cache.roots()` filtered to `root != CODEX_ROOT`.
  - If codex data present (codex root in cache, or non-empty codex cwds): `SourceEntry { name: "Codex", cache_roots: Some(vec![CODEX_ROOT.to_string()]), index_root: Some(CODEX_ROOT.to_string()) }`.
  - Collapse to just `[All]` when there's only one provider with data (preserve today's single-source behavior).

Because this needs `merged_cache`, compute the source list AFTER the cache is built in `App::new` (and recompute in `apply_discovery_result` after the reload merges codex). Pass `&merged_cache` into the builder.

- [ ] **Step 3 — thread index_root.** Where the code currently passes `self.config.source_roots[self.source_index].as_deref()` to index methods (`build_model_stats`, `build_minute_tokens`, etc.), pass `self.config.sources[self.source_index].index_root.as_deref()`. Where it passes to cache (`compute_daily_and_thresholds`), pass `self.config.sources[self.source_index].cache_roots.as_deref()`.

- [ ] **Step 4 — build + manual verify.** `cargo build` clean. Reason through: selecting "Codex" filters cache to the codex root (cards/heatmap/KPI show Codex) and the index to `codex` (no Claude index data → empty per-model split, expected). "Claude Code" includes real + backfill roots (full Claude history). "All" = everything.

- [ ] **Step 5 — commit**: `git add src/app.rs && git commit -m "feat(codex): provider-aware source selector (All / Claude Code / Codex)"`

---

## Self-Review

- **Spec coverage (spec §4 Codex, §5 combined):** parser→T2, pricing→T1, aggregation→T3, live integration→T4, provider dimension via source roots→T5+T6, UI toggle→T6.
- **No double-count:** Codex lives under `CODEX_ROOT`, disjoint from Claude real roots and `backfill:*`; `merge` is per-root max; Codex tokens are genuinely additional usage, so summing in "All" is correct.
- **Type consistency:** `CodexDelta{cwd,date,model,input,cache_read,output}`, `CODEX_ROOT`, `load_codex_cache()->(Cache,HashSet<String>)`, `is_codex_model`, `cost_from_tokens` (Phase A), `SourceEntry{name,cache_roots,index_root}` consistent across tasks.
- **Known v1 limits (acceptable):** Codex cost is an OpenAI ESTIMATE; Codex has no per-model card split (EventIndex is Claude-only); Codex `cache_creation` always 0.

## Acceptance

- [ ] `cargo build` clean; `cargo test codex` + `models` + `cache` pass (the 2 pre-existing `rate_limits` date tests remain unrelated).
- [ ] After launch, "All" heatmap/KPI/cost include Codex usage; ⇧Tab cycles All / Claude Code / Codex; "Codex" isolates Codex projects.
- [ ] Codex usage persists in `history.json` under the `codex` root and re-parses live each load.
