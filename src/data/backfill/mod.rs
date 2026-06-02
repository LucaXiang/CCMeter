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
        .iter_filtered(&crate::data::cache::RootFilter::All, None)
        .filter(|(root, _, _, _)| !root.starts_with("backfill:"))
        .filter_map(|(_, _, date, _)| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .min()
}

/// Replace the synthetic backfill roots with freshly computed days. BOTH
/// `backfill:*` roots are always removed first (regardless of which sources
/// were selected), then the layered days are inserted. This keeps re-runs
/// idempotent and prevents stale days from lingering once a richer source
/// covers the same date, but means a single-source run overwrites the entire
/// backfill state — always re-run with all sources to get a complete backfill.
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

/// Read persistent historical sources and merge them into the on-disk history
/// cache under synthetic `backfill:*` roots. The boundary is the earliest date
/// held under a real (live-JSONL) root; only dates strictly before it are
/// backfilled, so the live window is never overwritten. When the cache has no
/// real roots yet, the boundary falls back to today (all available history is
/// imported).
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

    let layered = layering::layer(ci_days, stats_days, boundary);

    let summary = summarize(&layered, opts.dry_run);
    if opts.dry_run {
        return summary;
    }

    apply_backfill(&mut cache, &layered);
    cache::save(&cache);
    summary
}

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
