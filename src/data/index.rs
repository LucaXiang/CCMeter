use std::collections::{HashMap, HashSet};

use chrono::{NaiveDate, Timelike};

use super::cache::RootFilter;
use super::models::{OUTPUT_COST_WEIGHT, PerModelUsage, model_breakdown_label};
use super::parser::Event;
use super::tokens::MinuteTokens;

// ---------------------------------------------------------------------------
// Compact pre-aggregated entry
// ---------------------------------------------------------------------------

/// One record per unique (root, cwd, model_idx, date, minute) combination.
///
/// `model_idx` points into `EventIndex::models` for the full model string
/// (e.g. "claude-opus-4-6-...") so per-entry pricing stays precise. The coarse
/// family (opus/sonnet/haiku) and the per-provider breakdown label are derived
/// on demand from that string via `model_breakdown_label`.
#[derive(Debug, Clone)]
pub struct CompactEntry {
    pub root_idx: u16,
    pub cwd_idx: u16,
    pub model_idx: u16,
    pub date: NaiveDate,
    pub minute: u16,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub cost: f64,
    pub lines_accepted: u64,
    pub lines_suggested: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
}

// ---------------------------------------------------------------------------
// Combined model stats result
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ModelStats {
    pub tokens: HashMap<(String, String), u64>,
    pub daily_costs: HashMap<(String, String), HashMap<NaiveDate, f64>>,
    pub minute_costs: HashMap<(String, String), HashMap<(NaiveDate, u16), f64>>,
}

// ---------------------------------------------------------------------------
// EventIndex
// ---------------------------------------------------------------------------

/// Compact, pre-indexed replacement for `Vec<Event>`.
///
/// Events are resolved (session → root/cwd) and aggregated by
/// (root, cwd, model, date, minute_of_day) to drastically reduce
/// memory compared to keeping every raw event.
pub struct EventIndex {
    roots: Vec<String>,
    cwds: Vec<String>,
    models: Vec<String>,
    root_intern: HashMap<String, u16>,
    cwd_intern: HashMap<String, u16>,
    entries: Vec<CompactEntry>,
}

impl EventIndex {
    /// Build from raw events + session map. After this call the raw events
    /// can be dropped.
    pub fn build(events: &[Event], session_info: &HashMap<String, (String, String)>) -> Self {
        let mut root_intern: HashMap<String, u16> = HashMap::new();
        let mut cwd_intern: HashMap<String, u16> = HashMap::new();
        let mut model_intern: HashMap<String, u16> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        let mut cwds: Vec<String> = Vec::new();
        let mut models: Vec<String> = Vec::new();

        // Aggregate into a HashMap first, then flatten.
        type Key = (u16, u16, u16, NaiveDate, u16);
        #[derive(Default)]
        struct Acc {
            input: u64,
            output: u64,
            cache_read: u64,
            cache_creation: u64,
            cost: f64,
            lines_accepted: u64,
            lines_suggested: u64,
            lines_added: u64,
            lines_deleted: u64,
        }

        let mut agg: HashMap<Key, Acc> = HashMap::new();

        for ev in events {
            let (root, cwd) = match session_info.get(&ev.session_file) {
                Some(pair) => pair,
                None => continue,
            };

            let root_idx = *root_intern.entry(root.clone()).or_insert_with(|| {
                let idx = roots.len() as u16;
                roots.push(root.clone());
                idx
            });
            let cwd_idx = *cwd_intern.entry(cwd.clone()).or_insert_with(|| {
                let idx = cwds.len() as u16;
                cwds.push(cwd.clone());
                idx
            });
            let model_idx = *model_intern.entry(ev.model.clone()).or_insert_with(|| {
                let idx = models.len() as u16;
                models.push(ev.model.clone());
                idx
            });

            let local = ev.timestamp.with_timezone(&chrono::Local);
            let date = local.date_naive();
            let minute = local.hour() as u16 * 60 + local.minute() as u16;

            let key = (root_idx, cwd_idx, model_idx, date, minute);
            let acc = agg.entry(key).or_default();
            acc.input += ev.input_tokens;
            acc.output += ev.output_tokens;
            acc.cache_read += ev.cache_read_input_tokens;
            acc.cache_creation += ev.cache_creation_input_tokens;
            acc.cost += ev.cost_usd;
            acc.lines_accepted += ev.lines_accepted;
            acc.lines_suggested += ev.lines_suggested;
            acc.lines_added += ev.lines_added;
            acc.lines_deleted += ev.lines_deleted;
        }

        let entries: Vec<CompactEntry> = agg
            .into_iter()
            .map(|((root_idx, cwd_idx, model_idx, date, minute), a)| {
                CompactEntry {
                    root_idx,
                    cwd_idx,
                    model_idx,
                    date,
                    minute,
                    input_tokens: a.input,
                    output_tokens: a.output,
                    cache_read: a.cache_read,
                    cache_creation: a.cache_creation,
                    cost: a.cost,
                    lines_accepted: a.lines_accepted,
                    lines_suggested: a.lines_suggested,
                    lines_added: a.lines_added,
                    lines_deleted: a.lines_deleted,
                }
            })
            .collect();

        EventIndex {
            roots,
            cwds,
            models,
            root_intern,
            cwd_intern,
            entries,
        }
    }

