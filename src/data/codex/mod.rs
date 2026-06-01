//! Live parsing of OpenAI Codex CLI usage (~/.codex/sessions). Folded into the
//! daily cache under the `codex` root so it aggregates with Claude in "All".

pub mod parser;

use chrono::NaiveDate;

/// One per-turn token delta attributed to a (cwd, date, model).
#[derive(Debug, Clone, PartialEq)]
pub struct CodexDelta {
    pub cwd: String,
    pub date: NaiveDate,
    pub model: String,
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
}
