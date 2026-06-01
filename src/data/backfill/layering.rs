//! Layer the backfill sources by date so each date is covered by exactly one
//! (richest available) source and never overlaps the live JSONL window.
//!
//! Precedence: live JSONL (>= boundary, handled elsewhere) > Code Insights >
//! stats-cache. We only emit dates strictly before `boundary_jsonl`, and for
//! a given date Code Insights wins over stats-cache.

use std::collections::HashSet;

use chrono::NaiveDate;

use super::BackfillDay;

/// Returns the backfill days to keep (CI-kept then stats-kept). The result is
/// NOT sorted by date; callers that need chronological order must sort it.
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

    #[test]
    fn stats_day_survives_when_ci_same_date_is_at_boundary() {
        let boundary = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        // CI day is exactly at the boundary -> dropped before ci_dates is built.
        let ci = vec![day(CODE_INSIGHTS_ROOT, (2026, 5, 3), 999)];
        // Stats day on the same date but the date is < boundary? No: same date
        // == boundary is also dropped. Use a stats day strictly before boundary
        // that shares NO date with any *kept* CI day.
        let stats = vec![day(STATS_CACHE_ROOT, (2026, 5, 2), 50)];
        let out = layer(ci, stats, boundary);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].root, STATS_CACHE_ROOT);
        assert_eq!(out[0].entry.input, 50);
    }
}
