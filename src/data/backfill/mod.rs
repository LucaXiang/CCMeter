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
