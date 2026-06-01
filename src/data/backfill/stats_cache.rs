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

#[derive(Deserialize, Default, Clone)]
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
    let mut remainder = total.saturating_sub(allocated);
    fracs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut k = 0usize;
    while remainder > 0 {
        out[fracs[k % 4].0] += 1;
        remainder -= 1;
        k += 1;
    }
    out
}

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

    #[test]
    fn split_total_distributes_remainder_by_largest_fraction() {
        // total 10, weights [3,3,3,0], sum 9 -> exact 3.33 each (idx 0..2).
        // floors sum to 9, remainder 1 goes to the first largest-fraction
        // bucket; ties keep original order (stable sort) -> index 0.
        assert_eq!(split_total(10, [3, 3, 3, 0]), [4, 3, 3, 0]);
        // sum exactly preserved
        let parts = split_total(10, [3, 3, 3, 0]);
        assert_eq!(parts.iter().sum::<u64>(), 10);
    }
}
