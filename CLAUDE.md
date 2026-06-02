# CCMeter — agent guide

Terminal dashboard (Rust + ratatui) for Claude Code **and** OpenAI Codex usage:
tokens, cost, model breakdown, live rate-limit tracking. Reads local session
logs only — no network except Anthropic OAuth usage polling.

## Build & test (IMPORTANT)

This machine's `stable` rustup toolchain has **no cargo component**, so plain
`cargo` fails with *"the 'cargo' binary … is not applicable"*. Always pin a
versioned toolchain:

```bash
cargo +1.95.0 build --bin ccmeter
cargo +1.95.0 build --release --bin ccmeter
cargo +1.95.0 test            # whole suite (binary crate — no lib target)
cargo +1.95.0 clippy --bin ccmeter
```

- Binary crate (no lib): `cargo test --lib` fails; use `cargo test` or `--bin`.
- Let-chains / `is_none_or` are used → needs a recent toolchain (≥1.88).
- Install target dir is `~/.cargo-target`; installed binary is `~/.cargo/bin/ccmeter`.
- **Replacing the installed binary**: copy to a temp then `mv -f` (atomic). A
  plain `cp` over the running path can hand a half-written binary to a launch
  in progress → `SIGKILL (9)` (not a code bug).
- **Known-failing tests (pre-existing, unrelated):**
  `rate_limits::detects_rate_limit_hit` and `::deduplicates_same_minute` —
  fail on clean `HEAD` too (look date-relative). Don't treat as regressions.

When grepping code, RTK rewrites/compresses Bash output and mangles symbol
names (e.g. `PerModelUsage`→`nUsage`). Use `rtk proxy rg …` for faithful output.

## Architecture

```
src/data/
  parser.rs      Claude JSONL → Event (input/output/cache tokens, cost, lines)
  index.rs       EventIndex: compact per-(root,cwd,model,date,minute) entries;
                 build_model_stats / build_minute_tokens / per_model_breakdown /
                 cost_in_window_split. fold_codex() injects Codex here.
  cache.rs       Daily cache (root→cwd→date→DayEntry), schema-versioned, merged
                 high-water-mark. RootFilter { All, Exclude(r), Only(r) }.
  models.rs      Pricing table + cost_from_tokens, normalize_model,
                 model_breakdown_label, format_tokens (K/M/B), format_cost.
  codex/         OpenAI Codex provider (~/.codex):
    parser.rs    session JSONL → CodexDelta (deltaized cumulative usage)
    mod.rs       collect_codex_deltas / aggregate → cache fragment (CODEX_ROOT)
    rate.rs      parse rate_limits snapshots → 5h/7d windows (rate-tracking)
  oauth.rs       Claude OAuth usage polling + the synthetic Codex credential
  rate_limits.rs / rate_history.rs   rate-limit hits + recorded session history
  backfill/      historical backfill from stats-cache / code-insights
src/ui/
  dashboard.rs   top tabs (period / source), heatmaps, KPIs, cards / detail
  cards/         project cards, detail charts, Codex per-model panel
  rate_tracking/ live 5h/7d status, hits, session pacing, daily session cost
  theme.rs       colors; model_color() (Claude families fixed, others hashed)
```

## Key domain concepts

- **Providers / source selector:** when Codex usage exists the source tabs are
  `All` / `Claude Code` (`Exclude(codex)`) / `Codex` (`Only(codex)`).
- **Cost model:** `input` is stored **fresh** (cache-exclusive) for both
  providers; `cost_from_tokens` expects cache-INCLUSIVE input and re-derives
  fresh internally. Costs are **API-equivalent estimates** (Codex/Claude
  subscriptions are flat-rate — the $ figure is "what API metering would bill",
  dominated by cache re-reads). gpt-* pricing is best-effort (gpt-5 rates).
- **Model breakdown:** keyed by `model_breakdown_label` — Claude collapses to a
  family (opus/sonnet/haiku); other providers keep the specific model
  (gpt-5.5 / gpt-5.3-codex). Legends/`model_order` are dynamic, not hardcoded.
- **Codex specifics:** Codex shares cwd strings with Claude projects, so index
  aggregation keys Codex strictly by `CODEX_ROOT` (never folds into a Claude
  project group) — otherwise it leaks across the source views. Codex has no
  ProjectGroup/card; its per-model usage shows via a dedicated panel.

## Conventions

- TDD (red→green); atomic, conventionally-typed commits; feature branch off
  `main`. Commit/PR co-author trailer per global guide.
- Bump `CURRENT_SCHEMA_VERSION` (cache.rs) only when stored values become
  inconsistent with a fresh parse.

## Possible future features (data already on disk)

- **Session titles/goals:** `~/.codex/session_index.jsonl` (thread_name) and
  `goals_1.sqlite::thread_goals` (goal text + tokens) — name Codex sessions
  instead of bare cwd. Claude session summaries are analogous.
- **Codex rate-limit hits:** synthesize from `rate_limits` 5h/7d `used_percent`
  peaks (≈95%+) to fill "Recent rate-limit hits" / "Last hit" for Codex
  (`rate_limit_reached_type` is never set, so use the percentage crossings).
- **Plan-tier history:** Codex `plan_type` evolves free→prolite→pro — show the
  subscription timeline / upgrade dates.
- **Tool & git activity:** Codex sessions log tool calls (exec_command,
  apply_patch…), failures, git commits and lines changed — a productivity panel.
- **Active-hours heatmap** and **MCP/web-search usage** (in both providers'
  `usage-data/` reports; note those report.json files are point-in-time
  snapshots, not live).
- **Value multiplier:** "$X API-equivalent ÷ $Y subscription = N× value".
