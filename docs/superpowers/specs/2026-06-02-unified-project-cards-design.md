# Unified project cards (Claude + Codex) — design

- **Date:** 2026-06-02
- **Status:** Approved (brainstorming → spec)
- **Branch:** `feat/unified-project-cards`
- **Supersedes the RESUME.md scope:** the original task #1 ("Recent Codex
  sessions" flat list) grew into a unified per-repo project-card model spanning
  both providers. Session titles (#1) become one phase of that.

## Problem

CCMeter shows Claude Code usage as per-project **cards** (grouped by git
identity, worktrees collapsed to the main repo). Codex has **no cards** — all
Codex usage is lumped under a single synthetic `CODEX_ROOT` and surfaced only as
a flat per-model panel. The user works on the same repos (e.g. `crab`) with both
Claude and Codex, often from many git worktrees, and wants:

1. **Codex to have real project cards**, grouped by the actual repository
   (all `crab` worktrees → one `crab` card), exactly like Claude.
2. **Claude + Codex unified per repo**: in the `All` view, one `crab` card shows
   combined usage with a per-provider split (the Codex Desktop mental model:
   one project = one repo).
3. Robust grouping that survives **deleted worktrees** (the live-git path fails
   for them today).
4. **Recent sessions with titles** under each card (Claude `ai-title`,
   Codex `thread_name`).

## Investigation findings (ground truth)

- **Codex stores git identity in the session file.** `session_meta.payload.git`
  carries `repository_url` (e.g. `git@github.com:LucaXiang/Crab.git`), `branch`,
  and sometimes `repo_root` / `commit_hash`. 154 of ~197 sampled sessions carry
  it; the rest (e.g. `~/.claude-mem/observer-sessions`) have none.
- **All `crab` worktree styles share one `repository_url`** — both
  `/crab/.claude/worktrees/<x>` and `/crab-worktrees/<x>` resolve to
  `git@github.com:LucaXiang/Crab.git`. Grouping by it collapses worktrees and is
  **deletion-proof** (it is a stored snapshot, not computed live).
- **Claude stores only `cwd` + `gitBranch`** in its JSONL — **no remote URL,
  no repo root**. Grouping relies on live `git` (`resolve_identity`), which:
  - works when the cwd still exists (most cases — many worktrees already group
    into `crab` correctly today);
  - **fails for deleted worktrees** → falls to `heuristic_root` and mis-groups
    (`crab/.claude/worktrees/<deleted>` → `.../crab/.claude`; `crab-worktrees/<x>`
    → its own group). This is the fragility to fix.
- **When Claude *can* resolve, its remote URL string is identical to Codex's**
  (`git@github.com:LucaXiang/Crab.git`) → a shared canonical key works.
- **Claude session titles exist** as `{"type":"ai-title","aiTitle":"…",
  "sessionId":"…"}` entries (fallback: first prompt / cwd basename).
- **Codex thread names** are in `~/.codex/session_index.jsonl`
  (`{id, thread_name, updated_at}`).

### Key architectural insight (why no schema bump)

- `build_cards` enumerates each `ProjectGroup`'s **cwd set** and pulls cache
  entries via `cache.iter_filtered(rootFilter, cwds)`. Claude and Codex share
  the same cwd string for the same repo, and Codex stays keyed by `CODEX_ROOT`.
- **`CODEX_ROOT` already IS the provider dimension.** The source tabs
  (`All` / `Exclude(codex)` / `Only(codex)`) already filter by provider via the
  cache root. Unification therefore happens at the **grouping layer**, not in the
  storage schema. No `CURRENT_SCHEMA_VERSION` bump is required for the cache.

## Decisions (locked)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Card model | **A — unified per-repo card**: one card per repository, combining Claude + Codex; provider split on the card. |
| 2 | Grouping algorithm | **Reuse** existing `resolve_identity` / `group_by_identity`; extend, do not rewrite. Canonical key = normalized git remote URL. |
| 3 | Robustness for deleted worktrees / missing git | **(c)** persisted `cwd → identity` cache (preferred) **+** path-pattern worktree-stripping fallback. |
| 4 | Card-face provider split | **A — show split on the card** (e.g. `CC $30 · CX $12` / mini bar); model breakdown + session list live in the detail view. |

## Architecture

### ① Identity resolution + persistence — `config/discovery.rs` (+ new persisted store)

- New persisted map `cwd → ResolvedIdentity { remote_url, canonical_root }`
  (JSON sidecar in the existing cache dir). Resolution order:
  1. **persisted cache** hit → use it;
  2. **live git** (`resolve_identity` as today) → on success, **write back** to
     the persisted cache;
  3. **path-pattern fallback** — strip the first matching worktree segment
     (`/.claude/worktrees/<x>`, `/<name>-worktrees/<x>`, `/worktrees/<x>`) and use
     the prefix as the canonical root.
- **Codex seeds the persisted cache**: each Codex `(cwd, repository_url)` is
  written in, so a Claude worktree cwd that no longer exists on disk still
  resolves (Codex saw the same cwd with its URL). Mutual reinforcement.
- Normalize remote URLs so `git@host:org/repo.git` and `https://host/org/repo[.git]`
  collapse to one key (strip scheme/credentials, drop trailing `.git`, lowercase
  host + path).

### ② Codex joins grouping — `data/codex/` + `config/discovery.rs`

- New Codex session-metadata parse: `session_meta` → `(session_id, cwd,
  repository_url, repo_root)`. (Separate from the token-delta parser.)
- Feed Codex `(cwd, pre-resolved identity)` into `group_by_identity` alongside
  Claude sources. Codex sources are flagged `provider = Codex`.
  - Same remote as a Claude group → merged into that group (its cwd set gains the
    Codex cwds, including each worktree cwd).
  - Codex-only repo, or no git info → its own group/card.
- **Cache/index keep Codex under `CODEX_ROOT`** (provider tag preserved). Only
  the *group's cwd set* changes, which is what `build_cards`' `iter_filtered`
  keys on.
- Remove the `entry_root == CODEX_ROOT ⇒ separate rk` special-case in
  `EventIndex::build_model_stats`, so Codex per-model usage resolves to its repo
  group. Provider isolation for the `Claude Code` / `Codex` tabs remains via
  `entry_passes` (RootFilter on the entry root).

### ③ Card-face provider split — `ui/cards/data.rs` + `ui/cards/render.rs`

- In `build_cards` accumulation, split totals into `(claude, codex)` by
  `root == CODEX_ROOT`.
- `ProjectCard` gains `cost_claude` / `cost_codex` (extend to tokens if the
  render needs it). Render a compact split indicator on the card face.

### ④ Recent sessions with titles — `data/codex/` + `data/parser.rs`/index + `ui/cards`

- **Codex:** add `session_id` to `CodexDelta`; aggregate per session
  (tokens / cost / last_date); resolve `thread_name` from
  `~/.codex/session_index.jsonl`.
- **Claude:** aggregate per session file (tokens / cost / date); title from the
  `ai-title` entry, falling back to first user prompt, then cwd basename.
- Map each session to its group via cwd→group. The card **detail view** lists
  recent sessions: `title · tokens · cost · date · provider(CC/CX)`, sorted by
  last activity descending, top-N to fit.

## Phasing (each phase: TDD red→green, atomic conventionally-typed commits)

1. **Unified grouping** — ① + ②. Codex gets cards, worktrees collapse,
   persisted+fallback identity, source tabs still filter by provider. Verifiable:
   `crab` card appears in the Codex view and combines both providers in `All`.
2. **Card-face provider split** — ③.
3. **Recent sessions list** — ④ (the original #1/#4, generalized to both
   providers).

## Defaults (adjustable)

- Card name from `derive_group_name(canonical_root)` (e.g. `crab`).
- Session list: sort by last activity desc, show as many as fit the panel.
- Persisted identity cache updated incrementally as cwds are resolved.

## Out of scope (this spec)

- Productivity panel (#4 tool/git activity) from RESUME.md — separate effort.
- Plan-tier history, active-hours heatmap, MCP/web-search usage.
- Merging Claude and Codex into a *single* per-session timeline (sessions remain
  provider-tagged rows).

## Testing strategy

- Pure functions are unit-tested (TDD): URL normalization, path-pattern
  fallback, persisted-cache resolution order, Codex `session_meta` parse,
  per-session aggregation (both providers), provider split accumulation.
- Grouping: table-driven tests over representative cwds (main repo, both
  worktree styles, deleted worktree via persisted cache, no-git cwd).
- Build/test gate per CLAUDE.md: `cargo +1.95.0 build --bin ccmeter`,
  `cargo +1.95.0 test`, `cargo +1.95.0 clippy --bin ccmeter` (no new warnings;
  the 2 known date-relative `rate_limits` failures are pre-existing).
