use std::path::PathBuf;

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};

use super::models::{PerModelUsage, per_model_total_tokens};
use super::rate_limits::RateLimitHit;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitHistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub source_root: String,
    /// Per-model tokens + cost split observed across the session leading up
    /// to the hit. `tokens = sum(input + output)` is derivable on demand.
    #[serde(default)]
    pub per_model: Vec<PerModelUsage>,
    /// Session duration in minutes (time from first assistant message to hit).
    #[serde(default)]
    pub session_duration_min: Option<f64>,
    pub dedup_key: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HitHistory {
    pub entries: Vec<HitHistoryEntry>,
}

/// Build a dedup key matching the 15-minute bucket logic in rate_limits.rs.
pub fn dedup_key(ts: &DateTime<Utc>, source_root: &str) -> String {
    format!(
        "{}-{:02}-{}",
        ts.format("%Y-%m-%dT%H"),
        ts.minute() / 15 * 15,
        source_root,
    )
}

fn history_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".config")
        .join("ccmeter")
        .join("usage-hit-history.json")
}

pub fn load() -> HitHistory {
    let path = history_path();
    if !path.exists() {
        return HitHistory::default();
    }
    let mut history: HitHistory = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let cutoff = chrono::Utc::now() - chrono::Duration::days(90);
    history.entries.retain(|e| e.timestamp >= cutoff);

    history
}

pub fn save(history: &HitHistory) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::write(&path, &json);
        }
    }
}

/// Result of merging fresh hits into persisted history.
pub struct MergeResult {
    /// The full merged hit list, sorted descending by timestamp.
    pub hits: Vec<RateLimitHit>,
    /// `true` when the merge either added a brand-new bucket or enriched an
    /// existing entry (populated `per_model` / `session_duration_min` that
    /// were previously missing). Used by the caller to decide whether to
    /// persist history to disk.
    pub changed: bool,
}

impl HitHistory {
    /// Merge fresh hits into persisted history (dedup by bucket key).
    ///
    /// The merge is asymmetric: on a collision we keep the richer value on
    /// each field so a fresh scan that can no longer compute
    /// `per_model` / `session_duration_min` (e.g. the source JSONL has been
    /// rotated out) doesn't wipe data that was previously resolved.
    pub fn merge_fresh_hits(&mut self, fresh_hits: &[RateLimitHit]) -> MergeResult {
        let mut changed = false;

        for hit in fresh_hits {
            let key = dedup_key(&hit.timestamp, &hit.source_root);

            match self.entries.iter_mut().find(|e| e.dedup_key == key) {
                Some(existing) => {
                    let new_per_model = if hit.per_model.is_empty() {
                        existing.per_model.clone()
                    } else {
                        hit.per_model.clone()
                    };
                    let new_duration = hit.session_duration_min.or(existing.session_duration_min);

                    let per_model_differs = new_per_model.len() != existing.per_model.len()
                        || per_model_total_tokens(&new_per_model)
                            != per_model_total_tokens(&existing.per_model);
                    if per_model_differs || new_duration != existing.session_duration_min {
                        changed = true;
                    }

                    existing.per_model = new_per_model;
                    existing.session_duration_min = new_duration;
                    existing.timestamp = hit.timestamp;
                }
                None => {
                    changed = true;
                    self.entries.push(HitHistoryEntry {
                        timestamp: hit.timestamp,
                        source_root: hit.source_root.clone(),
                        per_model: hit.per_model.clone(),
                        session_duration_min: hit.session_duration_min,
                        dedup_key: key,
                    });
                }
            }
        }

        let mut hits: Vec<RateLimitHit> = self
            .entries
            .iter()
            .map(|e| RateLimitHit {
                timestamp: e.timestamp,
                message: String::new(),
                source_root: e.source_root.clone(),
                session_duration_min: e.session_duration_min,
                tokens: per_model_total_tokens(&e.per_model),
                per_model: e.per_model.clone(),
            })
            .collect();

        hits.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        MergeResult { hits, changed }
    }
}
