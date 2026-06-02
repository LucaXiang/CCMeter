# Unified project cards — Phase 3 (Recent sessions with titles) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** In a project's detail view, list its recent sessions by human title — Codex `thread_name`, Claude `ai-title` — with tokens, cost, date, and provider (CC/CX). This is the original feature request #1, generalized to both providers.

**Architecture:** A single `SessionSummary { title, provider, cwd, tokens, cost, last_date }` value type. Claude summaries aggregate from the already-parsed `events` (grouped by `session_file`); titles come from a light per-file scan for the `ai-title` line. Codex summaries aggregate from `CodexDelta`s (gain a `session_id`); titles come from `~/.codex/session_index.jsonl` (`id → thread_name`). All summaries are computed on the background load thread and stored in `AppData`. `build_render_cache` already knows the selected project's cwds (`project_cwds`) and date filter, so it filters the summaries to the selected card, sorts by `last_date` desc, takes top-N, and hands them to `render_detail`, which draws a "Recent sessions" block.

**Tech Stack:** Rust 2021, ratatui. Build/test pinned: `cargo +1.95.0` (binary crate, no `--lib`).

**Spec:** `docs/superpowers/specs/2026-06-02-unified-project-cards-design.md` (Phase 3 / decision ④).

---

## File Structure
- `src/data/codex/mod.rs` — add `session_id: String` to `CodexDelta`.
- `src/data/codex/parser.rs` — capture `session_meta.payload.id` and stamp it on each delta.
- `src/data/codex/sessions.rs` — `read_thread_names() -> HashMap<String,String>` (parse `session_index.jsonl`); `codex_session_summaries(deltas, names) -> Vec<SessionSummary>`.
- `src/data/sessions.rs` (new) — the shared `SessionSummary` type; `claude_session_summaries(events, session_map, titles) -> Vec<SessionSummary>`; `scan_ai_titles(files) -> HashMap<String,String>` (basename → ai-title).
- `src/app.rs` — compute summaries in `load_data`, store `sessions: Vec<SessionSummary>` in `AppData`; in `build_render_cache` filter to the selected project and store `detail_sessions: Vec<SessionSummary>` in `RenderCache`.
- `src/ui/cards/render.rs` — `render_detail` draws the "Recent sessions" block.

---

## Task 1: `SessionSummary` type + Claude aggregation from events

**Files:**
- Create: `src/data/sessions.rs`
- Modify: `src/data/mod.rs` (add `pub mod sessions;`)

- [ ] **Step 1: Write the failing test**

```rust
//! Per-session rollups for the "Recent sessions" detail list. A SessionSummary
//! is provider-tagged (Claude/Codex) and carries the session's human title,
//! cwd (for grouping into a project card), token + cost totals, and last date.

use std::collections::HashMap;

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
    todo!()
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
        let s2 = out.iter().find(|s| s.cwd == "/p/crab" && s.title != "重新部署 dev-cloud").expect("s2");
        assert!(!s2.title.is_empty());
    }

    #[test]
    fn skips_events_with_unknown_session() {
        let events = vec![ev("ghost.jsonl", (2026, 5, 4), 5, 5, 0.1)];
        let out = claude_session_summaries(&events, &HashMap::new(), &HashMap::new());
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter sessions::tests` (add `pub mod sessions;` to `src/data/mod.rs` first). Expected: panic from `todo!()`.
NOTE: verify the `Event` field list in the test matches `src/data/parser.rs` exactly (it has `request_id: Option<String>` and possibly more fields after `session_file` — read the struct and fill ALL fields, using `..` is not allowed for a non-Default struct; set every field).

- [ ] **Step 3: Implement**

