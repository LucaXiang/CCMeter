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

### Key architectural insight (why no daily-cache schema bump)

- `build_cards` enumerates each `ProjectGroup`'s **cwd set** and pulls cache
  entries via `cache.iter_filtered(rootFilter, cwds)`
  (`src/ui/cards/data.rs:68`, `src/data/cache.rs:148`). Claude and Codex share
  the same cwd string for the same repo, and Codex stays keyed by `CODEX_ROOT`.
- **`CODEX_ROOT` is provider *isolation via `RootFilter`*, not the project
  key.** Precisely: the source tabs (`All` / `Exclude(codex)` / `Only(codex)`)
  filter by the cache/entry root, which doubles as a provider tag. The project
  identity is a separate axis (the `ProjectGroup` / cwd→root_key mapping).
- **Consequence (confirmed by review):** under `RootFilter::All`, Codex usage
  **already leaks into a Claude card's totals** whenever the card's cwd set
  contains a cwd Codex also used (e.g. `/Users/xzy/workspace/crab`). That is an
  accidental, *partial* unification today (main cwd only; no worktrees, no
  Codex-only repos, no split). This design makes it **controlled**: complete the
  group's cwd set (incl. Codex worktree cwds), add the provider split, and create
  cards for Codex-only repos.
- The **daily cache** (`root→cwd→date→DayEntry`) needs **no
  `CURRENT_SCHEMA_VERSION` bump** — unification happens at the grouping layer.
  The **new identity sidecar (① below) is a separate persisted artifact and
  carries its own schema version + invalidation rules.**

## Decisions (locked)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Card model | **A — unified per-repo card**: one card per repository, combining Claude + Codex; provider split on the card. |
| 2 | Grouping algorithm | **Reuse** existing `resolve_identity` / `group_by_identity`; extend, do not rewrite. Canonical key = normalized git remote URL. |
| 3 | Robustness for deleted worktrees / missing git | **(c)** persisted `cwd → identity` cache (preferred) **+** path-pattern worktree-stripping fallback. |
| 4 | Card-face provider split | **A — show split on the card** (e.g. `CC $30 · CX $12` / mini bar); model breakdown + session list live in the detail view. |

## Architecture

### ① Identity resolution + persistence — `config/discovery.rs` (+ new persisted sidecar)

- New persisted, **independently schema-versioned** sidecar:
  `cwd → ResolvedIdentity { remote_url, canonical_root, source, observed_at }`
  (JSON, alongside the daily cache). `source` ∈ {live-git, codex-seed,
  path-fallback}.
- **Resolution order (corrected per review — live-first when the cwd exists):**
  1. **cwd exists on disk → live git** (`resolve_identity` as today); on
     success **write through** to the sidecar (refreshing any stale entry).
     If live git's remote ≠ the persisted remote, the persisted entry is
     **invalidated** (a repo's remote changed / a path was reused).
  2. **cwd gone → persisted sidecar** hit (this is the deleted-worktree case).
  3. **else → path-pattern fallback** (below).
- **Codex seeds the sidecar:** each Codex `(cwd, repository_url)` is written in
  (`source = codex-seed`), so a Claude worktree cwd that no longer exists on disk
  still resolves (Codex saw the same cwd with its URL). Live git still wins over
  a seed when the cwd exists. Mutual reinforcement, no precedence inversion.
- **Path-pattern fallback** must extend today's `heuristic_root` (which only
  strips `/worktrees/` and `/Worktrees/`, `src/config/discovery.rs:427`) to also
  strip `/.claude/worktrees/<x>` and `/<name>-worktrees/<x>`. Table-driven tests
  guard against false merges (e.g. `crab-red-coral` must NOT collapse into
  `crab` by prefix).
- Normalize remote URLs so `git@host:org/repo.git` and `https://host/org/repo[.git]`
  collapse to one key (strip scheme/credentials, drop trailing `.git`, lowercase
  host + path).
- **`canonical_root` stays the shortest repo root path** (today's group root for
  Claude) so existing overrides keyed by `root_path` keep matching — see
  Overrides compatibility below.

### ② Codex joins grouping — pipeline restructure (the [P1] of this design)

The current pipeline does **Claude discovery first, then parses Codex inside
`load_data`** (`src/app.rs:192`, `:1017`); the refresh path splits them further
(discovery rebuilds groups without Codex at `:1046`; reload parses Codex without
rebuilding groups). **Identity seeding and Codex-in-groups cannot work in that
order.** Phase 1 therefore **unifies the pipeline**:

1. Collect Claude `ProjectSource`s **and** parse Codex session metadata
   (`session_meta` → `(session_id, cwd, repository_url, repo_root)`) up front.
2. Seed the identity sidecar from Codex `repository_url`s, then resolve every
   cwd (live-first per ①).
