# Phase A: Claude Code 全历史回填 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `ccmeter backfill` 从 `~/.claude/stats-cache.json` 与 `~/.code-insights/data.db` 回填历史 token/成本,合并进 `~/.config/ccmeter/history.json`,使 TUI 的 `All` 视图能展示回溯至 2026-01-01 的用量。

**Architecture:** 新增隔离的 `src/data/backfill/` 模块。三源**按日期分段取最全、互不重叠**(实时 JSONL ≥ boundary / Code Insights / stats-cache),产出合成 `source_root`(`backfill:*`)的缓存条目。每次回填**先清空 `backfill:*` 根再重写**,保证幂等且不与实时数据重复计数。现有实时管线、缓存 merge、UI 聚合(`to_daily_tokens_filtered(None,None)`)无需改动即可纳入这些历史条目。

**Tech Stack:** Rust 2024、clap(已用)、serde_json(已用)、chrono(已用)、新增 `rusqlite`(bundled,读 Code Insights SQLite)。

---

## v1 范围与刻意推迟项(YAGNI)

- **本计划做**:历史每日 `input/output/cache_read/cache_creation` token + `cost`(按 CCMeter 定价从 token 重算)。
- **推迟到后续**(置 0 并在 §Task 8 注脚标注):历史日的 `lines_*`(`lines_accepted` 本就无源)、`active_minutes`、逐模型细分(`DayEntry` 无 model 维度)。理由:用户首要诉求是"看到完整历史"(token/成本/热力图),行数与活跃时长是次要打磨,且复原成本高。spec §3.2 中这些字段的复原列为 follow-up。

## 文件结构

| 文件 | 职责 |
|---|---|
| `Cargo.toml` (改) | 新增 `rusqlite` 依赖 |
| `src/data/mod.rs` (改) | 注册 `pub mod backfill;` |
| `src/data/models.rs` (改) | 新增共享 `cost_from_tokens`(供回填与 parser 复用) |
| `src/data/cache.rs` (改) | 新增 `Cache::remove_root` |
| `src/data/backfill/mod.rs` (新) | 常量、`BackfillDay`、`run_backfill` 编排、`BackfillOptions`/`BackfillSummary` |
| `src/data/backfill/stats_cache.rs` (新) | 解析 stats-cache.json → `Vec<BackfillDay>` |
| `src/data/backfill/code_insights.rs` (新) | 读 Code Insights SQLite → `Vec<BackfillDay>` |
| `src/data/backfill/layering.rs` (新) | 按日期 boundary 分段去重 |
| `src/main.rs` (改) | `ccmeter backfill` 子命令 |
| `src/ui/...` (改) | 历史粗粒度注脚(Task 8) |

---

## Task 1: 共享 `cost_from_tokens`(models.rs)

**Files:**
- Modify: `src/data/models.rs`
- Test: 同文件 `#[cfg(test)]`

- [ ] **Step 1: 写失败测试**

