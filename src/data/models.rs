use serde::{Deserialize, Serialize};

pub(crate) const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// Output tokens cost ~5x more than input tokens across every model in
/// `PRICING_TABLE` below. Used to weight stacked bars by cost contribution.
pub(crate) const OUTPUT_COST_WEIGHT: f64 = 5.0;

/// Per-model token counts and USD cost contributions within a time window.
/// Stored in rate-history / hit-history so that the per-side cost split
/// (input / cache / output) survives even after the underlying JSONL
/// events are rotated out of the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerModelUsage {
    /// Full model string as seen in the JSONL (e.g. "claude-opus-4-6-...").
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub input_cost: f64,
    pub cache_cost: f64,
    pub output_cost: f64,
}

impl PerModelUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn total_cost(&self) -> f64 {
        self.input_cost + self.cache_cost + self.output_cost
    }
}

/// Sum `(input_cost, cache_cost, output_cost)` over a per-model breakdown.
pub fn per_model_cost_split(per_model: &[PerModelUsage]) -> (f64, f64, f64) {
    per_model.iter().fold((0.0, 0.0, 0.0), |acc, m| {
        (
            acc.0 + m.input_cost,
            acc.1 + m.cache_cost,
            acc.2 + m.output_cost,
        )
    })
}

/// Sum `(input_tokens + output_tokens)` over a per-model breakdown.
pub fn per_model_total_tokens(per_model: &[PerModelUsage]) -> u64 {
    per_model.iter().map(|m| m.total_tokens()).sum()
}

/// (pattern, (input_price, output_price, cache_read_price)) per million tokens.
const PRICING_TABLE: &[(&str, (f64, f64, f64))] = &[
    ("gpt-5", (1.25, 10.0, 0.125)),
    ("codex", (1.25, 10.0, 0.125)),
    ("opus-4-6", (5.0, 25.0, 0.50)),
    ("opus-4-5", (5.0, 25.0, 0.50)),
    ("opus-4-1", (15.0, 75.0, 1.50)),
    ("opus-4-0", (15.0, 75.0, 1.50)),
    ("opus-4-2", (15.0, 75.0, 1.50)),
    ("3-opus", (15.0, 75.0, 1.50)),
    ("sonnet", (3.0, 15.0, 0.30)),
    ("haiku-4-5", (1.0, 5.0, 0.10)),
    ("3-5-haiku", (0.80, 4.0, 0.08)),
    ("3-haiku", (0.25, 1.25, 0.03)),
];

const FALLBACK_PRICING: (f64, f64, f64) = (3.0, 15.0, 0.30);

/// Prix par million de tokens (input, output, cache_read) pour chaque modèle.
pub(crate) fn model_pricing(model: &str) -> (f64, f64, f64) {
    PRICING_TABLE
        .iter()
        .find(|(pattern, _)| model.contains(pattern))
        .map(|(_, pricing)| *pricing)
        .unwrap_or(FALLBACK_PRICING)
}

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

/// Normalize a full model ID to a short family name.
pub(crate) fn normalize_model(model: &str) -> &'static str {
    if model.contains("opus") {
        "opus"
    } else if model.contains("sonnet") {
        "sonnet"
    } else if model.contains("haiku") {
        "haiku"
    } else {
        "other"
    }
}

/// Format a token count as a human-readable string (e.g. "1.2M", "450K", "99").
pub(crate) fn format_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / TOKENS_PER_MILLION)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Format a USD cost as a compact human-readable string
/// (e.g. "¢42", "$1.23", "$45", "$1.2K").
pub(crate) fn format_cost(c: f64) -> String {
    if !c.is_finite() || c <= 0.0 {
        return "$0".to_string();
    }
    if c >= 1000.0 {
        format!("${:.1}K", c / 1000.0)
    } else if c >= 100.0 {
        format!("${:.0}", c)
    } else if c >= 1.0 {
        format!("${:.2}", c)
    } else {
        format!("¢{:.0}", c * 100.0)
    }
}

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

    #[test]
    fn format_tokens_uses_billions() {
        assert_eq!(format_tokens(24_400_000_000), "24.4B");
        assert_eq!(format_tokens(1_000_000_000), "1.0B");
        // Just under a billion stays in millions.
        assert_eq!(format_tokens(999_000_000), "999.0M");
    }

    #[test]
    fn codex_models_are_priced() {
        // gpt-5 priced (non-fallback): a pure-output cost is nonzero.
        let c = cost_from_tokens("gpt-5.5", 0, 1_000_000, 0, 0);
        assert!(c > 0.0);
    }
}
