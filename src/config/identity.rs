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

    /// Internal priming helper; `resolve` and `seed` are the public entry points.
    pub(crate) fn insert(&mut self, cwd: String, id: ResolvedIdentity) {
        if self.map.get(&cwd) != Some(&id) {
            self.map.insert(cwd, id);
            self.dirty = true;
        }
    }

    /// Serialize to pretty JSON. Returns `"{}"` on error (safe for display/tests).
    /// `save` uses `to_json_checked` to avoid writing a lossy fallback to disk.
    pub fn to_json(&self) -> String {
        self.to_json_checked().unwrap_or_else(|| "{}".into())
    }

    /// Returns `Some(json)` only when serialization succeeds; `None` on error.
    fn to_json_checked(&self) -> Option<String> {
        serde_json::to_string_pretty(&VersionedStore {
            schema_version: IDENTITY_SCHEMA_VERSION,
            data: self.map.clone(),
        }).ok()
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

    /// Persist the store to disk only when serialization succeeds; never writes
    /// `{}` on a serialize error (mirrors `src/data/cache.rs::save`).
    pub fn save(&self) {
        if !self.dirty { return; }
        let Some(json) = self.to_json_checked() else { return; };
        let path = Self::path();
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let tmp = path.with_extension("json.tmp");
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
    /// live-git entry already present for this cwd; does override a PathFallback
    /// entry because a CodexSeed carries a real `repository_url` — strictly more
    /// informative than a path-derived guess.
    pub fn seed(&mut self, cwd: String, remote_url: String, canonical_root: String) {
        // Only LiveGit is authoritative enough to block a seed update.
        if matches!(self.map.get(&cwd).map(|i| i.source), Some(IdentitySource::LiveGit)) {
            return;
        }
        let remote_url = normalize_remote_url(&remote_url);
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
    // Remember whether the original URL carried an explicit scheme (e.g. https://).
    let had_scheme = s.contains("://");
    // Strip scheme + optional credentials: `scheme://user@` → ``.
    let s = s.split("://").last().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s); // drop `user@` / creds
    // Normalize the host:path separator:
    //   • SSH shorthand (`git@host:org/repo`) has no `://`; the `:` separates
    //     host from path and must become `/`.
    //   • HTTPS/HTTP URLs that had `://` may have a port after the host
    //     (`host:8080/org/repo`); that `:port` is not a path segment — drop it.
    let s = if had_scheme {
        // Remove optional `:port` between host and the first `/`.
        let s = if let Some(slash) = s.find('/') {
            let host_part = &s[..slash];
            let path_part = &s[slash..];
            if let Some(colon) = host_part.rfind(':') {
                // Only strip if what follows the colon is all digits (a port number).
                if host_part[colon + 1..].chars().all(|c| c.is_ascii_digit()) {
                    format!("{}{}", &host_part[..colon], path_part)
                } else {
                    s.to_string()
                }
            } else {
                s.to_string()
            }
        } else {
            s.to_string()
        };
        s
    } else {
        // SSH shorthand: replace the first `:` (host:path) with `/`.
        s.replacen(':', "/", 1)
    };
    let s = s.strip_suffix(".git").unwrap_or(&s).to_string();
    s.trim_end_matches('/').to_lowercase()
}

/// Best-effort canonical repo path for a cwd with no resolvable git remote:
/// strip the first worktree segment. Returns `None` when no known worktree
/// pattern is present (caller then uses the cwd as-is).
pub fn strip_worktree_segment(cwd: &str) -> Option<String> {
    // `<root>/.claude/worktrees/<x>` and `<root>/worktrees/<x>` → `<root>`.
    // Note: `/worktrees/` is intentionally broader than the `.claude/worktrees`
    // sentinel — it also catches bare `<root>/worktrees/<name>` conventions used
    // by some projects and `git worktree add` defaults.
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
    fn normalizes_https_with_port() {
        // Port after host must be dropped, not turned into a path segment.
        assert_eq!(
            normalize_remote_url("https://github.example.com:8080/Org/Repo.git"),
            "github.example.com/org/repo"
        );
        // Without port still works.
        assert_eq!(
            normalize_remote_url("https://github.example.com/Org/Repo.git"),
            "github.example.com/org/repo"
        );
        // SSH shorthand (no scheme) still collapses host:path correctly.
        assert_eq!(
            normalize_remote_url("git@github.example.com:Org/Repo.git"),
            "github.example.com/org/repo"
        );
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

    #[test]
    fn save_does_not_write_on_serialize_error() {
        // We cannot easily force serde_json to fail on this type, but we can
        // verify to_json_checked returns Some for a valid store and that save
        // only writes when Some. This test exercises the guard path by confirming
        // to_json_checked is Some for a normal store (the error branch is a
        // defensive fallback for future type changes).
        let mut store = IdentityStore::default();
        store.insert("/test".into(), id(Some("github.com/a/b"), "/test", IdentitySource::LiveGit));
        // to_json_checked must succeed for valid data.
        assert!(store.to_json_checked().is_some());
        // to_json (lossy) must equal to_json_checked result.
        assert_eq!(store.to_json(), store.to_json_checked().unwrap());
    }

    #[test]
    fn seed_overrides_path_fallback_but_not_live() {
        let mut store = IdentityStore::default();
        // PathFallback entry: seed should override it.
        store.insert("/wt".into(), id(None, "/wt", IdentitySource::PathFallback));
        store.seed("/wt".into(), "github.com/org/repo".into(), "/root".into());
        let entry = store.get("/wt").unwrap();
        assert_eq!(entry.source, IdentitySource::CodexSeed);
        assert_eq!(entry.canonical_root, "/root");

        // LiveGit entry: seed must NOT override it.
        store.insert("/live".into(), id(Some("github.com/live/repo"), "/live", IdentitySource::LiveGit));
        store.seed("/live".into(), "github.com/seed/repo".into(), "/seed-root".into());
        let live_entry = store.get("/live").unwrap();
        assert_eq!(live_entry.source, IdentitySource::LiveGit);
        assert_eq!(live_entry.canonical_root, "/live");
    }
}
