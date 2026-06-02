//! Canonical git identity resolution shared by Claude + Codex discovery:
//! normalize remote URLs, collapse worktree paths, and persist resolved
//! identities so deleted worktrees still group correctly.

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

/// Canonicalize a git remote URL so `git@host:org/repo.git` and
/// `https://host/org/repo[.git]` collapse to one key: `host/org/repo`,
/// lowercased, no scheme/credentials, no trailing `.git`.
pub fn normalize_remote_url(url: &str) -> String {
    let s = url.trim();
    // Strip scheme + optional credentials: `scheme://user@` → ``.
    let s = s.split("://").last().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s); // drop `user@` / creds
    // SSH shorthand uses `host:org/repo`; HTTP uses `host/org/repo`.
    let s = s.replacen(':', "/", 1);
    let s = s.strip_suffix(".git").unwrap_or(&s).to_string();
    s.trim_end_matches('/').to_lowercase()
}

/// Best-effort canonical repo path for a cwd with no resolvable git remote:
/// strip the first worktree segment. Returns `None` when no known worktree
/// pattern is present (caller then uses the cwd as-is).
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

    #[test]
    fn distinct_repos_stay_distinct() {
        assert_ne!(
            normalize_remote_url("git@github.com:LucaXiang/Crab.git"),
            normalize_remote_url("git@github.com:LucaXiang/crab-red-coral.git"),
        );
    }

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
}