    /// Fold OpenAI Codex per-turn deltas into the index under [`CODEX_ROOT`]
    /// so Codex usage participates in the same per-model / per-window queries
    /// as Claude (rate-tracking breakdown, dashboard model stats). Codex models
    /// keep their specific name (`gpt-5.5`, `gpt-5.3-codex`) via `model_idx`;
    /// `ModelId` stays `Other` since the coarse family is meaningless for
    /// non-Claude providers. Deltas are aggregated by (cwd, model, date,
    /// minute) to match Claude's per-minute granularity. `d.input` is already
    /// fresh (cache-exclusive); cost reconstructs the cache-inclusive input.
    pub fn fold_codex(&mut self, deltas: &[crate::data::codex::CodexDelta]) {
        use crate::data::codex::CODEX_ROOT;
        use crate::data::models::cost_from_tokens;
        if deltas.is_empty() {
            return;
        }

        let root_idx = *self
            .root_intern
            .entry(CODEX_ROOT.to_string())
            .or_insert_with(|| {
                let i = self.roots.len() as u16;
                self.roots.push(CODEX_ROOT.to_string());
                i
            });

        // Seed an owned model interner from the existing table so freshly
        // pushed models get correct, stable indices into `self.models`.
        let mut model_intern: HashMap<String, u16> = self
            .models
            .iter()
            .enumerate()
            .map(|(i, m)| (m.clone(), i as u16))
            .collect();

        // Aggregate per (cwd_idx, model_idx, date, minute) →
        // (input, output, cache_read, cost) to match Claude's minute keying.
        type CodexKey = (u16, u16, NaiveDate, u16);
        type CodexAcc = (u64, u64, u64, f64);
        let mut agg: HashMap<CodexKey, CodexAcc> = HashMap::new();
        for d in deltas {
            let cwd_idx = match self.cwd_intern.get(&d.cwd) {
                Some(&i) => i,
                None => {
                    let i = self.cwds.len() as u16;
                    self.cwds.push(d.cwd.clone());
                    self.cwd_intern.insert(d.cwd.clone(), i);
                    i
                }
            };
            let model_idx = match model_intern.get(d.model.as_str()) {
                Some(&i) => i,
                None => {
                    let i = self.models.len() as u16;
                    self.models.push(d.model.clone());
                    model_intern.insert(d.model.clone(), i);
                    i
                }
            };
            let cost = cost_from_tokens(&d.model, d.input + d.cache_read, d.output, d.cache_read, 0);
            let slot = agg.entry((cwd_idx, model_idx, d.date, d.minute)).or_default();
            slot.0 += d.input;
            slot.1 += d.output;
            slot.2 += d.cache_read;
            slot.3 += cost;
        }

        for ((cwd_idx, model_idx, date, minute), (input, output, cache_read, cost)) in agg {
            self.entries.push(CompactEntry {
                root_idx,
                cwd_idx,
                model_idx,
                date,
                minute,
                input_tokens: input,
                output_tokens: output,
                cache_read,
                cache_creation: 0,
                cost,
                lines_accepted: 0,
                lines_suggested: 0,
                lines_added: 0,
                lines_deleted: 0,
            });
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Build MinuteTokens (for intraday heatmap), filtered by root and/or cwds.
    pub fn build_minute_tokens(
        &self,
        roots: &RootFilter,
        project_cwds: Option<&[String]>,
    ) -> MinuteTokens {
        let cwd_filter = project_cwds.map(|cwds| self.cwd_set(cwds));

        let mut mt = MinuteTokens::default();
        for e in &self.entries {
            if !self.entry_passes(e, roots, cwd_filter.as_ref()) {
                continue;
            }
            let key = (e.date, e.minute);
            *mt.input.entry(key).or_default() += e.input_tokens;
            *mt.output.entry(key).or_default() += e.output_tokens;
            *mt.lines_accepted.entry(key).or_default() += e.lines_accepted;
            *mt.lines_suggested.entry(key).or_default() += e.lines_suggested;
            *mt.lines_added.entry(key).or_default() += e.lines_added;
            *mt.lines_deleted.entry(key).or_default() += e.lines_deleted;
            *mt.cost.entry(key).or_default() += e.cost;
        }
        mt
    }

    /// Build all model-level aggregations in a single pass over entries.
    ///
    /// When `min_minute` is `Some(m)`, only entries on `today` whose minute >=
    /// `m` are included (sub-day filtering for 1H / 12H).
    pub fn build_model_stats(
        &self,
        cwd_to_root: &HashMap<String, String>,
        roots: &RootFilter,
        date_filter: &impl Fn(NaiveDate) -> bool,
        project_cwds: Option<&[String]>,
        include_minute: bool,
        subday_start: Option<(NaiveDate, u16)>,
    ) -> ModelStats {
        let cwd_filter = project_cwds.map(|cwds| self.cwd_set(cwds));

        // Map cwd_idx → root_key_idx so multiple cwds sharing a root_key
        // aggregate correctly. Owned Strings so the loop can also intern roots
        // for cwds that have no ProjectGroup (e.g. Codex) without lifetime
        // tangles between `cwd_to_root` and `self.roots`.
        let mut rk_intern: HashMap<String, u16> = HashMap::new();
        let mut rk_strings: Vec<String> = Vec::new();
        let cwd_to_rk: HashMap<u16, u16> = self
            .cwds
            .iter()
            .enumerate()
            .filter_map(|(i, cwd)| {
                let rk = cwd_to_root.get(cwd.as_str())?.as_str();
                let rk_idx = *rk_intern.entry(rk.to_string()).or_insert_with(|| {
                    let idx = rk_strings.len() as u16;
                    rk_strings.push(rk.to_string());
                    idx
                });
                Some((i as u16, rk_idx))
            })
            .collect();

        // Keyed by (root_key_idx, breakdown label). The label is the Claude
        // family (opus/sonnet/haiku/other) but the specific model name for
        // other providers (gpt-5.5, gpt-5.3-codex), so Codex models stay split.
        let mut tok_agg: HashMap<(u16, String), u64> = HashMap::new();
        let mut daily_agg: HashMap<(u16, String), HashMap<NaiveDate, f64>> = HashMap::new();
        let mut minute_agg: HashMap<(u16, String), HashMap<(NaiveDate, u16), f64>> =
            HashMap::new();

        for e in &self.entries {
            if !self.entry_passes(e, roots, cwd_filter.as_ref()) {
                continue;
            }
            if !date_filter(e.date) {
                continue;
            }
            // Sub-day filtering: skip entries before the start (date, minute).
            if let Some((sd, sm)) = subday_start
                && e.date == sd
                && e.minute < sm
            {
                continue;
            }
            let entry_root = self.roots[e.root_idx as usize].as_str();
            let rk_lookup = cwd_to_rk.get(&e.cwd_idx).copied();
            let rk_idx = match rk_lookup {
                Some(idx) => idx,
                None => match rk_intern.get(entry_root) {
                    Some(&idx) => idx,
                    None => {
                        let idx = rk_strings.len() as u16;
                        rk_strings.push(entry_root.to_string());
                        rk_intern.insert(entry_root.to_string(), idx);
                        idx
                    }
                },
            };

            let label = model_breakdown_label(&self.models[e.model_idx as usize]);

            let total = e.input_tokens + e.output_tokens;
            if label != "other" || total > 0 {
                *tok_agg.entry((rk_idx, label.clone())).or_default() += total;
            }

            if e.cost > 0.0 {
                *daily_agg
                    .entry((rk_idx, label.clone()))
                    .or_default()
                    .entry(e.date)
                    .or_default() += e.cost;
            }

            if include_minute && e.cost > 0.0 {
                *minute_agg
                    .entry((rk_idx, label))
                    .or_default()
                    .entry((e.date, e.minute))
                    .or_default() += e.cost;
            }
        }

        // Convert (rk_idx, label) → (root_key string, label).
        let resolve = |k: &(u16, String)| -> (String, String) {
            (rk_strings[k.0 as usize].clone(), k.1.clone())
        };

        let tokens = tok_agg.into_iter().map(|(k, v)| (resolve(&k), v)).collect();

        let daily_costs = daily_agg
            .into_iter()
            .map(|(k, v)| (resolve(&k), v))
            .collect();

        let minute_costs = if include_minute {
            minute_agg
                .into_iter()
                .map(|(k, v)| (resolve(&k), v))
                .collect()
        } else {
            HashMap::new()
        };

        ModelStats {
            tokens,
            daily_costs,
            minute_costs,
        }
    }

    /// Build a [`Cache`] containing only entries within the window
    /// `[start_date:start_minute .. end_date:*]`. Correctly handles windows
    /// that cross midnight. `active_minutes` is estimated from the distinct
    /// minutes present in the filtered window.
    pub fn build_subday_cache(
        &self,
        start_date: NaiveDate,
        start_minute: u16,
        end_date: NaiveDate,
    ) -> super::cache::Cache {
        use super::cache::Cache;

        let mut cache = Cache::new();
        let roots = &self.roots;

        // Collect distinct active minutes per (root, cwd, date) for active_minutes estimation.
        let mut active_minutes_map: HashMap<(u16, u16, NaiveDate), Vec<u16>> = HashMap::new();

        for e in &self.entries {
            if e.date < start_date || e.date > end_date {
                continue;
            }
            if e.date == start_date && e.minute < start_minute {
                continue;
            }
            let root = &roots[e.root_idx as usize];
            let cwd = &self.cwds[e.cwd_idx as usize];
            let date_str = e.date.format("%Y-%m-%d").to_string();

            let day_entry = cache
                .entry_root(root.clone())
                .entry(cwd.clone())
                .or_default()
                .entry(date_str)
                .or_default();

            day_entry.input += e.input_tokens;
            day_entry.output += e.output_tokens;
            day_entry.cache_read += e.cache_read;
            day_entry.cache_creation += e.cache_creation;
            day_entry.cost += e.cost;
            day_entry.lines_suggested += e.lines_suggested;
            day_entry.lines_accepted += e.lines_accepted;
            day_entry.lines_added += e.lines_added;
            day_entry.lines_deleted += e.lines_deleted;

            active_minutes_map
                .entry((e.root_idx, e.cwd_idx, e.date))
                .or_default()
                .push(e.minute);
        }

        // Estimate active_minutes using the same clustering approach as the main cache.
        for ((root_idx, cwd_idx, date), mut minutes) in active_minutes_map {
            minutes.sort();
            minutes.dedup();
            let active = super::cache::cluster_active_minutes(&minutes);
            let root = &roots[root_idx as usize];
            let cwd = &self.cwds[cwd_idx as usize];
            let date_str = date.format("%Y-%m-%d").to_string();
            if let Some(day_entry) = cache
                .get_root_mut(root)
                .and_then(|cwd_map| cwd_map.get_mut(cwd))
                .and_then(|days| days.get_mut(&date_str))
            {
                day_entry.active_minutes = active;
            }
        }

        cache
    }

    // ------------------------------------------------------------------
    // Token window query
    // ------------------------------------------------------------------

    /// Sum (input + output) tokens within a local time window for a given source root.
    pub fn tokens_in_window(
        &self,
        source_root: &str,
        start_local: chrono::NaiveDateTime,
        end_local: chrono::NaiveDateTime,
    ) -> u64 {
        let (inp, out) = self.tokens_in_window_split(source_root, start_local, end_local);
        inp + out
    }

    /// Return (input, output) tokens within a local time window for a given source root.
    pub fn tokens_in_window_split(
        &self,
        source_root: &str,
        start_local: chrono::NaiveDateTime,
        end_local: chrono::NaiveDateTime,
    ) -> (u64, u64) {
        let Some(&root_idx) = self.root_intern.get(source_root) else {
            return (0, 0);
        };
        let start_date = start_local.date();
        let end_date = end_local.date();
        let start_min = start_local.hour() as u16 * 60 + start_local.minute() as u16;
        let end_min = end_local.hour() as u16 * 60 + end_local.minute() as u16;

        let mut total_in = 0u64;
        let mut total_out = 0u64;
        for e in &self.entries {
            if e.root_idx != root_idx {
                continue;
            }
            if e.date < start_date || e.date > end_date {
                continue;
            }
            if start_date == end_date {
                if e.minute < start_min || e.minute > end_min {
                    continue;
                }
            } else if e.date == start_date {
                if e.minute < start_min {
                    continue;
                }
            } else if e.date == end_date && e.minute > end_min {
                continue;
            }
            total_in += e.input_tokens;
            total_out += e.output_tokens;
        }
        (total_in, total_out)
    }

    /// Split a single entry's pre-computed cost into
    /// (input_side_cost, cache_read_cost, output_cost).
    ///
    /// Across every model in the pricing table the ratios are constant
    /// (output = 5× input, cache_read = 1/10 input), so the *share* of cost
    /// going to each side depends only on the token mix — not on which
    /// model produced the entry. We compute those shares with universal
    /// weights, then split the per-entry real USD cost accordingly.
    ///
    /// `cache_creation` is grouped with fresh input: semantically it's
    /// data the user sent to the model (just destined for the cache), and
    /// Anthropic bills it at the input rate (×1.25 for the default 5-min
    /// TTL). Without this grouping the input slice is dominated by the
    /// raw `input_tokens` API field — which is typically just the latest
    /// user-message delta (~50 tokens) and barely visible.
    fn split_entry_cost(e: &CompactEntry) -> (f64, f64, f64) {
        // Universal cost weights (relative units, per token):
        //   fresh input    = 1
        //   cache_creation = 1.25 (5-min ephemeral TTL, Claude Code default)
        //   cache_read     = 0.1
        //   output         = OUTPUT_COST_WEIGHT (= 5)
        const CACHE_CREATION_WEIGHT: f64 = 1.25;
        let input_units = e.input_tokens as f64 + e.cache_creation as f64 * CACHE_CREATION_WEIGHT;
        let cache_units = e.cache_read as f64 * 0.1;
        let output_units = e.output_tokens as f64 * OUTPUT_COST_WEIGHT;
        let total_units = input_units + cache_units + output_units;
        if total_units <= 0.0 || e.cost <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let input_cost = e.cost * input_units / total_units;
        let cache_cost = e.cost * cache_units / total_units;
        let output_cost = e.cost - input_cost - cache_cost;
        (input_cost, cache_cost, output_cost)
    }

    /// Sum `(fresh_input_cost, cache_read_cost, output_cost)` within a
    /// local time window for a given source root.
    pub fn cost_in_window_split(
        &self,
        source_root: &str,
        start_local: chrono::NaiveDateTime,
        end_local: chrono::NaiveDateTime,
    ) -> (f64, f64, f64) {
        let Some(&root_idx) = self.root_intern.get(source_root) else {
            return (0.0, 0.0, 0.0);
        };
        let start_date = start_local.date();
        let end_date = end_local.date();
        let start_min = start_local.hour() as u16 * 60 + start_local.minute() as u16;
        let end_min = end_local.hour() as u16 * 60 + end_local.minute() as u16;

        let mut fresh_total = 0.0_f64;
        let mut cache_total = 0.0_f64;
        let mut out_total = 0.0_f64;
        for e in &self.entries {
            if e.root_idx != root_idx {
                continue;
            }
            if e.date < start_date || e.date > end_date {
                continue;
            }
            if start_date == end_date {
                if e.minute < start_min || e.minute > end_min {
                    continue;
                }
            } else if e.date == start_date {
                if e.minute < start_min {
                    continue;
                }
            } else if e.date == end_date && e.minute > end_min {
                continue;
            }
            let (f, c, o) = Self::split_entry_cost(e);
            fresh_total += f;
            cache_total += c;
            out_total += o;
        }
        (fresh_total, cache_total, out_total)
    }

    /// Build a per-model usage breakdown (tokens + cost split) within a
    /// local time window for a given source root. Entries with an empty
    /// model string (user-side events: lines_accepted, etc.) are skipped.
    pub fn per_model_breakdown_in_window(
        &self,
        source_root: &str,
        start_local: chrono::NaiveDateTime,
        end_local: chrono::NaiveDateTime,
    ) -> Vec<PerModelUsage> {
        let Some(&root_idx) = self.root_intern.get(source_root) else {
            return Vec::new();
        };
        let start_date = start_local.date();
        let end_date = end_local.date();
        let start_min = start_local.hour() as u16 * 60 + start_local.minute() as u16;
        let end_min = end_local.hour() as u16 * 60 + end_local.minute() as u16;

        let mut agg: HashMap<u16, PerModelUsage> = HashMap::new();
        for e in &self.entries {
            if e.root_idx != root_idx {
                continue;
            }
            if e.date < start_date || e.date > end_date {
                continue;
            }
            if start_date == end_date {
                if e.minute < start_min || e.minute > end_min {
                    continue;
                }
            } else if e.date == start_date {
                if e.minute < start_min {
                    continue;
                }
            } else if e.date == end_date && e.minute > end_min {
                continue;
            }
            let model_str = &self.models[e.model_idx as usize];
            if model_str.is_empty() {
                continue;
            }
            let (in_c, cache_c, out_c) = Self::split_entry_cost(e);
            let slot = agg.entry(e.model_idx).or_insert_with(|| PerModelUsage {
                model: model_str.clone(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                input_cost: 0.0,
                cache_cost: 0.0,
                output_cost: 0.0,
            });
            slot.input_tokens += e.input_tokens;
            slot.output_tokens += e.output_tokens;
            slot.cache_read_tokens += e.cache_read;
            slot.cache_creation_tokens += e.cache_creation;
            slot.input_cost += in_c;
            slot.cache_cost += cache_c;
            slot.output_cost += out_c;
        }
        let mut result: Vec<PerModelUsage> = agg
            .into_values()
            .filter(|m| m.total_tokens() > 0 || m.total_cost() > 0.0)
            .collect();
        result.sort_by(|a, b| {
            b.total_cost()
                .partial_cmp(&a.total_cost())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }

    // ------------------------------------------------------------------
    // Filter helpers
    // ------------------------------------------------------------------

    fn cwd_set(&self, cwds: &[String]) -> HashSet<u16> {
        cwds.iter()
            .filter_map(|c| self.cwd_intern.get(c).copied())
            .collect()
    }

    /// Whether an entry passes the source-root filter (by its own root, so the
    /// codex root is honored even though it shares cwds with Claude) and the
    /// optional cwd filter.
    fn entry_passes(
        &self,
        entry: &CompactEntry,
        roots: &RootFilter,
        cwd_filter: Option<&HashSet<u16>>,
    ) -> bool {
        roots.matches(&self.roots[entry.root_idx as usize])
            && cwd_filter.is_none_or(|cwds| cwds.contains(&entry.cwd_idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::codex::{CodexDelta, CODEX_ROOT};

    fn codex(date: NaiveDate, minute: u16, model: &str, input: u64, cache: u64, output: u64) -> CodexDelta {
        CodexDelta { cwd: "/p".into(), session_id: String::new(), date, minute, model: model.into(), input, cache_read: cache, output }
    }

    #[test]
    fn fold_codex_exposes_specific_models_in_per_model_breakdown() {
        let date = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
        let mut index = EventIndex::build(&[], &HashMap::new());
        index.fold_codex(&[
            codex(date, 600, "gpt-5.5", 1000, 4000, 200),
            codex(date, 601, "gpt-5.3-codex", 500, 0, 100),
        ]);

        let start = date.and_hms_opt(0, 0, 0).unwrap();
        let end = date.and_hms_opt(23, 59, 0).unwrap();
        let bd = index.per_model_breakdown_in_window(CODEX_ROOT, start, end);

        let models: Vec<&str> = bd.iter().map(|m| m.model.as_str()).collect();
        assert!(models.contains(&"gpt-5.5"), "got {models:?}");
        assert!(models.contains(&"gpt-5.3-codex"), "got {models:?}");

        let g55 = bd.iter().find(|m| m.model == "gpt-5.5").unwrap();
        assert_eq!(g55.input_tokens, 1000, "fresh input preserved");
        assert_eq!(g55.output_tokens, 200);
        assert_eq!(g55.cache_read_tokens, 4000);
        assert!(g55.total_cost() > 0.0);
    }

    #[test]
    fn build_model_stats_splits_codex_by_specific_model_and_keeps_claude_family() {
        use chrono::{TimeZone, Utc};

        let claude = Event {
            timestamp: Utc.with_ymd_and_hms(2026, 5, 4, 10, 0, 0).unwrap(),
            model: "claude-opus-4-6".into(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            cost_usd: 1.0,
            lines_suggested: 0,
            lines_accepted: 0,
            lines_added: 0,
            lines_deleted: 0,
            session_file: "s1.jsonl".into(),
            request_id: None,
            raw_cost_usd: None,
            line_uuid: None,
        };
        let mut session_info = HashMap::new();
        session_info.insert("s1.jsonl".to_string(), ("claude-root".to_string(), "/claude-proj".to_string()));

        let mut index = EventIndex::build(&[claude], &session_info);
        let date = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
        // Codex cwd "/p" is mapped to repo group "crab" via cwd_to_root,
        // so Codex per-model usage should attribute to the "crab" root key.
        index.fold_codex(&[
            codex(date, 600, "gpt-5.5", 1000, 4000, 200),
            codex(date, 601, "gpt-5.3-codex", 500, 0, 100),
        ]);

        let mut cwd_to_root = HashMap::new();
        cwd_to_root.insert("/claude-proj".to_string(), "claude-root".to_string());
        // Map Codex cwd "/p" to the "crab" repo group.
        cwd_to_root.insert("/p".to_string(), "crab".to_string());

        let stats = index.build_model_stats(&cwd_to_root, &RootFilter::All, &|_| true, None, false, None);

        // Claude collapses to family.
        assert_eq!(stats.tokens.get(&("claude-root".to_string(), "opus".to_string())), Some(&150));
        // Codex split by specific model, now grouped under the "crab" repo key (not CODEX_ROOT).
        assert_eq!(
            stats.tokens.get(&("crab".to_string(), "gpt-5.5".to_string())),
            Some(&1200),
            "input 1000 + output 200 under crab; keys: {:?}",
            stats.tokens.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            stats.tokens.get(&("crab".to_string(), "gpt-5.3-codex".to_string())),
            Some(&600)
        );
        // Codex models must NOT appear under CODEX_ROOT anymore.
        assert_eq!(
            stats.tokens.get(&(CODEX_ROOT.to_string(), "gpt-5.5".to_string())),
            None,
            "gpt-5.5 must not be under CODEX_ROOT; keys: {:?}",
            stats.tokens.keys().collect::<Vec<_>>()
        );

        // Source-tab isolation: Exclude(CODEX_ROOT) filters out Codex entries
        // entirely (entry_passes checks the entry's own root_idx = CODEX_ROOT).
        let stats_no_codex = index.build_model_stats(
            &cwd_to_root,
            &RootFilter::Exclude(CODEX_ROOT.to_string()),
            &|_| true, None, false, None,
        );
        assert_eq!(
            stats_no_codex.tokens.get(&("crab".to_string(), "gpt-5.5".to_string())),
            None,
            "Codex model must be absent from Claude-tab view"
        );

        // Only(CODEX_ROOT): only Codex entries pass, so gpt-5.x is present under "crab".
        let stats_only_codex = index.build_model_stats(
            &cwd_to_root,
            &RootFilter::Only(CODEX_ROOT.to_string()),
            &|_| true, None, false, None,
        );
        assert_eq!(
            stats_only_codex.tokens.get(&("crab".to_string(), "gpt-5.5".to_string())),
            Some(&1200),
            "gpt-5.5 must be present in Codex-only tab under crab"
        );
    }
}