```rust
pub fn claude_session_summaries(
    events: &[Event],
    session_map: &HashMap<String, (String, String)>,
    titles: &HashMap<String, String>,
) -> Vec<SessionSummary> {
    struct Acc { tokens: u64, cost: f64, last: NaiveDate }
    let mut by_file: HashMap<&str, Acc> = HashMap::new();
    for e in events {
        let Some((_root, _cwd)) = session_map.get(&e.session_file) else { continue };
        let date = e.timestamp.with_timezone(&chrono::Local).date_naive();
        let acc = by_file.entry(e.session_file.as_str()).or_insert(Acc {
            tokens: 0, cost: 0.0, last: date,
        });
        acc.tokens += e.input_tokens + e.output_tokens;
        acc.cost += e.cost_usd;
        if date > acc.last { acc.last = date; }
    }
    by_file
        .into_iter()
        .map(|(file, acc)| {
            let cwd = session_map.get(file).map(|(_, c)| c.clone()).unwrap_or_default();
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter sessions::tests`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add src/data/sessions.rs src/data/mod.rs
git commit -m "feat(sessions): SessionSummary + Claude per-session aggregation"
```

---

## Task 2: Scan Claude session files for `ai-title`

**Files:**
- Modify: `src/data/sessions.rs`

- [ ] **Step 1: Write the failing test** (tmp files)

```rust
    #[test]
    fn scans_ai_title_lines() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ccmeter-aititle-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("abc.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, r#"{{"type":"user","cwd":"/p/crab"}}"#).unwrap();
        writeln!(f, r#"{{"type":"ai-title","aiTitle":"重新部署 dev-cloud","sessionId":"abc"}}"#).unwrap();
        let titles = scan_ai_titles(&[p.clone()]);
        assert_eq!(titles.get("abc.jsonl").map(String::as_str), Some("重新部署 dev-cloud"));
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter sessions::tests::scans_ai_title`
Expected: fail — `scan_ai_titles` undefined.

- [ ] **Step 3: Implement**

```rust
use std::path::{Path, PathBuf};

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
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
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
        if !line.contains("ai-title") { continue; }
        if let Ok(t) = serde_json::from_str::<TitleLine>(&line)
            && t.kind.as_deref() == Some("ai-title")
            && let Some(title) = t.ai_title.filter(|s| !s.is_empty())
        {
            return Some(title);
        }
    }
    None
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter sessions::tests`
Expected: PASS (all sessions tests).

- [ ] **Step 5: Commit**

```bash
git add src/data/sessions.rs
git commit -m "feat(sessions): scan Claude ai-title per session file"
```

---

## Task 3: Add `session_id` to `CodexDelta` and stamp it during parse

**Files:**
- Modify: `src/data/codex/mod.rs` (`CodexDelta` struct + its test constructors)
- Modify: `src/data/codex/parser.rs` (`parse_codex_str` captures `session_meta.payload.id`)

- [ ] **Step 1: Write the failing test** (extend parser.rs tests)

Add to `src/data/codex/parser.rs` tests:

```rust
    #[test]
    fn stamps_session_id_from_session_meta() {
        let raw = lines(&[
            r#"{"type":"session_meta","timestamp":"2026-05-04T10:00:00.000Z","payload":{"id":"uuid-1","cwd":"/proj"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-04T10:00:01.000Z","payload":{"model":"gpt-5.5"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-04T10:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":60}}}}"#,
        ]);
        let out = parse_codex_str(&raw, "f.jsonl");
        assert!(out.iter().all(|d| d.session_id == "uuid-1"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter codex::parser::tests::stamps_session_id`
Expected: compile error — `CodexDelta` has no `session_id`.

- [ ] **Step 3: Implement**

In `src/data/codex/mod.rs`, add to `CodexDelta` (after `cwd`):
```rust
    pub session_id: String,
```
Update the two test `CodexDelta { … }` literals in `mod.rs` tests and the `codex(...)` helper in `index.rs` tests to set `session_id: String::new()` (or a value) — find every `CodexDelta {` construction (`rg 'CodexDelta {'`) and add the field.

In `src/data/codex/parser.rs`:
- Add a `session_id: String` local (default empty) alongside `cwd`/`model`.
- In the `Payload` serde struct, add `id: Option<String>`.
- In the `Some("session_meta")` arm, set `session_id = p.id.clone().unwrap_or_default()` (and keep the cwd capture).
- In the `out.push(CodexDelta { … })`, add `session_id: session_id.clone(),`.

- [ ] **Step 4: Run tests to verify**

Run: `cargo +1.95.0 test --bin ccmeter codex::parser:: codex::tests:: index::` (all green — existing delta tests still pass with the new field).
Then `cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error'` (no errors).

- [ ] **Step 5: Commit**

```bash
git add src/data/codex/mod.rs src/data/codex/parser.rs src/data/index.rs
git commit -m "feat(codex): stamp session_id from session_meta onto deltas"
```

---

## Task 4: Codex thread names + Codex session summaries

**Files:**
- Modify: `src/data/codex/sessions.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn parses_thread_names_from_index() {
        let raw = [
            r#"{"id":"uuid-1","thread_name":"了解项目","updated_at":"2026-05-02T14:25:04Z"}"#,
            r#"{"id":"uuid-2","thread_name":"修复金额类型","updated_at":"2026-05-02T16:49:41Z"}"#,
        ].join("\n");
        let names = parse_thread_names(&raw);
        assert_eq!(names.get("uuid-1").map(String::as_str), Some("了解项目"));
        assert_eq!(names.get("uuid-2").map(String::as_str), Some("修复金额类型"));
    }

    #[test]
    fn summarizes_codex_sessions_with_thread_name() {
        use crate::data::codex::CodexDelta;
        use chrono::NaiveDate;
        let d = |sid: &str, day: u32, input: u64, out: u64| CodexDelta {
            cwd: "/p/crab".into(), session_id: sid.into(),
            date: NaiveDate::from_ymd_opt(2026, 5, day).unwrap(), minute: 0,
            model: "gpt-5.5".into(), input, cache_read: 1000, output: out,
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
        assert!(out.iter().any(|s| s.cwd == "/p/crab" && s.title != "了解项目" && !s.title.is_empty()));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter codex::sessions::tests::parses_thread_names codex::sessions::tests::summarizes_codex`
Expected: fail — functions undefined.

- [ ] **Step 3: Implement**

```rust
use std::collections::HashMap;
use crate::config::discovery::Provider;
use crate::data::models::cost_from_tokens;
use crate::data::sessions::{short_id, SessionSummary};

#[derive(serde::Deserialize)]
struct IndexLine { id: Option<String>, thread_name: Option<String> }

/// Parse `~/.codex/session_index.jsonl` contents → (session id → thread_name).
pub fn parse_thread_names(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in raw.lines() {
        let Ok(rec) = serde_json::from_str::<IndexLine>(line) else { continue };
        if let (Some(id), Some(name)) = (rec.id, rec.thread_name) {
            if !name.is_empty() { out.insert(id, name); }
        }
    }
    out
}

/// Read the session-name index from disk (empty if absent).
pub fn read_thread_names() -> HashMap<String, String> {
    let Some(home) = dirs::home_dir() else { return HashMap::new() };
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
    struct Acc { tokens: u64, cost: f64, last: chrono::NaiveDate, cwd: String }
    let mut by_sid: HashMap<&str, Acc> = HashMap::new();
    for d in deltas {
        if d.session_id.is_empty() { continue; }
        let cost = cost_from_tokens(&d.model, d.input + d.cache_read, d.output, d.cache_read, 0);
        let acc = by_sid.entry(d.session_id.as_str()).or_insert(Acc {
            tokens: 0, cost: 0.0, last: d.date, cwd: d.cwd.clone(),
        });
        acc.tokens += d.input + d.output;
        acc.cost += cost;
        if d.date > acc.last { acc.last = d.date; }
    }
    by_sid
        .into_iter()
        .map(|(sid, acc)| SessionSummary {
            title: names.get(sid).cloned().filter(|t| !t.is_empty()).unwrap_or_else(|| short_id(sid)),
            provider: Provider::Codex,
            cwd: acc.cwd,
            tokens: acc.tokens,
            cost: acc.cost,
            last_date: acc.last,
        })
        .collect()
}
```
(Make `SessionSummary` and `short_id` `pub(crate)` as needed — `short_id` is already `pub(crate)` from Task 1.)

- [ ] **Step 4: Run to verify**

Run: `cargo +1.95.0 test --bin ccmeter codex::sessions:: sessions::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/data/codex/sessions.rs
git commit -m "feat(codex): thread-name index + Codex session summaries"
```

---

## Task 5: Compute summaries on load; filter to selected project in render cache

**Files:**
- Modify: `src/app.rs` (`AppData`, `load_data`, `RenderCache`, `build_render_cache`)

- [ ] **Step 1: Thread the data (no new unit test — covered by Tasks 1-4 + a Task 6 probe)**

In `AppData` add:
```rust
    pub(crate) sessions: Vec<crate::data::sessions::SessionSummary>,
```
In `load_data`, after `let events = parser::parse_session_files(&all_session_files);` and after `let codex_deltas = crate::data::codex::collect_codex_deltas();`, compute:
```rust
    let ai_titles = crate::data::sessions::scan_ai_titles(&all_session_files);
    let mut sessions = crate::data::sessions::claude_session_summaries(&events, session_map, &ai_titles);
    let thread_names = crate::data::codex::sessions::read_thread_names();
    sessions.extend(crate::data::codex::sessions::codex_session_summaries(&codex_deltas, &thread_names));
```
`load_data` currently returns `(cache, index, state)`. Change it to also return `sessions` → `(cache, index, sessions, state)`, OR (less churn) store sessions via a field by returning a small struct. Update BOTH callers: `App::new` (destructure + put into `AppData`) and `spawn_reload` (the `ReloadResult` type + the reload handler that rebuilds `AppData`). Add `sessions` to `ReloadResult` (currently `(Cache, EventIndex)` → `(Cache, EventIndex, Vec<SessionSummary>)`) and assign it when the reload result is applied.

In `RenderCache` add:
```rust
    pub(crate) detail_sessions: Vec<crate::data::sessions::SessionSummary>,
```
In `build_render_cache` add a `sessions: &[SessionSummary]` parameter (thread it from the two call sites — `App::new` and `recompute_render_cache`, which both have `self.data.sessions`). Build `detail_sessions` only when a project is selected:
```rust
    let detail_sessions = match project_cwds {
        Some(cwds) => {
            let mut v: Vec<_> = sessions
                .iter()
                .filter(|s| cwds.contains(&s.cwd) && date_filter(s.last_date))
                .cloned()
                .collect();
            v.sort_by(|a, b| b.last_date.cmp(&a.last_date).then(b.cost.total_cmp(&a.cost)));
            v
        }
        None => Vec::new(),
    };
```
Add `detail_sessions` to the `RenderCache { … }` literal.

- [ ] **Step 2: Build**

Run: `cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'`
Expected: empty (resolve every call-site signature change). `detail_sessions` will warn "never read" until Task 6 — note it.

- [ ] **Step 3: Suite**

Run: `cargo +1.95.0 test 2>&1 | rg 'test result'`
Expected: only the 2 known date-relative `rate_limits` failures.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): compute session summaries on load; filter to detail view"
```

---

## Task 6: Render "Recent sessions" in the detail view

**Files:**
- Modify: `src/ui/cards/render.rs` (`render_detail` + a new `render_recent_sessions` helper)
- Modify: `src/ui/dashboard.rs` (pass `&self.render.detail_sessions` into `render_detail`)

- [ ] **Step 1: Read render_detail layout (lines ~952-973)**

It splits `inner` into `[Length(3) metrics, Length(1), Min(4) charts]`. Reserve space for the sessions list by splitting the charts row (`rows[2]`) vertically: charts on top (`Min(4)`), a sessions block at the bottom (`Length(h)`), where `h = (detail_sessions.len()+1).min(rows[2].height/2).min(8)` and only when `detail_sessions` is non-empty and `rows[2].height >= 8`.

- [ ] **Step 2: Implement (visual — verified via Task 6 probe + user)**

Change `render_detail` to accept `sessions: &[crate::data::sessions::SessionSummary]` (new last param). After computing `rows`, replace the single `render_detail_charts(... rows[2] ...)` with:
```rust
    if !sessions.is_empty() && rows[2].height >= 8 {
        let sess_h = ((sessions.len() as u16 + 1).min(rows[2].height / 2)).min(8);
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(sess_h)])
            .split(rows[2]);
        render_detail_charts(frame, split[0], card, granularity, range_start, range_end, minute_tokens, minute_model_costs);
        render_recent_sessions(frame, split[1], sessions);
    } else {
        render_detail_charts(frame, rows[2], card, granularity, range_start, range_end, minute_tokens, minute_model_costs);
    }
```
Add the helper (reuse `format_tokens`/`format_cost`, theme `model_color` for the provider tag):
```rust
fn render_recent_sessions(frame: &mut Frame, area: Rect, sessions: &[crate::data::sessions::SessionSummary]) {
    use crate::config::discovery::Provider;
    if area.height < 2 { return; }
    let t = theme();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Recent sessions",
            Style::default().fg(t.text_secondary).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let rows = area.height.saturating_sub(1) as usize;
    for (i, s) in sessions.iter().take(rows).enumerate() {
        let tag = match s.provider { Provider::Claude => "CC", Provider::Codex => "CX" };
        let tag_color = match s.provider {
            Provider::Claude => t.model_color("opus"),
            Provider::Codex => t.model_color("gpt-5.5"),
        };
        let date = s.last_date.format("%m-%d").to_string();
        // title gets the remaining width; tokens/cost/date right-aligned.
        let right = format!("{:>8} {:>8} {}", format_tokens(s.tokens), format_cost(s.cost), date);
        let title_w = (area.width as usize).saturating_sub(right.len() + 5).max(4);
        let mut title: String = s.title.chars().take(title_w).collect();
        while title.chars().count() < title_w { title.push(' '); }
        let line = Line::from(vec![
            Span::styled(format!("{tag} "), Style::default().fg(tag_color)),
            Span::styled(title, Style::default().fg(t.text_primary)),
            Span::styled(format!(" {right}"), Style::default().fg(t.text_dim)),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x + 1, area.y + 1 + i as u16, area.width.saturating_sub(1), 1),
        );
    }
}
```

In `src/ui/dashboard.rs`, pass `&self.render.detail_sessions` as the new last arg to the `render_detail(...)` call (~line 410).

- [ ] **Step 3: Build + suite + clippy**

Run: `cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'` (empty — `detail_sessions` now read).
Run: `cargo +1.95.0 test 2>&1 | rg 'test result'` (only the 2 known failures).
Run: `cargo +1.95.0 clippy --bin ccmeter 2>&1 | rg 'generated'` (not above 11).

- [ ] **Step 4: Real-data probe (orchestrator)**

Temporary `#[ignore]` probe: build summaries from real data (or assert via a small harness) and print the crab project's recent sessions (titles + tokens + cost + provider). Confirm Codex thread_names and Claude ai-titles appear. Revert the probe.

- [ ] **Step 5: Commit**

```bash
git add src/ui/cards/render.rs src/ui/dashboard.rs
git commit -m "feat(cards): Recent sessions list in the detail view"
```

---

## Phase 3 acceptance
- [ ] Selecting a project (e.g. crab) shows a "Recent sessions" list: title · tokens · cost · date · CC/CX, newest first.
- [ ] Codex rows use `thread_name`; Claude rows use `ai-title`; titleless sessions fall back to a short id (non-empty).
- [ ] Sessions are filtered to the selected project's cwds and the active date range.
- [ ] Detail view never overflows (sessions block is height-guarded; charts keep ≥4 rows).
- [ ] `build --bin ccmeter` clean; `test` only the 2 known failures; clippy not above 11.