在 `src/data/models.rs` 末尾的 `#[cfg(test)] mod tests` 中(若无则新建)加入:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_from_tokens_matches_pricing() {
        // opus-4-6: input $5/M, output $25/M, cache_read $0.5/M,
        // cache_creation = input * 1.25 = $6.25/M.
        // fresh_input 500*5 + cache_read 500*0.5 + cache_creation 300*5*1.25
        //   + output 200*25 = 2500 + 250 + 1875 + 5000 = 9625 micro-USD
        let c = cost_from_tokens("claude-opus-4-6", 1000, 200, 500, 300);
        assert!((c - 0.009625).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn cost_from_tokens_zero_is_zero() {
        assert_eq!(cost_from_tokens("claude-opus-4-6", 0, 0, 0, 0), 0.0);
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test --lib models::tests::cost_from_tokens_matches_pricing`
Expected: 编译失败 `cannot find function 'cost_from_tokens'`

- [ ] **Step 3: 实现**

在 `src/data/models.rs` 的 `model_pricing` 之后加入(注意 `input` 表示包含 cache_read 的"总输入",`fresh_input = input - cache_read`,与 `parser.rs::cost_from_tokens` 语义一致):

```rust
/// Cost (USD) from delta token counts via the local pricing table.
/// Mirrors Anthropic billing: fresh input at input_price, cache reads at
/// cache_read_price, cache creation at input_price * 1.25, output at
/// output_price. `input` is the total input (cache_read inclusive).
pub(crate) fn cost_from_tokens(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
) -> f64 {
    const CACHE_CREATION_MULTIPLIER: f64 = 1.25;
    let (input_price, output_price, cache_read_price) = model_pricing(model);
    let fresh_input = input.saturating_sub(cache_read);
    (fresh_input as f64 * input_price
        + cache_read as f64 * cache_read_price
        + cache_creation as f64 * input_price * CACHE_CREATION_MULTIPLIER
        + output as f64 * output_price)
        / TOKENS_PER_MILLION
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test --lib models::tests`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/data/models.rs
git commit -m "feat(backfill): add shared cost_from_tokens to models"
```

---

## Task 2: 依赖 + 模块骨架 + `Cache::remove_root`

**Files:**
- Modify: `Cargo.toml`, `src/data/mod.rs`, `src/data/cache.rs`
- Create: `src/data/backfill/mod.rs`

- [ ] **Step 1: 加依赖**

`Cargo.toml` 的 `[dependencies]` 末尾加入:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

- [ ] **Step 2: 注册模块 + 写 `remove_root` 失败测试**

`src/data/mod.rs` 加一行:

```rust
pub mod backfill;
```

在 `src/data/cache.rs` 的 `#[cfg(test)] mod tests` 中加入:

```rust
    #[test]
    fn remove_root_drops_only_that_root() {
        let mut cache = Cache::new();
        insert_entry(&mut cache, "real", "p", "2026-05-01", DayEntry { input: 1, ..Default::default() });
        insert_entry(&mut cache, "backfill:stats-cache", "(historical)", "2026-01-01", DayEntry { input: 2, ..Default::default() });
        cache.remove_root("backfill:stats-cache");
        assert!(cache.get_root("real").is_some());
        assert!(cache.get_root("backfill:stats-cache").is_none());
    }
```

- [ ] **Step 3: 运行,确认失败**

Run: `cargo test --lib cache::tests::remove_root_drops_only_that_root`
Expected: 编译失败 `no method named 'remove_root'`

- [ ] **Step 4: 实现 `remove_root` + 骨架文件**

`src/data/cache.rs` 的 `impl Cache` 中(如 `roots` 之后)加入:

```rust
    /// Drop an entire source root (used to replace synthetic `backfill:*`
    /// roots on each backfill run so re-runs stay idempotent).
    pub fn remove_root(&mut self, root: &str) {
        self.0.remove(root);
    }
```

创建 `src/data/backfill/mod.rs`:

```rust
//! Historical backfill from persistent token sources that outlive Claude
//! Code's 30-day JSONL cleanup. See
//! docs/superpowers/specs/2026-06-01-multi-source-full-history-design.md.

pub mod code_insights;
pub mod layering;
pub mod stats_cache;

use chrono::NaiveDate;

use crate::data::cache::DayEntry;

/// Synthetic source roots so backfilled days are identifiable and replaceable.
pub const STATS_CACHE_ROOT: &str = "backfill:stats-cache";
pub const CODE_INSIGHTS_ROOT: &str = "backfill:code-insights";
/// Pseudo-cwd for stats-cache days, which carry no project dimension.
pub const HISTORICAL_CWD: &str = "(historical)";

/// One backfilled day for a `(root, cwd)`.
#[derive(Debug, Clone, PartialEq)]
pub struct BackfillDay {
    pub root: String,
    pub cwd: String,
    pub date: NaiveDate,
    pub entry: DayEntry,
}
```

> 注:`code_insights`/`layering`/`stats_cache` 三个子模块文件将在后续 Task 创建;本步骤先建空文件以便编译。创建三个空文件:
> - `src/data/backfill/stats_cache.rs`(内容:`// filled in Task 3`)
> - `src/data/backfill/code_insights.rs`(内容:`// filled in Task 4`)
> - `src/data/backfill/layering.rs`(内容:`// filled in Task 5`)

需要 `DayEntry` 派生 `PartialEq` 以便 `BackfillDay` 派生。检查 `src/data/cache.rs` 的 `DayEntry` derive,若缺则改为:

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct DayEntry {
```

- [ ] **Step 5: 运行,确认通过 + 编译**

Run: `cargo test --lib cache::tests::remove_root_drops_only_that_root`
Expected: PASS（首次会下载编译 rusqlite，耗时较长）

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml Cargo.lock src/data/mod.rs src/data/cache.rs src/data/backfill/
git commit -m "feat(backfill): scaffold module, rusqlite dep, Cache::remove_root"
```

---

## Task 3: stats-cache 解析器

**Files:**
- Create/Replace: `src/data/backfill/stats_cache.rs`
- Test: 同文件

- [ ] **Step 1: 写失败测试**

`src/data/backfill/stats_cache.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_daily_rows_and_splits_by_model_usage() {
        let json = r#"{
          "dailyModelTokens": [
            {"date":"2026-01-01","tokensByModel":{"claude-opus-4-6":1000}}
          ],
          "modelUsage": {
            "claude-opus-4-6":{"inputTokens":600,"outputTokens":200,"cacheReadInputTokens":150,"cacheCreationInputTokens":50}
          }
        }"#;
        let days = parse_stats_cache_str(json);
        assert_eq!(days.len(), 1);
        let d = &days[0];
        assert_eq!(d.root, crate::data::backfill::STATS_CACHE_ROOT);
        assert_eq!(d.cwd, crate::data::backfill::HISTORICAL_CWD);
        assert_eq!(d.date, chrono::NaiveDate::from_ymd_opt(2026,1,1).unwrap());
        // 1000 总 token 按 600:200:150:50 拆分 = 600/200/150/50
        assert_eq!(d.entry.input, 600);
        assert_eq!(d.entry.output, 200);
        assert_eq!(d.entry.cache_read, 150);
        assert_eq!(d.entry.cache_creation, 50);
        assert!(d.entry.cost > 0.0);
    }

    #[test]
    fn unknown_model_usage_falls_back_to_all_input() {
        let json = r#"{"dailyModelTokens":[{"date":"2026-02-02","tokensByModel":{"mystery":900}}]}"#;
        let days = parse_stats_cache_str(json);
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].entry.input, 900);
        assert_eq!(days[0].entry.output, 0);
    }

    #[test]
    fn empty_or_missing_yields_nothing() {
        assert!(parse_stats_cache_str("{}").is_empty());
        assert!(parse_stats_cache_str("not json").is_empty());
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test --lib backfill::stats_cache`
Expected: 编译失败 `cannot find function 'parse_stats_cache_str'`

- [ ] **Step 3: 实现**

替换 `src/data/backfill/stats_cache.rs` 顶部为:

```rust
//! Parse `~/.claude/stats-cache.json` -> per-day token totals. This file is
//! Claude Code's own persistent daily-by-model telemetry and survives the
//! 30-day JSONL cleanup (data back to 2026-01-01 observed).

use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDate;
use serde::Deserialize;

use super::{BackfillDay, HISTORICAL_CWD, STATS_CACHE_ROOT};
use crate::data::cache::DayEntry;
use crate::data::models::cost_from_tokens;

#[derive(Deserialize)]
struct StatsCache {
    #[serde(rename = "dailyModelTokens", default)]
    daily: Vec<DailyRow>,
    #[serde(rename = "modelUsage", default)]
    model_usage: HashMap<String, ModelUsage>,
}

#[derive(Deserialize)]
struct DailyRow {
    date: Option<String>,
    #[serde(rename = "tokensByModel", default)]
    tokens_by_model: HashMap<String, u64>,
}

#[derive(Deserialize, Default)]
struct ModelUsage {
    #[serde(rename = "inputTokens", default)]
    input: u64,
    #[serde(rename = "outputTokens", default)]
    output: u64,
    #[serde(rename = "cacheReadInputTokens", default)]
    cache_read: u64,
    #[serde(rename = "cacheCreationInputTokens", default)]
    cache_creation: u64,
}

pub fn parse_stats_cache(path: &Path) -> Vec<BackfillDay> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse_stats_cache_str(&s),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn parse_stats_cache_str(raw: &str) -> Vec<BackfillDay> {
    let Ok(sc) = serde_json::from_str::<StatsCache>(raw) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for row in &sc.daily {
        let Some(date_str) = &row.date else { continue };
        let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
            continue;
        };

        let mut entry = DayEntry::default();
        for (model, &total) in &row.tokens_by_model {
            if total == 0 {
                continue;
            }
            let mu = sc.model_usage.get(model).cloned().unwrap_or_default();
            let [i, o, cr, cc] =
                split_total(total, [mu.input, mu.output, mu.cache_read, mu.cache_creation]);
            entry.input += i;
            entry.output += o;
            entry.cache_read += cr;
            entry.cache_creation += cc;
            entry.cost += cost_from_tokens(model, i, o, cr, cc);
        }

        if entry.input + entry.output + entry.cache_read + entry.cache_creation > 0 {
            out.push(BackfillDay {
                root: STATS_CACHE_ROOT.to_string(),
                cwd: HISTORICAL_CWD.to_string(),
                date,
                entry,
            });
        }
    }
    out
}

impl Clone for ModelUsage {
    fn clone(&self) -> Self {
        Self {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_creation: self.cache_creation,
        }
    }
}

/// Distribute `total` across 4 buckets by `weights` (largest-remainder so the
/// parts sum exactly to `total`). All-zero weights => everything to input.
fn split_total(total: u64, weights: [u64; 4]) -> [u64; 4] {
    if total == 0 {
        return [0; 4];
    }
    let sum: u64 = weights.iter().sum();
    if sum == 0 {
        return [total, 0, 0, 0];
    }
    let mut out = [0u64; 4];
    let mut fracs: [(usize, f64); 4] = [(0, 0.0); 4];
    let mut allocated = 0u64;
    for i in 0..4 {
        let exact = weights[i] as f64 / sum as f64 * total as f64;
        let floor = exact.floor();
        out[i] = floor as u64;
        allocated += out[i];
        fracs[i] = (i, exact - floor);
    }
    let mut remainder = total - allocated;
    fracs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut k = 0usize;
    while remainder > 0 {
        out[fracs[k % 4].0] += 1;
        remainder -= 1;
        k += 1;
    }
    out
}
```

> 注:`ModelUsage` 手写 `Clone`(避免给字段加额外 derive 改动测试)。也可直接给 struct 加 `#[derive(Clone)]` 并删掉手写 impl——二选一,保持一处即可。

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test --lib backfill::stats_cache`
Expected: PASS（3 个测试）

- [ ] **Step 5: 提交**

```bash
git add src/data/backfill/stats_cache.rs
git commit -m "feat(backfill): parse stats-cache.json into per-day token entries"
```

---

## Task 4: Code Insights SQLite 读取器

**Files:**
- Create/Replace: `src/data/backfill/code_insights.rs`
- Test: 同文件(用内存 SQLite)

- [ ] **Step 1: 写失败测试**

`src/data/backfill/code_insights.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, project_path TEXT);
             CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, usage TEXT, timestamp TEXT);
             INSERT INTO sessions VALUES ('s1','/Users/x/proj');
             INSERT INTO messages VALUES ('m1','s1','assistant',
               '{\"inputTokens\":10,\"outputTokens\":7,\"cacheReadTokens\":100,\"cacheCreationTokens\":5,\"model\":\"claude-opus-4-6\"}',
               '2026-03-25T12:00:00.000Z');
             INSERT INTO messages VALUES ('m2','s1','assistant',
               '{\"inputTokens\":20,\"outputTokens\":3,\"cacheReadTokens\":0,\"cacheCreationTokens\":0,\"model\":\"claude-opus-4-6\"}',
               '2026-03-25T13:00:00.000Z');
             INSERT INTO messages VALUES ('m3','s1','user','', '2026-03-25T13:05:00.000Z');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn aggregates_messages_by_cwd_and_date() {
        let days = read_from_conn(&seed());
        assert_eq!(days.len(), 1);
        let d = &days[0];
        assert_eq!(d.root, crate::data::backfill::CODE_INSIGHTS_ROOT);
        assert_eq!(d.cwd, "/Users/x/proj");
        // m1+m2 deltas summed
        assert_eq!(d.entry.input, 30);
        assert_eq!(d.entry.output, 10);
        assert_eq!(d.entry.cache_read, 100);
        assert_eq!(d.entry.cache_creation, 5);
        assert!(d.entry.cost > 0.0);
    }

    #[test]
    fn missing_db_returns_empty() {
        let days = read_code_insights(std::path::Path::new("/no/such/file.db"));
        assert!(days.is_empty());
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test --lib backfill::code_insights`
Expected: 编译失败 `cannot find function 'read_from_conn'`

- [ ] **Step 3: 实现**

替换 `src/data/backfill/code_insights.rs` 顶部为:

```rust
//! Read `~/.code-insights/data.db` (Code Insights, a third-party Claude Code
//! analytics tool). Its `messages.usage` carries per-message delta token
//! counts back to ~2026-03-25, richer than stats-cache (has project_path).
//! Read-only; if the DB is absent/locked/unrecognized we return nothing and
//! let stats-cache cover those dates.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, NaiveDate};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use super::{BackfillDay, CODE_INSIGHTS_ROOT};
use crate::data::cache::DayEntry;
use crate::data::models::cost_from_tokens;

#[derive(Deserialize)]
struct CiUsage {
    #[serde(rename = "inputTokens", default)]
    input: u64,
    #[serde(rename = "outputTokens", default)]
    output: u64,
    #[serde(rename = "cacheReadTokens", default)]
    cache_read: u64,
    #[serde(rename = "cacheCreationTokens", default)]
    cache_creation: u64,
    #[serde(default)]
    model: String,
}

pub fn read_code_insights(db_path: &Path) -> Vec<BackfillDay> {
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return Vec::new();
    };
    read_from_conn(&conn)
}

fn read_from_conn(conn: &Connection) -> Vec<BackfillDay> {
    let mut acc: HashMap<(String, NaiveDate), DayEntry> = HashMap::new();

    let sql = "SELECT m.timestamp, m.usage, s.project_path \
               FROM messages m JOIN sessions s ON m.session_id = s.id \
               WHERE m.usage IS NOT NULL AND m.usage != ''";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };

    for row in rows.flatten() {
        let (ts_str, usage_str, project_path) = row;
        if project_path.is_empty() {
            continue;
        }
        let Ok(usage) = serde_json::from_str::<CiUsage>(&usage_str) else {
            continue;
        };
        if usage.input + usage.output + usage.cache_read + usage.cache_creation == 0 {
            continue;
        }
        let Ok(ts) = DateTime::parse_from_rfc3339(&ts_str) else {
            continue;
        };
        let date = ts.with_timezone(&chrono::Local).date_naive();

        let e = acc.entry((project_path, date)).or_default();
        e.input += usage.input;
        e.output += usage.output;
        e.cache_read += usage.cache_read;
        e.cache_creation += usage.cache_creation;
        e.cost += cost_from_tokens(
            &usage.model,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_creation,
        );
    }

    acc.into_iter()
        .map(|((cwd, date), entry)| BackfillDay {
            root: CODE_INSIGHTS_ROOT.to_string(),
            cwd,
            date,
            entry,
        })
        .collect()
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test --lib backfill::code_insights`
Expected: PASS（2 个测试）

- [ ] **Step 5: 提交**

```bash
git add src/data/backfill/code_insights.rs
git commit -m "feat(backfill): read Code Insights SQLite into per-day entries"
```

---

## Task 5: 分层去重(layering)

**Files:**
- Create/Replace: `src/data/backfill/layering.rs`
- Test: 同文件

- [ ] **Step 1: 写失败测试**

`src/data/backfill/layering.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::backfill::{BackfillDay, CODE_INSIGHTS_ROOT, STATS_CACHE_ROOT};
    use crate::data::cache::DayEntry;
    use chrono::NaiveDate;

    fn day(root: &str, ymd: (i32, u32, u32), input: u64) -> BackfillDay {
        BackfillDay {
            root: root.to_string(),
            cwd: "c".to_string(),
            date: NaiveDate::from_ymd_opt(ymd.0, ymd.1, ymd.2).unwrap(),
            entry: DayEntry { input, ..Default::default() },
        }
    }

    #[test]
    fn ci_wins_over_stats_for_same_date_and_drops_at_boundary() {
        let boundary = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        let ci = vec![
            day(CODE_INSIGHTS_ROOT, (2026, 3, 25), 100), // kept
            day(CODE_INSIGHTS_ROOT, (2026, 5, 4), 999),  // dropped (>= boundary)
        ];
        let stats = vec![
            day(STATS_CACHE_ROOT, (2026, 1, 1), 10),  // kept (older, no CI)
            day(STATS_CACHE_ROOT, (2026, 3, 25), 50), // dropped (CI covers it)
        ];
        let out = layer(ci, stats, boundary);
        let dates: Vec<_> = out.iter().map(|d| (d.root.as_str(), d.date)).collect();
        assert!(dates.contains(&(CODE_INSIGHTS_ROOT, NaiveDate::from_ymd_opt(2026,3,25).unwrap())));
        assert!(dates.contains(&(STATS_CACHE_ROOT, NaiveDate::from_ymd_opt(2026,1,1).unwrap())));
        assert_eq!(out.len(), 2);
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test --lib backfill::layering`
Expected: 编译失败 `cannot find function 'layer'`

- [ ] **Step 3: 实现**

替换 `src/data/backfill/layering.rs` 顶部为:

```rust
//! Layer the backfill sources by date so each date is covered by exactly one
//! (richest available) source and never overlaps the live JSONL window.
//!
//! Precedence: live JSONL (>= boundary, handled elsewhere) > Code Insights >
//! stats-cache. We only emit dates strictly before `boundary_jsonl`, and for
//! a given date Code Insights wins over stats-cache.

use std::collections::HashSet;

use chrono::NaiveDate;

use super::BackfillDay;

pub fn layer(
    ci_days: Vec<BackfillDay>,
    stats_days: Vec<BackfillDay>,
    boundary_jsonl: NaiveDate,
) -> Vec<BackfillDay> {
    let ci: Vec<BackfillDay> = ci_days
        .into_iter()
        .filter(|d| d.date < boundary_jsonl)
        .collect();
    let ci_dates: HashSet<NaiveDate> = ci.iter().map(|d| d.date).collect();

    let mut out = ci;
    out.extend(
        stats_days
            .into_iter()
            .filter(|d| d.date < boundary_jsonl && !ci_dates.contains(&d.date)),
    );
    out
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test --lib backfill::layering`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/data/backfill/layering.rs
git commit -m "feat(backfill): layer sources by date without double-counting"
```

---

## Task 6: 编排 `run_backfill`

**Files:**
- Modify: `src/data/backfill/mod.rs`
- Test: 同文件

- [ ] **Step 1: 写失败测试**

在 `src/data/backfill/mod.rs` 末尾加入:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::cache::{Cache, DayEntry};
    use chrono::NaiveDate;

    #[test]
    fn boundary_uses_only_real_roots() {
        let mut cache = Cache::new();
        cache
            .entry_root("/real/projects".into())
            .entry("p".into())
            .or_default()
            .insert("2026-05-03".into(), DayEntry { input: 1, ..Default::default() });
        cache
            .entry_root(STATS_CACHE_ROOT.into())
            .entry(HISTORICAL_CWD.into())
            .or_default()
            .insert("2026-01-01".into(), DayEntry { input: 1, ..Default::default() });
        assert_eq!(
            real_root_min_date(&cache),
            Some(NaiveDate::from_ymd_opt(2026, 5, 3).unwrap())
        );
    }

    #[test]
    fn apply_replaces_backfill_roots_idempotently() {
        let mut cache = Cache::new();
        let layered = vec![BackfillDay {
            root: STATS_CACHE_ROOT.to_string(),
            cwd: HISTORICAL_CWD.to_string(),
            date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            entry: DayEntry { input: 42, ..Default::default() },
        }];
        apply_backfill(&mut cache, &layered);
        apply_backfill(&mut cache, &layered); // second run must not duplicate
        let root = cache.get_root(STATS_CACHE_ROOT).unwrap();
        assert_eq!(root[HISTORICAL_CWD]["2026-01-01"].input, 42);
    }
}
```

> 测试用到 `Cache::get_root`(已存在,`#[cfg(test)]`)。

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test --lib backfill::tests`
Expected: 编译失败 `cannot find function 'real_root_min_date'`

- [ ] **Step 3: 实现**

在 `src/data/backfill/mod.rs`(`BackfillDay` 定义之后)加入:

```rust
use std::path::Path;

use crate::data::cache::{self, Cache};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSel {
    All,
    StatsCache,
    CodeInsights,
}

impl SourceSel {
    fn includes_ci(self) -> bool {
        matches!(self, SourceSel::All | SourceSel::CodeInsights)
    }
    fn includes_stats(self) -> bool {
        matches!(self, SourceSel::All | SourceSel::StatsCache)
    }
}

#[derive(Debug, Clone)]
pub struct BackfillOptions {
    pub dry_run: bool,
    pub source: SourceSel,
    pub since: Option<NaiveDate>,
}

#[derive(Debug, Default)]
pub struct BackfillSummary {
    pub days: usize,
    pub earliest: Option<NaiveDate>,
    pub latest: Option<NaiveDate>,
    pub total_input: u64,
    pub total_output: u64,
    pub total_cost: f64,
    pub dry_run: bool,
}

/// Earliest date held under a real (non-`backfill:`) source root.
fn real_root_min_date(cache: &Cache) -> Option<NaiveDate> {
    cache
        .iter_filtered(None, None)
        .filter(|(root, _, _, _)| !root.starts_with("backfill:"))
        .filter_map(|(_, _, date, _)| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .min()
}

/// Replace the synthetic backfill roots with freshly computed days. Removing
/// first keeps re-runs idempotent and prevents stale stats-cache days from
/// lingering once Code Insights covers the same date.
fn apply_backfill(cache: &mut Cache, layered: &[BackfillDay]) {
    cache.remove_root(STATS_CACHE_ROOT);
    cache.remove_root(CODE_INSIGHTS_ROOT);
    for d in layered {
        cache
            .entry_root(d.root.clone())
            .entry(d.cwd.clone())
            .or_default()
            .insert(d.date.format("%Y-%m-%d").to_string(), d.entry.clone());
    }
}

fn summarize(layered: &[BackfillDay], dry_run: bool) -> BackfillSummary {
    BackfillSummary {
        days: layered.len(),
        earliest: layered.iter().map(|d| d.date).min(),
        latest: layered.iter().map(|d| d.date).max(),
        total_input: layered.iter().map(|d| d.entry.input).sum(),
        total_output: layered.iter().map(|d| d.entry.output).sum(),
        total_cost: layered.iter().map(|d| d.entry.cost).sum(),
        dry_run,
    }
}

pub fn run_backfill(opts: &BackfillOptions) -> BackfillSummary {
    let home = dirs::home_dir().unwrap_or_default();
    let stats_path = home.join(".claude").join("stats-cache.json");
    let ci_path = home.join(".code-insights").join("data.db");

    let mut cache = cache::load().cache;
    let boundary =
        real_root_min_date(&cache).unwrap_or_else(|| chrono::Local::now().date_naive());

    let ci_days = if opts.source.includes_ci() {
        code_insights::read_code_insights(&ci_path)
    } else {
        Vec::new()
    };
    let stats_days = if opts.source.includes_stats() {
        stats_cache::parse_stats_cache(&stats_path)
    } else {
        Vec::new()
    };

    let mut layered = layering::layer(ci_days, stats_days, boundary);
    if let Some(since) = opts.since {
        layered.retain(|d| d.date >= since);
    }

    let summary = summarize(&layered, opts.dry_run);
    if opts.dry_run {
        return summary;
    }

    apply_backfill(&mut cache, &layered);
    cache::save(&cache);
    summary
}
```

需要在文件顶部已 `use chrono::NaiveDate;`(Task 2 已加)。

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test --lib backfill::tests`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add src/data/backfill/mod.rs
git commit -m "feat(backfill): orchestrate run_backfill with idempotent root replacement"
```

---

## Task 7: `ccmeter backfill` 子命令

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: 改 CLI 定义**

将 `src/main.rs` 的 `Cli` 结构(第 21-23 行)替换为:

```rust
#[derive(Parser)]
#[command(name = "ccmeter", about = "Claude Code usage statistics")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Backfill historical usage from persistent sources (stats-cache.json,
    /// Code Insights) back to 2026-01-01, merging into the local history cache.
    Backfill {
        /// Print what would be written without modifying the cache.
        #[arg(long)]
        dry_run: bool,
        /// Which source(s) to read.
        #[arg(long, value_enum, default_value_t = SourceArg::All)]
        source: SourceArg,
        /// Only backfill dates on or after YYYY-MM-DD.
        #[arg(long)]
        since: Option<String>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SourceArg {
    All,
    StatsCache,
    CodeInsights,
}
```

- [ ] **Step 2: 改 `main` 入口分流**

将 `fn main()` 开头的 `let _cli = Cli::parse();`(第 43 行)替换为:

```rust
    let cli = Cli::parse();

    if let Some(Command::Backfill {
        dry_run,
        source,
        since,
    }) = cli.command
    {
        return run_backfill_cli(dry_run, source, since);
    }
```

并在 `fn main` 之后新增辅助函数(不进入 TUI,纯 stdout 输出):

```rust
fn run_backfill_cli(
    dry_run: bool,
    source: SourceArg,
    since: Option<String>,
) -> io::Result<()> {
    use crate::data::backfill::{self, BackfillOptions, SourceSel};

    let since = match since {
        Some(s) => match chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(_) => {
                eprintln!("invalid --since date '{s}', expected YYYY-MM-DD");
                std::process::exit(2);
            }
        },
        None => None,
    };

    let source = match source {
        SourceArg::All => SourceSel::All,
        SourceArg::StatsCache => SourceSel::StatsCache,
        SourceArg::CodeInsights => SourceSel::CodeInsights,
    };

    let summary = backfill::run_backfill(&BackfillOptions {
        dry_run,
        source,
        since,
    });

    let verb = if summary.dry_run {
        "would backfill"
    } else {
        "backfilled"
    };
    match (summary.earliest, summary.latest) {
        (Some(e), Some(l)) => println!(
            "{verb} {} day(s), {e} -> {l}: {} input + {} output tokens, ${:.2}",
            summary.days, summary.total_input, summary.total_output, summary.total_cost
        ),
        _ => println!("{verb} 0 days (no historical sources found)"),
    }
    Ok(())
}
```

需要顶部已 `use chrono;`?不需要——用全限定 `chrono::NaiveDate`。`io` 已在用。

- [ ] **Step 3: 编译 + 手动 dry-run 验证**

Run: `cargo run -- backfill --dry-run`
Expected: 打印类似 `would backfill N day(s), 2026-01-01 -> 2026-05-02: ... tokens, $...`,**不写盘**(`~/.config/ccmeter/history.json` 不变)。

Run: `cargo run -- backfill --dry-run --source stats-cache`
Expected: 仅 stats-cache 段,最早 2026-01-01。

- [ ] **Step 4: 真实回填 + 验证 TUI**

Run: `cargo run -- backfill`
Expected: `backfilled N day(s) ...`。随后 `cargo run`(进 TUI)在 `All` 视图热力图应延伸到 1 月。

- [ ] **Step 5: 提交**

```bash
git add src/main.rs
git commit -m "feat(backfill): add 'ccmeter backfill' subcommand"
```

---

## Task 8: 历史粗粒度注脚(诚实标注)

**Files:**
- Modify: `src/ui/dashboard.rs`(主视图渲染处)

**目的**:当缓存含 `backfill:*` 历史(老于实时窗口)时,在主视图底部显示一行说明,避免把粗粒度历史误读为全保真。仿 slopmeter "earlier activity may be undercounted"。

- [ ] **Step 1: 在 App 暴露"是否有回填历史"标志**

`src/app.rs` 的 `AppData` 已持有 `merged_cache`。在 `App` 上新增只读方法(放 `impl App` 内,`helpers` 区):

```rust
    /// True when the cache holds synthetic backfill roots (history older than
    /// the live JSONL window). Used to render a low-fidelity notice.
    pub(crate) fn has_backfilled_history(&self) -> bool {
        self.data
            .merged_cache
            .roots()
            .any(|(root, _)| root.starts_with("backfill:"))
    }
```

- [ ] **Step 2: 在主视图渲染注脚**

定位 `src/ui/dashboard.rs` 中绘制主视图底部状态/帮助行的位置(搜索现有 footer/help 文案,如包含 `"Tab"` 或 `"q quit"` 的 `Line`/`Span`)。在该行渲染逻辑附近,当 `app.has_backfilled_history()` 为真时追加一段灰色说明文本:

```rust
    if app.has_backfilled_history() {
        let note = Line::from(Span::styled(
            "Pre-30d history is token-only (no per-project/line/model breakdown).",
            Style::default().fg(theme().muted),
        ));
        // render `note` on the line directly above the existing footer
        // (use the same Rect math as the surrounding footer; subtract 1 row).
    }
```

> 注:具体 `Rect`/主题字段名以 `dashboard.rs` 现有 footer 实现为准(`theme()` 已在 `ui` 内可用;若无 `muted` 字段,用最接近的暗灰,如 `heatmap_label`)。保持与现有 footer 同样的布局方式,只占一行。

- [ ] **Step 3: 编译 + 目测**

Run: `cargo run`(在已回填的缓存上)
Expected: 主视图底部出现该灰色注脚;切到非 backfill 状态(删 `history.json` 后重跑且未回填)则不显示。

- [ ] **Step 4: 提交**

```bash
git add src/app.rs src/ui/dashboard.rs
git commit -m "feat(backfill): note low-fidelity historical data in main view"
```

---

## Self-Review 结果

- **Spec 覆盖**:§3.1 分层→Task5;§3.2 stats-cache→Task3、CI→Task4、合成 root/cwd→Task2/6;§3.3 注脚→Task8、成本重算→Task1;§3.4 CLI/幂等→Task6/7;§3.5 rusqlite→Task2。**已声明推迟**:§3.2 的 `lines_*`、`active_minutes`、逐模型(见顶部"v1 范围")——作为 follow-up,非隐藏缺口。
- **类型一致**:`BackfillDay{root,cwd,date,entry}`、`STATS_CACHE_ROOT`/`CODE_INSIGHTS_ROOT`/`HISTORICAL_CWD`、`cost_from_tokens(model,input,output,cache_read,cache_creation)`、`run_backfill(&BackfillOptions)->BackfillSummary` 在 Task 间一致。
- **占位符**:无 TODO/TBD;唯一"以现有实现为准"在 Task8 的 footer Rect/主题字段(执行时按 dashboard.rs 现状对齐),已显式说明而非含糊。

## 验收(对应 spec §8)

- [ ] `cargo run -- backfill` 后 `history.json` 含 2026-01-01 起条目
- [ ] TUI `All` 热力图/总成本/总 token 延伸到 1 月
- [ ] `--dry-run` 总量 = CI 段 + stats 段(无重复计数)
- [ ] 5/3 起的日子仍全保真(项目卡片/行数不变)
- [ ] 主视图有粗粒度注脚
- [ ] `cargo test` 通过;`cargo clippy` 无新增告警
