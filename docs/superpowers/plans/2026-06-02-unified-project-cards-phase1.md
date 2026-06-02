# Unified project cards — Phase 1 (grouping + pipeline) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Group Codex usage into per-repo project cards (worktrees collapsed via the stored git `repository_url`), unified with Claude per repo, so the `crab` card combines both providers in `All` and isolates under the `Claude Code` / `Codex` source tabs — including Codex-only repos and deleted-worktree cwds.

**Architecture:** Codex usage already flows into the daily cache + `EventIndex` under `CODEX_ROOT` via `load_data` (`collect_codex_deltas`/`aggregate`/`fold_codex`). The only missing axis is **grouping**: `ProjectGroup`s come from Claude discovery only, so cards never enumerate Codex cwds. Phase 1 makes **discovery provider-aware** in one place (used by both initial load and refresh): collect Claude sources + Codex session metadata, resolve every cwd to a canonical git identity (live-git-first, persisted sidecar for gone cwds, path-pattern fallback), and emit one unified group set whose cwd sets include Codex cwds. `CODEX_ROOT` stays the provider tag for the source-tab `RootFilter`; no daily-cache schema bump. The dedicated `CodexBreakdown` panel is retired (per-card model breakdown carries Codex models).

**Tech Stack:** Rust 2021, ratatui; `serde`/`serde_json`; build/test pinned to `cargo +1.95.0` (the machine's `stable` has no cargo component). Binary crate — no `--lib`.

**Spec:** `docs/superpowers/specs/2026-06-02-unified-project-cards-design.md`

---

## File Structure

**New files**
- `src/config/identity.rs` — `ResolvedIdentity`, `normalize_remote_url`, `strip_worktree_segment` (path fallback), and the persisted identity **sidecar** (`~/.config/ccmeter/identities.json`, own `schema_version`): load / save / `resolve(cwd, seeds)` with the live-first → persisted → path order.
- `src/data/codex/sessions.rs` — `CodexSessionMeta { session_id, cwd, repository_url, repo_root }`; `parse_session_meta(raw) -> Option<CodexSessionMeta>`; `collect_codex_session_meta() -> Vec<CodexSessionMeta>` (reuses `discover_session_files`). (Thread-name reading lands in Phase 3.)

**Modified files**
- `src/config/discovery.rs` — add `provider` to `ProjectSource`; route `resolve_identity` through the sidecar (seeded by Codex `repository_url`s); extend `heuristic_root` worktree patterns; new public `discover_project_groups_unified()` that folds Codex sources into `group_by_identity`.
- `src/app.rs` — `App::new` and `spawn_discovery` call `discover_project_groups_unified()`; drop `CodexBreakdown` from `RenderCache`/`build_render_cache`; `build_codex_breakdown` removed.
- `src/data/index.rs` — remove the `entry_root == CODEX_ROOT ⇒ separate rk` special-case in `build_model_stats`.
- `src/ui/cards/render.rs` + `src/ui/dashboard.rs` — remove `render_codex_breakdown` and its call sites.

**Provider tag:** `ProjectSource.provider` is an enum `Provider { Claude, Codex }`. Cache/index keep Codex under `CODEX_ROOT`; the enum is only used during grouping/discovery and (Phase 2) for the card split.

---

## Task 1: Remote-URL normalization

**Files:**
- Create: `src/config/identity.rs`
- Modify: `src/config/mod.rs` (add `pub mod identity;`)

- [ ] **Step 1: Write the failing test**

In `src/config/identity.rs`:

```rust
//! Canonical git identity resolution shared by Claude + Codex discovery:
//! normalize remote URLs, collapse worktree paths, and persist resolved
//! identities so deleted worktrees still group correctly.

/// Canonicalize a git remote URL so `git@host:org/repo.git` and
/// `https://host/org/repo[.git]` collapse to one key: `host/org/repo`,
/// lowercased, no scheme/credentials, no trailing `.git`.
pub fn normalize_remote_url(url: &str) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ssh_and_https_to_same_key() {
        let ssh = normalize_remote_url("git@github.com:LucaXiang/Crab.git");
        let https = normalize_remote_url("https://github.com/LucaXiang/Crab.git");
        let https_no_git = normalize_remote_url("https://github.com/LucaXiang/Crab");
        assert_eq!(ssh, "github.com/lucaxiang/crab");
        assert_eq!(https, ssh);
        assert_eq!(https_no_git, ssh);
    }

    #[test]
    fn distinct_repos_stay_distinct() {
        assert_ne!(
            normalize_remote_url("git@github.com:LucaXiang/Crab.git"),
            normalize_remote_url("git@github.com:LucaXiang/crab-red-coral.git"),
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests::normalizes`
Expected: panic from `todo!()` (or compile error until `mod identity;` is added — add it now).

- [ ] **Step 3: Implement `normalize_remote_url`**

```rust
pub fn normalize_remote_url(url: &str) -> String {
    let s = url.trim();
    // Strip scheme + optional credentials: `scheme://user@` → ``.
    let s = s.split("://").last().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s); // drop `user@` / creds
    // SSH shorthand uses `host:org/repo`; HTTP uses `host/org/repo`.
    let s = s.replacen(':', "/", 1);
    let s = s.strip_suffix(".git").unwrap_or(s);
    s.trim_end_matches('/').to_lowercase()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/identity.rs src/config/mod.rs
git commit -m "feat(identity): canonical git remote-URL normalization"
```

---

## Task 2: Worktree path-pattern fallback

**Files:**
- Modify: `src/config/identity.rs`

- [ ] **Step 1: Write the failing test**

Add to `identity.rs` (above `mod tests` add the fn signature; add cases inside `tests`):

```rust
/// Best-effort canonical repo path for a cwd with no resolvable git remote:
/// strip the first worktree segment. Returns `None` when no known worktree
/// pattern is present (caller then uses the cwd as-is).
pub fn strip_worktree_segment(cwd: &str) -> Option<String> {
    todo!()
}
```

```rust
    #[test]
    fn strips_known_worktree_layouts() {
        assert_eq!(
            strip_worktree_segment("/Users/xzy/workspace/crab/.claude/worktrees/crab-launcher"),
            Some("/Users/xzy/workspace/crab".to_string())
        );
        assert_eq!(
            strip_worktree_segment("/Users/xzy/workspace/crab-worktrees/const-batch0-exec"),
            Some("/Users/xzy/workspace/crab".to_string())
        );
        assert_eq!(
            strip_worktree_segment("/Users/xzy/code/proj/worktrees/feat-x"),
            Some("/Users/xzy/code/proj".to_string())
        );
    }

    #[test]
    fn leaves_non_worktree_paths_and_avoids_false_merges() {
        assert_eq!(strip_worktree_segment("/Users/xzy/workspace/crab"), None);
        // `crab-red-coral` is a different repo; must NOT collapse to `crab`.
        assert_eq!(strip_worktree_segment("/Users/xzy/workspace/crab-red-coral"), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests::strips`
Expected: panic from `todo!()`.

- [ ] **Step 3: Implement `strip_worktree_segment`**

```rust
pub fn strip_worktree_segment(cwd: &str) -> Option<String> {
    // `<root>/.claude/worktrees/<x>` and `<root>/worktrees/<x>` → `<root>`.
    for marker in ["/.claude/worktrees/", "/worktrees/"] {
        if let Some(idx) = cwd.find(marker) {
            return Some(cwd[..idx].to_string());
        }
    }
    // Sibling style `<parent>/<name>-worktrees/<x>` → `<parent>/<name>`.
    if let Some(idx) = cwd.find("-worktrees/") {
        return Some(cwd[..idx].to_string());
    }
    None
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests::strips identity::tests::leaves`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/identity.rs
git commit -m "feat(identity): worktree path-pattern fallback"
```

---

## Task 3: Identity sidecar (persist + versioned load)

**Files:**
- Modify: `src/config/identity.rs`

- [ ] **Step 1: Write the failing test**

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// One resolved identity for a cwd. `canonical_root` is the shortest repo
/// root path so it stays compatible with overrides keyed by root_path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedIdentity {
    pub remote_url: Option<String>, // normalized; None when no git remote
    pub canonical_root: String,
    pub source: IdentitySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum IdentitySource { LiveGit, CodexSeed, PathFallback }

/// In-memory sidecar: cwd -> identity, plus its own schema version on disk.
#[derive(Debug, Default)]
pub struct IdentityStore {
    map: HashMap<String, ResolvedIdentity>,
    dirty: bool,
}
```

```rust
    #[test]
    fn store_roundtrips_through_json() {
        let mut store = IdentityStore::default();
        store.insert(
            "/Users/xzy/workspace/crab/.claude/worktrees/x".into(),
            ResolvedIdentity {
                remote_url: Some("github.com/lucaxiang/crab".into()),
                canonical_root: "/Users/xzy/workspace/crab".into(),
                source: IdentitySource::CodexSeed,
            },
        );
        let json = store.to_json();
        let back = IdentityStore::from_json(&json).expect("valid");
        assert_eq!(
            back.get("/Users/xzy/workspace/crab/.claude/worktrees/x").unwrap().canonical_root,
            "/Users/xzy/workspace/crab"
        );
    }

    #[test]
    fn from_json_rejects_wrong_schema_version() {
        // A sidecar from a future/old schema must be ignored (treated as empty),
        // never silently trusted.
        let bad = r#"{"schema_version":999,"data":{}}"#;
        assert!(IdentityStore::from_json(bad).is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests::store`
Expected: compile error / fail — `to_json`/`from_json`/`insert`/`get` undefined.

- [ ] **Step 3: Implement the store**

```rust
const IDENTITY_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct VersionedStore { schema_version: u32, data: HashMap<String, ResolvedIdentity> }

impl IdentityStore {
    pub fn get(&self, cwd: &str) -> Option<&ResolvedIdentity> { self.map.get(cwd) }

    pub fn insert(&mut self, cwd: String, id: ResolvedIdentity) {
        if self.map.get(&cwd) != Some(&id) {
            self.map.insert(cwd, id);
            self.dirty = true;
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&VersionedStore {
            schema_version: IDENTITY_SCHEMA_VERSION,
            data: self.map.clone(),
        })
        .unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(raw: &str) -> Option<Self> {
        let v: VersionedStore = serde_json::from_str(raw).ok()?;
        if v.schema_version != IDENTITY_SCHEMA_VERSION {
            return None;
        }
        Some(Self { map: v.data, dirty: false })
    }

    fn path() -> std::path::PathBuf {
        dirs::home_dir().unwrap_or_default()
            .join(".config").join("ccmeter").join("identities.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|raw| Self::from_json(&raw))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if !self.dirty { return; }
        let path = Self::path();
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let tmp = path.with_extension("json.tmp");
        let json = self.to_json();
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::write(&path, &json);
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests`
Expected: PASS (all identity tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/identity.rs
git commit -m "feat(identity): versioned persisted identity sidecar"
```

---

## Task 4: Resolution order (live-first → persisted → path)

**Files:**
- Modify: `src/config/identity.rs`

This is the live-first resolver the spec [P2] mandates. It takes a closure for
live git resolution so tests stay hermetic (no real git / no real FS).

- [ ] **Step 1: Write the failing test**

```rust
    fn id(url: Option<&str>, root: &str, src: IdentitySource) -> ResolvedIdentity {
        ResolvedIdentity { remote_url: url.map(|s| s.to_string()), canonical_root: root.into(), source: src }
    }

    #[test]
    fn live_git_wins_when_cwd_exists_and_writes_through() {
        let mut store = IdentityStore::default();
        // Pretend a stale Codex seed exists for this cwd.
        store.insert("/repo".into(), id(Some("github.com/old/x"), "/old", IdentitySource::CodexSeed));
        let live = Some(id(Some("github.com/new/x"), "/repo", IdentitySource::LiveGit));
        let got = store.resolve("/repo", || live.clone());
        assert_eq!(got.canonical_root, "/repo");
        assert_eq!(got.source, IdentitySource::LiveGit);
        // Write-through replaced the stale seed.
        assert_eq!(store.get("/repo").unwrap().remote_url.as_deref(), Some("github.com/new/x"));
    }

    #[test]
    fn falls_back_to_persisted_then_path_when_live_none() {
        let mut store = IdentityStore::default();
        store.insert("/gone/wt".into(), id(Some("github.com/lucaxiang/crab"), "/Users/xzy/workspace/crab", IdentitySource::CodexSeed));
        // Persisted hit (deleted-worktree case): live returns None.
        let got = store.resolve("/gone/wt", || None);
        assert_eq!(got.canonical_root, "/Users/xzy/workspace/crab");
        // No persisted entry + no live → path fallback strips the worktree seg.
        let got2 = store.resolve("/Users/xzy/workspace/crab-worktrees/x", || None);
        assert_eq!(got2.canonical_root, "/Users/xzy/workspace/crab");
        assert_eq!(got2.source, IdentitySource::PathFallback);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests::live_git identity::tests::falls_back`
Expected: fail — `resolve` undefined.

- [ ] **Step 3: Implement `resolve`**

```rust
impl IdentityStore {
    /// Resolve a cwd's identity. `live` performs the actual git lookup and
    /// returns `Some` only when the cwd exists and git resolves. Order:
    /// live-git (write-through, invalidates stale) → persisted → path fallback.
    pub fn resolve(
        &mut self,
        cwd: &str,
        live: impl FnOnce() -> Option<ResolvedIdentity>,
    ) -> ResolvedIdentity {
        if let Some(found) = live() {
            self.insert(cwd.to_string(), found.clone());
            return found;
        }
        if let Some(found) = self.map.get(cwd) {
            return found.clone();
        }
        let canonical_root = strip_worktree_segment(cwd).unwrap_or_else(|| cwd.to_string());
        let id = ResolvedIdentity { remote_url: None, canonical_root, source: IdentitySource::PathFallback };
        self.insert(cwd.to_string(), id.clone());
        id
    }

    /// Seed an identity learned from Codex `repository_url` (used before live
    /// resolution so deleted-worktree cwds still resolve). Never overrides a
    /// live-git entry already present for this cwd.
    pub fn seed(&mut self, cwd: String, remote_url: String, canonical_root: String) {
        if matches!(self.map.get(&cwd).map(|i| i.source), Some(IdentitySource::LiveGit)) {
            return;
        }
        self.insert(cwd, ResolvedIdentity {
            remote_url: Some(remote_url),
            canonical_root,
            source: IdentitySource::CodexSeed,
        });
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter identity::tests`
Expected: PASS (all identity tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/identity.rs
git commit -m "feat(identity): live-first resolve with seed + path fallback"
```

---

## Task 5: Parse Codex session metadata (git + cwd)

**Files:**
- Create: `src/data/codex/sessions.rs`
- Modify: `src/data/codex/mod.rs` (add `pub mod sessions;`)

- [ ] **Step 1: Write the failing test**

In `src/data/codex/sessions.rs`:

```rust
//! Parse `session_meta` from Codex session files for project grouping:
//! cwd + git identity (repository_url, repo_root) + the session UUID.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct CodexSessionMeta {
    pub session_id: String,
    pub cwd: String,
    pub repository_url: Option<String>,
    pub repo_root: Option<String>,
}

/// Parse the FIRST `session_meta` line of a session file's contents.
pub fn parse_session_meta(raw: &str) -> Option<CodexSessionMeta> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cwd_and_git_repository_url() {
        let raw = r#"{"timestamp":"2026-05-02T16:02:20Z","type":"session_meta","payload":{"id":"019de96d-2d14-7f40-a333-ead58534fe57","cwd":"/Users/xzy/workspace/crab","git":{"branch":"main","repository_url":"git@github.com:LucaXiang/Crab.git"}}}"#;
        let m = parse_session_meta(raw).expect("meta");
        assert_eq!(m.session_id, "019de96d-2d14-7f40-a333-ead58534fe57");
        assert_eq!(m.cwd, "/Users/xzy/workspace/crab");
        assert_eq!(m.repository_url.as_deref(), Some("git@github.com:LucaXiang/Crab.git"));
    }

    #[test]
    fn handles_session_with_no_git() {
        let raw = r#"{"type":"session_meta","payload":{"id":"abc","cwd":"/Users/xzy/.claude-mem/observer-sessions"}}"#;
        let m = parse_session_meta(raw).expect("meta");
        assert_eq!(m.cwd, "/Users/xzy/.claude-mem/observer-sessions");
        assert_eq!(m.repository_url, None);
    }

    #[test]
    fn none_when_no_session_meta() {
        let raw = r#"{"type":"event_msg","payload":{"type":"token_count"}}"#;
        assert!(parse_session_meta(raw).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter codex::sessions::tests`
Expected: panic from `todo!()` (add `pub mod sessions;` to `codex/mod.rs` first so it compiles).

- [ ] **Step 3: Implement `parse_session_meta`**

```rust
#[derive(Deserialize)]
struct Line { #[serde(rename = "type")] kind: Option<String>, payload: Option<Payload> }
#[derive(Deserialize)]
struct Payload { id: Option<String>, cwd: Option<String>, git: Option<Git> }
#[derive(Deserialize)]
struct Git { repository_url: Option<String>, repo_root: Option<String> }

pub fn parse_session_meta(raw: &str) -> Option<CodexSessionMeta> {
    for line in raw.lines() {
        if !line.contains("session_meta") { continue; }
        let Ok(rec) = serde_json::from_str::<Line>(line) else { continue };
        if rec.kind.as_deref() != Some("session_meta") { continue; }
        let p = rec.payload?;
        let cwd = p.cwd?;
        return Some(CodexSessionMeta {
            session_id: p.id.unwrap_or_default(),
            cwd,
            repository_url: p.git.as_ref().and_then(|g| g.repository_url.clone()),
            repo_root: p.git.and_then(|g| g.repo_root),
        });
    }
    None
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter codex::sessions::tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/data/codex/sessions.rs src/data/codex/mod.rs
git commit -m "feat(codex): parse session_meta git identity + cwd"
```

---

## Task 6: Collect Codex session metadata across sessions

**Files:**
- Modify: `src/data/codex/sessions.rs`
- Modify: `src/data/codex/mod.rs` (expose `discover_session_files` to the crate)

`discover_session_files` is currently private; expose it `pub(crate)` so the
collector reuses the same dedup-by-filename discovery as the delta path.

- [ ] **Step 1: Write the failing test** (uses a tmp dir of session files)

```rust
    #[test]
    fn collects_meta_from_a_dir_of_sessions() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ccmeter-codexmeta-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut f = std::fs::File::create(dir.join("s1.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"session_meta","payload":{{"id":"id1","cwd":"/p/crab","git":{{"repository_url":"git@github.com:LucaXiang/Crab.git"}}}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"token_count"}}}}"#).unwrap();
        let metas = collect_session_meta_in(&dir);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].cwd, "/p/crab");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter codex::sessions::tests::collects`
Expected: fail — `collect_session_meta_in` undefined.

- [ ] **Step 3: Implement collectors**

In `codex/mod.rs`, change `fn discover_session_files` → `pub(crate) fn discover_session_files`. Then in `sessions.rs`:

```rust
use std::path::Path;

/// Parse `session_meta` from every `*.jsonl` directly in `dir` (test seam).
pub(crate) fn collect_session_meta_in(dir: &Path) -> Vec<CodexSessionMeta> {
    let mut files = Vec::new();
    super::collect_jsonl(dir, &mut files);
    files.iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|raw| parse_session_meta(&raw))
        .collect()
}

/// Parse `session_meta` from every discovered Codex session (sessions/ +
/// archived_sessions/), deduped by filename like the delta path.
pub fn collect_codex_session_meta() -> Vec<CodexSessionMeta> {
    super::discover_session_files()
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|raw| parse_session_meta(&raw))
        .collect()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo +1.95.0 test --bin ccmeter codex::sessions::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/data/codex/sessions.rs src/data/codex/mod.rs
git commit -m "feat(codex): collect session_meta across all sessions"
```

---

## Task 7: Add `provider` to `ProjectSource`

**Files:**
- Modify: `src/config/discovery.rs`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn project_source_defaults_to_claude_provider() {
        let s = ProjectSource {
            dir_name: "d".into(), path: PathBuf::from("/p"),
            session_files: vec![], cwd: Some("/p".into()),
            source_root: PathBuf::from("/r"), provider: Provider::Claude,
        };
        assert_eq!(s.provider, Provider::Claude);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter discovery::tests::project_source_defaults`
Expected: compile error — `Provider` / field `provider` missing.

- [ ] **Step 3: Implement**

Add the enum and field; set `provider: Provider::Claude` at the existing
`ProjectSource { … }` construction in `discover_sources` (~`src/config/discovery.rs:148`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider { Claude, Codex }
```
Add `pub provider: Provider,` to the `ProjectSource` struct (after `source_root`).

- [ ] **Step 4: Run test to verify it passes (and the crate still builds)**

Run: `cargo +1.95.0 build --bin ccmeter && cargo +1.95.0 test --bin ccmeter discovery::tests`
Expected: builds; PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/discovery.rs
git commit -m "feat(discovery): tag ProjectSource with provider"
```

---

## Task 8: Provider-aware unified discovery (folds Codex into groups)

**Files:**
- Modify: `src/config/discovery.rs`

This is the load-bearing task. It adds `discover_project_groups_unified()`:
runs Claude `discover_sources()`, then turns each `CodexSessionMeta` into a
Codex `ProjectSource`, seeds the identity store from Codex `repository_url`s,
and groups everything together. Grouping reuses `group_by_identity` but is
fed the identity store so Codex remotes merge with Claude. To keep
`group_by_identity` testable, factor its identity lookup behind the store.

- [ ] **Step 1: Write the failing test** (hermetic — builds sources directly)

```rust
    fn src(cwd: &str, provider: Provider) -> ProjectSource {
        ProjectSource {
            dir_name: cwd.into(), path: PathBuf::from(cwd),
            session_files: vec![PathBuf::from(format!("{cwd}/s.jsonl"))],
            cwd: Some(cwd.into()), source_root: PathBuf::from("/r"), provider,
        }
    }

    #[test]
    fn codex_worktree_and_claude_main_collapse_to_one_group() {
        use crate::config::identity::{IdentityStore, ResolvedIdentity, IdentitySource};
        let mut store = IdentityStore::default();
        let crab = ResolvedIdentity {
            remote_url: Some("github.com/lucaxiang/crab".into()),
            canonical_root: "/Users/xzy/workspace/crab".into(),
            source: IdentitySource::CodexSeed,
        };
        // Claude main + a Codex worktree both resolve to the crab remote.
        store.insert("/Users/xzy/workspace/crab".into(), crab.clone());
        store.insert("/Users/xzy/workspace/crab-worktrees/x".into(), crab);
        let sources = vec![
            src("/Users/xzy/workspace/crab", Provider::Claude),
            src("/Users/xzy/workspace/crab-worktrees/x", Provider::Codex),
        ];
        let groups = group_with_store(sources, &mut store);
        assert_eq!(groups.len(), 1, "both providers in one crab group");
        let g = &groups[0];
        let cwds: Vec<&str> = g.sources.iter().filter_map(|s| s.cwd.as_deref()).collect();
        assert!(cwds.contains(&"/Users/xzy/workspace/crab"));
        assert!(cwds.contains(&"/Users/xzy/workspace/crab-worktrees/x"));
    }

    #[test]
    fn codex_only_repo_gets_its_own_group() {
        use crate::config::identity::{IdentityStore, ResolvedIdentity, IdentitySource};
        let mut store = IdentityStore::default();
        store.insert("/o/obs".into(), ResolvedIdentity {
            remote_url: None, canonical_root: "/o/obs".into(), source: IdentitySource::PathFallback });
        let groups = group_with_store(vec![src("/o/obs", Provider::Codex)], &mut store);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].sources[0].provider, Provider::Codex);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter discovery::tests::codex_worktree discovery::tests::codex_only`
Expected: fail — `group_with_store` undefined.

- [ ] **Step 3: Implement `group_with_store` + `discover_project_groups_unified`**

`group_with_store` is `group_by_identity` with the resolution step delegated to
the store (so Codex cwds resolve from seeds and Claude cwds resolve live). Keep
the existing `group_by_identity` for the Claude-only callers/tests; implement the
new fn as:

```rust
/// Group a mixed Claude+Codex source list using an already-populated identity
/// store. Same canonical key (normalized remote URL, else canonical_root path)
/// → one group. Mirrors group_by_identity phases 2-5 but reads the store
/// instead of calling resolve_identity per source.
pub(crate) fn group_with_store(
    sources: Vec<ProjectSource>,
    store: &mut IdentityStore,
) -> Vec<ProjectGroup> {
    use crate::config::identity::normalize_remote_url;
    // Resolve each source: live git only when the cwd exists on disk.
    let resolved: Vec<(ProjectSource, ResolvedIdentity)> = sources
        .into_iter()
        .map(|s| {
            let cwd = s.cwd.clone().unwrap_or_else(|| s.path.to_string_lossy().into_owned());
            let id = store.resolve(&cwd, || resolve_identity_live(&cwd));
            (s, id)
        })
        .collect();

    // Canonical key: normalized remote URL when present, else canonical_root.
    let key_of = |id: &ResolvedIdentity| -> String {
        id.remote_url.clone().map(|u| normalize_remote_url(&u)).unwrap_or_else(|| id.canonical_root.clone())
    };
    // Group key → (canonical_root chosen as shortest, remote_url, sources).
    let mut groups_map: HashMap<String, (PathBuf, Option<String>, Vec<ProjectSource>)> = HashMap::new();
    for (s, id) in resolved {
        let k = key_of(&id);
        let root = PathBuf::from(&id.canonical_root);
        let entry = groups_map.entry(k).or_insert_with(|| (root.clone(), id.remote_url.clone(), Vec::new()));
        if root.as_os_str().len() < entry.0.as_os_str().len() { entry.0 = root; }
        entry.2.push(s);
    }
    // Phase 4 (merge same-cwd) + Phase 5 (build ProjectGroup) reused from group_by_identity.
    finalize_groups(groups_map)
}

/// Live-only identity: `None` unless the cwd exists and git resolves.
fn resolve_identity_live(cwd: &str) -> Option<ResolvedIdentity> {
    use crate::config::identity::{normalize_remote_url, IdentitySource, ResolvedIdentity};
    let p = Path::new(cwd);
    if !p.is_dir() { return None; }
    let root = find_git_root(p)?;
    let remote = get_remote_url(&root).map(|u| normalize_remote_url(&u));
    Some(ResolvedIdentity {
        remote_url: remote,
        canonical_root: root.to_string_lossy().into_owned(),
        source: IdentitySource::LiveGit,
    })
}
```

Factor the existing Phase 4/5 body of `group_by_identity` into a shared
`finalize_groups(groups_map: HashMap<PathBuf,(Option<String>,Vec<ProjectSource>)>)`
helper and call it from both. (Adjust the `group_with_store` map shape to match,
or write a thin adapter — keep one canonical finalizer.)

Then the public entry point:

```rust
/// Provider-aware discovery used by both initial load and refresh. Collects
/// Claude sources + Codex session metadata, seeds + resolves identities, and
/// returns one unified group set. Persists the identity store.
pub fn discover_project_groups_unified() -> (Vec<ProjectGroup>, RootMap, SessionMap) {
    use crate::config::identity::{normalize_remote_url, IdentityStore};
    use crate::data::codex::sessions::collect_codex_session_meta;

    let claude_sources = discover_sources();
    let (root_map, session_map) = build_root_and_session_maps(&claude_sources); // existing loop, extracted

    let mut store = IdentityStore::load();

    // Seed identities from Codex repository_url BEFORE resolving, then build
    // Codex ProjectSources (one per distinct cwd; session_files left empty —
    // Codex cache/index entries are produced separately under CODEX_ROOT).
    let metas = collect_codex_session_meta();
    let mut codex_sources: Vec<ProjectSource> = Vec::new();
    let mut seen_cwd = std::collections::HashSet::new();
    for m in &metas {
        if let Some(url) = &m.repository_url {
            let canonical = m.repo_root.clone().unwrap_or_else(|| m.cwd.clone());
            store.seed(m.cwd.clone(), url.clone(), canonical);
        }
        if seen_cwd.insert(m.cwd.clone()) {
            codex_sources.push(ProjectSource {
                dir_name: m.cwd.clone(), path: PathBuf::from(&m.cwd),
                session_files: vec![], cwd: Some(m.cwd.clone()),
                source_root: PathBuf::from(crate::data::codex::CODEX_ROOT),
                provider: Provider::Codex,
            });
        }
    }

    let mut all = claude_sources;
    all.extend(codex_sources);
    let groups = group_with_store(all, &mut store);
    store.save();
    (groups, root_map, session_map)
}
```

Extract the existing root_map/session_map building loop (`discover_project_groups_with_root_map`
lines ~90-105) into `build_root_and_session_maps(&[ProjectSource]) -> (RootMap, SessionMap)`
and call it from both the old and new entry points.

- [ ] **Step 4: Run tests + build**

Run: `cargo +1.95.0 build --bin ccmeter && cargo +1.95.0 test --bin ccmeter discovery::`
Expected: builds; all discovery tests PASS (old `test_discover_runs` still green).

- [ ] **Step 5: Commit**

```bash
git add src/config/discovery.rs
git commit -m "feat(discovery): unified provider-aware grouping with identity store"
```

---

## Task 9: Remove the `CODEX_ROOT` special-case in `build_model_stats`

**Files:**
- Modify: `src/data/index.rs:345-349`

With Codex cwds now mapped to repo groups via `cwd_to_root`, the special-case
that forced Codex to a separate `rk` must go, so Codex per-model usage attributes
to its repo group. Isolation for the source tabs is unchanged (`entry_passes`
filters by entry root first).

- [ ] **Step 1: Update the existing index test to assert the new behavior**

Modify `build_model_stats_splits_codex_by_specific_model_and_keeps_claude_family`
(`src/data/index.rs:751`) so the Codex entry's cwd is mapped via `cwd_to_root`
to a repo root (e.g. insert `("/p".into(), "crab".into())` into the test's
`cwd_to_root`) and assert the Codex model now appears under the `"crab"`
root_key (not `CODEX_ROOT`). Add an assertion that with `RootFilter::Exclude(CODEX_ROOT)`
the Codex model is absent (Claude-tab isolation holds).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter index::tests::build_model_stats_splits_codex`
Expected: FAIL — Codex still keyed under `CODEX_ROOT` (old special-case).

- [ ] **Step 3: Remove the special-case**

Replace (`src/data/index.rs:345-349`):

```rust
            let rk_lookup = if entry_root == CODEX_ROOT {
                None
            } else {
                cwd_to_rk.get(&e.cwd_idx).copied()
            };
```
with:
```rust
            let rk_lookup = cwd_to_rk.get(&e.cwd_idx).copied();
```
(The `entry_root` binding may now be unused except for the fallback arm below — keep it for the `rk_intern.get(entry_root)` fallback path; silence any unused warning only if it actually fires.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo +1.95.0 test --bin ccmeter index::`
Expected: PASS (updated test + existing fold_codex test).

- [ ] **Step 5: Commit**

```bash
git add src/data/index.rs
git commit -m "refactor(index): map Codex per-model usage to its repo group"
```

---

## Task 10: Wire unified discovery into `App::new` and refresh

**Files:**
- Modify: `src/app.rs` (`App::new` ~`:192`, `spawn_discovery` ~`:1046`)

- [ ] **Step 1: Switch both call sites to the unified entry point**

In `App::new` (`src/app.rs:192-193`):
```rust
        let (raw_groups, root_cwd_map, session_map) =
            discovery::discover_project_groups_unified();
```
In `spawn_discovery` (`src/app.rs:1049-1050`):
```rust
        let (raw_groups, root_cwd_map, session_map) =
            discovery::discover_project_groups_unified();
```

- [ ] **Step 2: Build**

Run: `cargo +1.95.0 build --bin ccmeter`
Expected: builds clean.

- [ ] **Step 3: Manual smoke (real data)**

Run: `cargo +1.95.0 run --bin ccmeter` then `⇧Tab` to the `Codex` source.
Expected: project cards (incl. `crab`) appear; `All` shows `crab` combining
Claude+Codex; a Codex-only dir (e.g. `observer-sessions`) appears as its own card.

- [ ] **Step 4: Run the whole suite**

Run: `cargo +1.95.0 test`
Expected: only the 2 known date-relative `rate_limits` failures (pre-existing).

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): use unified provider-aware discovery (load + refresh)"
```

---

## Task 11: Retire the dedicated `CodexBreakdown` panel

**Files:**
- Modify: `src/app.rs` (remove `CodexBreakdown` struct ~`:113`, `codex_breakdown` field ~`:131`, `build_codex_breakdown` ~`:937`, its use in `build_render_cache` ~`:918`)
- Modify: `src/ui/dashboard.rs:424-444` (remove the codex-breakdown branches)
- Modify: `src/ui/cards/render.rs:1301` (remove `render_codex_breakdown`)

Codex per-model usage now renders through the per-card model breakdown
(`model_daily_costs`/`model_shares`), so the standalone panel is dead. In the
Codex source view, Codex now has real cards, so the `cards.is_empty()` branch in
`dashboard.rs` no longer needs a codex fallback.

- [ ] **Step 1: Remove the panel data + builder**

Delete `CodexBreakdown` (struct), the `codex_breakdown` field from `RenderCache`,
`build_codex_breakdown`, and the `let codex_breakdown = …` block in
`build_render_cache`; drop `codex_breakdown` from the `RenderCache { … }` literal.

- [ ] **Step 2: Remove the renderer + call sites**

Delete `render_codex_breakdown` (`render.rs`) and replace the
`dashboard.rs:421-444` block so the no-cards branch is a plain empty state and
the combined branch just renders cards:

```rust
        } else {
            cards::render(frame, chunks[2], &self.render.cards, anim_tick, range_start, range_end, self.card_scroll);
        }
```

- [ ] **Step 3: Build (catch every reference)**

Run: `cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning'`
Expected: empty (no dangling references, no unused-import warnings).

- [ ] **Step 4: Run suite + clippy**

Run: `cargo +1.95.0 test 2>&1 | rg 'test result'` then
`cargo +1.95.0 clippy --bin ccmeter 2>&1 | rg 'generated'`
Expected: same 2 known failures only; clippy warning count not above the 11 baseline.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/ui/dashboard.rs src/ui/cards/render.rs
git commit -m "refactor(ui): retire dedicated Codex panel; per-card model breakdown carries Codex"
```

---

## Phase 1 acceptance

- [ ] `Codex` source tab shows project cards grouped by repo; all `crab`
      worktrees collapse into one `crab` card.
- [ ] `All` view: `crab` card cost/tokens = Claude + Codex; `Claude Code`
      (`Exclude(codex)`) and `Codex` (`Only(codex)`) tabs each isolate.
- [ ] A Codex-only dir gets its own card; a deleted-worktree cwd still collapses
      via the identity sidecar (`~/.config/ccmeter/identities.json` written).
- [ ] `cargo +1.95.0 build --bin ccmeter` clean; `cargo +1.95.0 test` shows only
      the 2 known date-relative failures; clippy not above baseline.

Phases 2 (card-face provider split) and 3 (recent sessions with titles) get
their own plans once Phase 1's concrete `ProjectCard` / grouping shapes land.