3. Build **one** unified group set + `cwd→root_key` map from both providers, then
   build the cache + index from that.
4. **Both** the initial load (`App::new`/`load_data`) and the refresh/reload
   paths run this single pipeline — no Claude-only vs Codex-only split.

- **Codex must produce real `ProjectSource`/`ProjectGroup` entries** (not merely
  be "fed into" grouping): `build_cards` iterates `groups` (`src/ui/cards/data.rs:56`),
  which today are Claude-only (`src/app.rs:913` documents "Codex has no
  ProjectGroup"). Codex sources carry `provider = Codex` and their cwd (incl.
  Codex-only repos and Codex worktree cwds Claude never used).
  - Same canonical remote as a Claude group → merged in (its cwd set gains the
    Codex cwds).
  - Codex-only repo, or no git info → its own group/card.
- **Cache/index keep Codex under `CODEX_ROOT`** (provider tag for the source
  tabs preserved). Only the *group's cwd set* changes, which is what
  `build_cards`' `iter_filtered` keys on.
- Remove the `entry_root == CODEX_ROOT ⇒ separate rk` special-case in
  `EventIndex::build_model_stats` (`src/data/index.rs:345`) so Codex per-model
  usage resolves to its repo group. Isolation for the `Claude Code` (`Exclude`)
  / `Codex` (`Only`) tabs still holds because `entry_passes`
  (`src/data/index.rs:707`) filters by the entry's actual root **before**
  root-key mapping.
- **Retire / redesign the dedicated Codex per-model panel.** `build_codex_breakdown`
  reads only `CODEX_ROOT`-keyed stats (`src/app.rs:918`); once Codex resolves to
  repo-root keys it would render **empty**. Codex per-model usage instead surfaces
  through the **per-card model breakdown** (`model_daily_costs`/`model_shares`,
  which already support non-Claude model labels). The `CodexBreakdown` struct and
  its renderer are removed or repurposed in Phase 1.

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

### Overrides compatibility (per review [P2])

Overrides (star / rename / hide) are keyed by `root_key == root_path`
(`src/ui/cards/data.rs:57`). Because `canonical_root` stays the shortest repo
root path (① above), **existing Claude overrides keep matching** — the crab
group's root_key is unchanged. New Codex-only groups, and any group whose
canonical root genuinely shifts, may need re-starring; note this in the changelog.
No automatic override migration in Phase 1 unless a root_key is observed to
change for an existing group (then add a one-time remap).

## Phasing (each phase: TDD red→green, atomic conventionally-typed commits)

1. **Unified grouping + pipeline restructure** — ① + ②. Single load/refresh
   pipeline (collect Claude + Codex metadata → seed/resolve identities → build
   unified groups → build cache/index). Codex produces real `ProjectGroup`s
   (incl. Codex-only + worktree cwds); `build_model_stats` special-case removed;
   the dedicated `CodexBreakdown` panel retired in favour of the per-card model
   breakdown. Identity sidecar with its own schema version. Verifiable: `crab`
   card appears with combined usage in `All`, isolates under the `Claude Code` /
   `Codex` tabs, and a Codex-only repo gets its own card; deleted-worktree cwds
   still collapse via the sidecar.
2. **Card-face provider split** — ③.
3. **Recent sessions list** — ④ (the original #1/#4, generalized to both
   providers).

Phase 1 is the large, load-bearing phase (data pipeline + grouping). Phases 2–3
are additive UI on top of the unified model.

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

## Independent review (Codex, 2026-06-02)

An independent Codex review read the actual code and returned "not safe as
written" with 4 [P1] and 3 [P2] findings. None contradicted the approved
direction (unified cards / live-first persistence / reuse the grouping algorithm
/ card-face split); all were implementation-precision gaps, now folded in above:

- **[P1] provider-dimension wording** → reworded: `CODEX_ROOT` is RootFilter
  isolation, not the project key; the Codex→Claude-card leak already happens for
  shared cwd and this design makes it controlled.
- **[P1] removing the special-case empties the Codex panel** → Phase 1 retires
  `CodexBreakdown`; per-card model breakdown carries Codex models.
- **[P1] Codex-only repos get no card** → Codex must produce real
  `ProjectSource`/`ProjectGroup` entries.
- **[P1] pipeline ordering** → Phase 1 unifies the load/refresh pipeline so Codex
  metadata is collected before grouping/seeding.
- **[P2] resolution order** → flipped to live-git-first when cwd exists, sidecar
  only for gone cwds, with remote-change invalidation.
- **[P2] weak path fallback** → extend `heuristic_root` patterns + false-merge
  tests.
- **[P2] overrides drift** → keep `canonical_root` = shortest repo root so
  existing overrides match; remap only if a root_key actually changes.
