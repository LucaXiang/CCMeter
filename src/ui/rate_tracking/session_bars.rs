use chrono::NaiveDate;

use crate::data::index::EventIndex;
use crate::data::models::{per_model_cost_split, per_model_total_tokens};
use crate::data::oauth::OAuthCredential;
use crate::data::rate_history::{RateHistory, RateHistoryEntry};
use crate::data::rate_limits::RateLimitHit;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum BarKind {
    /// Violet: historical average from rate-history (no rate-limit hit that day).
    History,
    /// Red: historical average from rate-history, only sessions that hit the limit.
    RateLimited,
    /// Blue: current live session (always last bar).
    Current,
}

pub(super) struct DayBar {
    pub(super) date: NaiveDate,
    /// Per-side USD costs. `cost() = input + cache + output` is the bar
    /// height; the triplet drives the 3-layer stacked visualization.
    pub(super) input_cost: f64,
    pub(super) cache_cost: f64,
    pub(super) output_cost: f64,
    pub(super) kind: BarKind,
}

impl DayBar {
    pub(super) fn cost(&self) -> f64 {
        self.input_cost + self.cache_cost + self.output_cost
    }
}

pub(super) fn should_extrapolate_live_session(elapsed_min: f64, utilization: f64) -> bool {
    elapsed_min >= 30.0 && utilization >= 2.0
}

/// Extrapolate a session's per-side cost split by scaling the per-model
/// breakdown stored in history up to the estimated 100%-saturation total.
/// Returns `(input, cache, output)` in USD.
fn extrapolate_entry_cost(entry: &RateHistoryEntry) -> (f64, f64, f64) {
    let (in_cost, cache_cost, out_cost) = per_model_cost_split(&entry.per_model);
    let actual_tokens = per_model_total_tokens(&entry.per_model);
    if actual_tokens == 0 {
        return (0.0, 0.0, 0.0);
    }
    let scale = entry.estimated_tokens as f64 / actual_tokens as f64;
    (in_cost * scale, cache_cost * scale, out_cost * scale)
}

/// Compute session bars:
/// - Violet/Red bars from rate-history (one per session, keyed by session window).
///   Red if at least one rate-limit hit overlaps the session, violet otherwise.
/// - Blue bar for the current live session (always appended last).
pub(super) fn compute_session_bars(
    hits: &[&RateLimitHit],
    index: &EventIndex,
    source_root: Option<&str>,
    selected_cred: Option<&OAuthCredential>,
    rate_history: &RateHistory,
) -> Vec<DayBar> {
    let Some(root) = source_root else {
        return Vec::new();
    };

    // Collect rate-limit hit timestamps in local time for overlap checking.
    let hit_timestamps: Vec<chrono::NaiveDateTime> = hits
        .iter()
        .map(|h| h.timestamp.with_timezone(&chrono::Local).naive_local())
        .collect();

    let entries = rate_history.entries_for_root(root);

    // Compute per-session costs, then aggregate by session-start date.
    // Multiple sessions on the same day are averaged into one bar.
    let mut day_accum: std::collections::BTreeMap<NaiveDate, (f64, f64, f64, bool, usize)> =
        std::collections::BTreeMap::new();

    // Track which hits have been attributed to a rate-history session so
    // we can emit standalone red bars for the rest.
    let mut covered_hits: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for entry in &entries {
        let Some(session_start) = entry.session_start_local() else {
            continue;
        };
        let Some(session_end) = entry.session_end_local() else {
            continue;
        };

        let mut is_rate_limited = false;
        for (i, &ts) in hit_timestamps.iter().enumerate() {
            if ts >= session_start && ts <= session_end {
                is_rate_limited = true;
                covered_hits.insert(i);
            }
        }

        let (in_cost, cache_cost, out_cost) = extrapolate_entry_cost(entry);

        let date = session_start.date();
        let acc = day_accum.entry(date).or_default();
        acc.0 += in_cost;
        acc.1 += cache_cost;
        acc.2 += out_cost;
        acc.3 = acc.3 || is_rate_limited;
        acc.4 += 1;
    }

    // Standalone hits (outside any rate-history session window): render at
    // the hit's own saturation cost — by definition, reaching a rate-limit
    // means the session was at 100% of its 5h allotment.
    for (i, hit) in hits.iter().enumerate() {
        if covered_hits.contains(&i) {
            continue;
        }
        let (in_c, cache_c, out_c) = per_model_cost_split(&hit.per_model);
        if in_c + cache_c + out_c <= 0.0 {
            continue;
        }
        let date = hit_timestamps[i].date();
        let acc = day_accum.entry(date).or_default();
        acc.0 += in_c;
        acc.1 += cache_c;
        acc.2 += out_c;
        acc.3 = true;
        acc.4 += 1;
    }

    let mut bars: Vec<DayBar> = Vec::new();
    for (date, (sum_in, sum_cache, sum_out, rate_limited, count)) in &day_accum {
        let n = *count as f64;
        bars.push(DayBar {
            date: *date,
            input_cost: sum_in / n,
            cache_cost: sum_cache / n,
            output_cost: sum_out / n,
            kind: if *rate_limited {
                BarKind::RateLimited
            } else {
                BarKind::History
            },
        });
    }

    // Fill gaps: ensure every day from first to last has an entry
    if !bars.is_empty() {
        let first = bars[0].date;
        let last = bars.last().unwrap().date;
        let existing: std::collections::HashSet<NaiveDate> = bars.iter().map(|b| b.date).collect();
        let mut d = first;
        while d <= last {
            if !existing.contains(&d) {
                bars.push(DayBar {
                    date: d,
                    input_cost: 0.0,
                    cache_cost: 0.0,
                    output_cost: 0.0,
                    kind: BarKind::History,
                });
            }
            d += chrono::Duration::days(1);
        }
        bars.sort_by_key(|b| b.date);
    }

    if let Some(cred) = selected_cred
        && let Some(usage) = &cred.usage
        && let Some(five_h) = &usage.five_hour
        && let Some(resets_at_str) = &five_h.resets_at
        && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
    {
        let resets_utc = resets_at.with_timezone(&chrono::Utc);
        let session_start_utc = resets_utc - chrono::Duration::hours(5);
        let now_utc = chrono::Utc::now();

        let elapsed_min = (now_utc - session_start_utc).num_seconds().max(0) as f64 / 60.0;
        let utilization = five_h.utilization;

        if should_extrapolate_live_session(elapsed_min, utilization) {
            let start_local = session_start_utc
                .with_timezone(&chrono::Local)
                .naive_local();
            let end_local = now_utc.with_timezone(&chrono::Local).naive_local();
            let (live_in_cost, live_cache_cost, live_out_cost) =
                index.cost_in_window_split(root, start_local, end_local);
            if live_in_cost + live_cache_cost + live_out_cost > 0.0 {
                let scale = 1.0 / (utilization / 100.0);
                bars.push(DayBar {
                    date: start_local.date(),
                    input_cost: live_in_cost * scale,
                    cache_cost: live_cache_cost * scale,
                    output_cost: live_out_cost * scale,
                    kind: BarKind::Current,
                });
            }
        }
    }

    bars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_extrapolation_waits_for_stable_session() {
        assert!(!should_extrapolate_live_session(12.0, 20.0));
        assert!(!should_extrapolate_live_session(45.0, 1.0));
        assert!(should_extrapolate_live_session(45.0, 20.0));
    }
}
