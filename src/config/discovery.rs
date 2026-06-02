use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which provider produced this project source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
}

/// A discovered Claude Code project directory with its JSONL session files.
#[derive(Debug, Clone)]
pub struct ProjectSource {
    /// The encoded directory name (e.g. `-Users-hmenzagh-code-CCMeter`).
    pub dir_name: String,
    /// Absolute path to the project directory inside Claude's data.
    pub path: PathBuf,
    /// JSONL session files found (direct or nested in subagent dirs).
    pub session_files: Vec<PathBuf>,
    /// The actual working directory extracted from JSONL metadata.
    pub cwd: Option<String>,
    /// Which Claude root this came from (e.g. `~/.claude/projects`).
    pub source_root: PathBuf,
    /// Which provider produced this source (Claude or Codex).
    // read by the card provider-split (Phase 2); set during discovery now
    #[allow(dead_code)]
    pub provider: Provider,
}

impl ProjectSource {
    /// The effective root path: cwd if available, otherwise the data directory path.
    pub fn effective_root(&self) -> PathBuf {
        self.cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.path.clone())
    }
}

/// A group of project sources that belong to the same git repository.
#[derive(Debug, Clone)]
pub struct ProjectGroup {
    /// Display name for this project group.
    pub name: String,
    /// The resolved root path (git root or heuristic).
    pub root_path: PathBuf,
    /// Git remote URL if available.
    pub remote_url: Option<String>,
    /// All project sources belonging to this group.
    pub sources: Vec<ProjectSource>,
    /// Total number of JSONL session files across all sources.
    pub total_sessions: usize,
    /// Set when this group was created by an override (split or merge).
    pub override_info: Option<OverrideInfo>,
}

impl ProjectGroup {
    /// String key derived from root_path, used for overrides and cache lookups.
    pub fn root_key(&self) -> String {
        self.root_path.to_string_lossy().into_owned()
    }
}

/// How a group was created by an override.
#[derive(Debug, Clone)]
pub enum OverrideInfo {
    /// This group was split out from a larger auto-detected group.
    Split { original_root: PathBuf },
    /// This group was created by manually merging two or more groups.
    Merged,
}

/// Mapping from each source root to the set of cwds that originate from it.
pub type RootMap = HashMap<PathBuf, HashSet<String>>;

/// Mapping from each session file basename to `(root, cwd)`.
pub type SessionMap = HashMap<String, (String, String)>;

/// Build the `(RootMap, SessionMap)` for a set of sources *before* grouping, so
/// they are not affected by same-cwd merges across roots. Shared by both the
/// Claude-only and the unified discovery entry points.
fn build_root_and_session_maps(sources: &[ProjectSource]) -> (RootMap, SessionMap) {
    let mut root_map: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut session_map: HashMap<String, (String, String)> = HashMap::new();

    for s in sources {
        let cwd = match &s.cwd {
            Some(c) => c.clone(),
            None => s.path.to_string_lossy().to_string(),
        };
        let root_str = s.source_root.to_string_lossy().to_string();
        root_map
            .entry(s.source_root.clone())
            .or_default()
            .insert(cwd.clone());
        for f in &s.session_files {
            if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                session_map.insert(name.to_string(), (root_str.clone(), cwd.clone()));
            }
        }
    }

    (root_map, session_map)
}

/// Provider-aware discovery used by both initial load and refresh. Collects
/// Claude sources + Codex session metadata, seeds + resolves identities through
/// the persisted identity store, and returns one unified group set (Codex cwds
/// fold into their repo's group). Persists the identity store.
///
/// The returned `RootMap`/`SessionMap` are Claude-derived only: Codex cache and
/// index entries are produced separately under `CODEX_ROOT`, so Codex sources
/// carry no `session_files` here and contribute nothing to the session map.
pub fn discover_project_groups_unified() -> (Vec<ProjectGroup>, RootMap, SessionMap) {
    use crate::config::identity::IdentityStore;
    use crate::data::codex::sessions::collect_codex_session_meta;

    let claude_sources = discover_sources();
    let (root_map, session_map) = build_root_and_session_maps(&claude_sources);

    let mut store = IdentityStore::load();

    // Seed identities from Codex repository_url BEFORE resolving, then build
    // Codex ProjectSources (one per distinct cwd; session_files left empty —
    // Codex cache/index entries are produced separately under CODEX_ROOT).
    let metas = collect_codex_session_meta();
    let mut codex_sources: Vec<ProjectSource> = Vec::new();
    let mut seen_cwd: HashSet<String> = HashSet::new();
    for m in &metas {
        if let Some(url) = &m.repository_url {
            let canonical = m.repo_root.clone().unwrap_or_else(|| m.cwd.clone());
            store.seed(m.cwd.clone(), url.clone(), canonical);
        }
        if seen_cwd.insert(m.cwd.clone()) {
            codex_sources.push(ProjectSource {
                dir_name: m.cwd.clone(),
                path: PathBuf::from(&m.cwd),
                session_files: vec![],
                cwd: Some(m.cwd.clone()),
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

// ---------------------------------------------------------------------------
// Source discovery
// ---------------------------------------------------------------------------

fn discover_sources() -> Vec<ProjectSource> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let roots = find_project_roots(&home);
    let mut sources = Vec::new();

    for root in &roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        paths.sort();

        for path in paths {
            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let session_files = collect_jsonl_files_recursive(&path);
            if session_files.is_empty() {
                continue;
            }

            let cwd = extract_cwd_from_first_session(&session_files);

            sources.push(ProjectSource {
                dir_name,
                path,
                session_files,
                cwd,
                source_root: root.clone(),
                provider: Provider::Claude,
            });
        }
    }

    sources.sort_by(|a, b| {
        a.source_root
            .cmp(&b.source_root)
            .then_with(|| a.cwd.cmp(&b.cwd))
            .then_with(|| a.path.cmp(&b.path))
    });
    sources
}

fn find_project_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    let config_claude = home.join(".config").join("claude").join("projects");
    if config_claude.is_dir() {
        roots.push(config_claude);
    }

    let dot_claude = home.join(".claude").join("projects");
    if dot_claude.is_dir() {
        roots.push(dot_claude);
    }

    if let Ok(entries) = std::fs::read_dir(home) {
        let mut home_entries: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        home_entries.sort();
        for entry_path in home_entries {
            let Some(name) = entry_path.file_name() else {
                continue;
            };
            let name_str = name.to_string_lossy();

            if !name_str.starts_with('.') || !name_str.contains("claude") {
                continue;
            }
            if name_str == ".claude" || name_str == ".config" {
                continue;
            }

            let projects_dir = entry_path.join("projects");
            if projects_dir.is_dir() && !roots.contains(&projects_dir) {
                roots.push(projects_dir);
            }
        }
    }

    roots.sort();
    roots
}

// ---------------------------------------------------------------------------
// JSONL collection
// ---------------------------------------------------------------------------

fn collect_jsonl_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_into(dir, &mut files);
    files.sort();
    files
}

fn collect_jsonl_into(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            collect_jsonl_into(&path, files);
        }
    }
}

// ---------------------------------------------------------------------------
// CWD extraction from JSONL
// ---------------------------------------------------------------------------

fn extract_cwd_from_first_session(session_files: &[PathBuf]) -> Option<String> {
    for path in session_files {
        if let Some(cwd) = extract_cwd_from_jsonl(path) {
            return Some(cwd);
        }
    }
    None
}

fn extract_cwd_from_jsonl(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    for (i, line) in reader.lines().enumerate() {
        if i > 30 {
            break;
        }
        let line = line.ok()?;
        if !line.contains("\"cwd\"") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
            && value.get("type").and_then(|t| t.as_str()) == Some("user")
            && let Some(cwd) = value.get("cwd").and_then(|c| c.as_str())
        {
            return Some(cwd.to_string());
        }
    }
    None
}

/// Find the main git repository root for a path.
/// For linked worktrees, resolves back to the main repo root.
fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--show-toplevel"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let toplevel = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());

    // If .git is a file (not a directory), this is a linked worktree.
    let git_path = toplevel.join(".git");
    if git_path.is_file()
        && let Some(main_root) = resolve_worktree_main_root(&git_path)
    {
        return Some(main_root);
    }

    Some(toplevel)
}

/// Follow a worktree `.git` file → main repo root.
/// File content: `gitdir: /path/to/main-repo/.git/worktrees/<name>`
fn resolve_worktree_main_root(git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    let gitdir = content.strip_prefix("gitdir: ")?.trim();
    let gitdir_path = PathBuf::from(gitdir);

    // <main-repo>/.git/worktrees/<name> → go up 2 to .git, up 1 to main-repo
    let main_root = gitdir_path.parent()?.parent()?.parent()?;

    if main_root.is_dir() {
        Some(main_root.to_path_buf())
    } else {
        None
    }
}

/// Get the `origin` remote URL for a git repo.
fn get_remote_url(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "remote", "get-url", "origin"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

/// Shared finalizer for both `group_with_store` calls.
///
/// Takes a map keyed by the chosen canonical root path, each value being
/// `(remote_url, sources)`. Performs:
/// - Phase 4: merge sources that share the same cwd (within a group).
/// - Phase 5: build the `ProjectGroup` vec, sorted by lowercased name.
fn finalize_groups(
    mut groups_map: HashMap<PathBuf, (Option<String>, Vec<ProjectSource>)>,
) -> Vec<ProjectGroup> {
    // Phase 4: merge sources that share the same cwd
    for (_url, sources) in groups_map.values_mut() {
        let mut merged: Vec<ProjectSource> = Vec::new();
        for source in sources.drain(..) {
            if let Some(existing) = source
                .cwd
                .as_ref()
                .and_then(|cwd| merged.iter_mut().find(|m| m.cwd.as_ref() == Some(cwd)))
            {
                existing.session_files.extend(source.session_files);
            } else {
                merged.push(source);
            }
        }
        // Re-sort session files after merge
        for s in &mut merged {
            s.session_files.sort();
        }
        *sources = merged;
    }

    // Phase 5: build ProjectGroup vec
    let mut groups: Vec<ProjectGroup> = groups_map
        .into_iter()
        .map(|(root_path, (remote_url, sources))| {
            let total_sessions = sources.iter().map(|s| s.session_files.len()).sum();
            let name = derive_group_name(&root_path);

            ProjectGroup {
                name,
                root_path,
                remote_url,
                sources,
                total_sessions,
                override_info: None,
            }
        })
        .collect();

    groups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    groups
}

// ---------------------------------------------------------------------------
// Store-aware grouping (provider-unified: Claude + Codex)
// ---------------------------------------------------------------------------

/// Live-only identity: returns `None` unless the cwd exists on disk and git
/// resolves. Uses the existing `find_git_root` (worktree → main repo) and
/// `get_remote_url`, normalizing the remote so it shares a key with seeds.
fn resolve_identity_live(cwd: &str) -> Option<crate::config::identity::ResolvedIdentity> {
    use crate::config::identity::{normalize_remote_url, IdentitySource, ResolvedIdentity};
    let p = Path::new(cwd);
    if !p.is_dir() {
        return None;
    }
    let root = find_git_root(p)?;
    let remote = get_remote_url(&root).map(|u| normalize_remote_url(&u));
    Some(ResolvedIdentity {
        remote_url: remote,
        canonical_root: root.to_string_lossy().into_owned(),
        source: IdentitySource::LiveGit,
    })
}

/// Group a mixed Claude+Codex source list using an identity store. Each source's
/// cwd is resolved live-git-first (write-through), falling back to the store's
/// seeded/persisted entry, then a path-pattern guess. Sources sharing a
/// canonical key (normalized remote URL when present, else `canonical_root`)
/// collapse into one group whose root is the SHORTEST canonical root among them
/// (so existing overrides keyed by the Claude repo root still match).
pub(crate) fn group_with_store(
    sources: Vec<ProjectSource>,
    store: &mut crate::config::identity::IdentityStore,
) -> Vec<ProjectGroup> {
    use crate::config::identity::{normalize_remote_url, ResolvedIdentity};

    // Resolve each source: live git only when the cwd exists on disk.
    let resolved: Vec<(ProjectSource, ResolvedIdentity)> = sources
        .into_iter()
        .map(|s| {
            let cwd = s
                .cwd
                .clone()
                .unwrap_or_else(|| s.path.to_string_lossy().into_owned());
            let id = store.resolve(&cwd, || resolve_identity_live(&cwd));
            (s, id)
        })
        .collect();

    // Canonical key: normalized remote URL when present, else canonical_root.
    let key_of = |id: &ResolvedIdentity| -> String {
        id.remote_url
            .clone()
            .map(|u| normalize_remote_url(&u))
            .unwrap_or_else(|| id.canonical_root.clone())
    };

    // Group key → (canonical_root chosen as shortest, remote_url, sources).
    let mut keyed: HashMap<String, (PathBuf, Option<String>, Vec<ProjectSource>)> = HashMap::new();
    for (s, id) in resolved {
        let k = key_of(&id);
        let root = PathBuf::from(&id.canonical_root);
        let entry = keyed
            .entry(k)
            .or_insert_with(|| (root.clone(), id.remote_url.clone(), Vec::new()));
        if root.as_os_str().len() < entry.0.as_os_str().len() {
            entry.0 = root;
        }
        entry.2.push(s);
    }

    // Adapt to the shared finalizer's map shape (keyed by canonical root path).
    let groups_map: HashMap<PathBuf, (Option<String>, Vec<ProjectSource>)> = keyed
        .into_values()
        .map(|(root, remote_url, sources)| (root, (remote_url, sources)))
        .collect();

    finalize_groups(groups_map)
}

/// Derive a human-readable name from a root path.
pub fn derive_group_name(root: &Path) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let path_str = root.to_string_lossy();

    let relative = if let Some(stripped) = path_str.strip_prefix(home.to_string_lossy().as_ref()) {
        stripped.trim_start_matches('/')
    } else {
        &path_str
    };

    let parts: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();

    if parts.is_empty() {
        return root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.to_string());
    }

    if parts.len() <= 2 {
        parts.join("/")
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_source_defaults_to_claude_provider() {
        let s = ProjectSource {
            dir_name: "d".into(),
            path: PathBuf::from("/p"),
            session_files: vec![],
            cwd: Some("/p".into()),
            source_root: PathBuf::from("/r"),
            provider: Provider::Claude,
        };
        assert_eq!(s.provider, Provider::Claude);
    }

    #[test]
    fn test_derive_group_name() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            derive_group_name(&home.join("code/CCMeter")),
            "code/CCMeter"
        );
        assert_eq!(
            derive_group_name(&home.join("oppus/GitHubCode/Francis-Monorepo/Francis")),
            "Francis-Monorepo/Francis"
        );
    }

    #[test]
    fn test_discover_runs() {
        let (groups, _, _) = discover_project_groups_unified();
        for g in &groups {
            assert!(!g.name.is_empty());
            // Codex-only groups have session_files=[] (Codex cache/index are
            // produced separately), so total_sessions may legitimately be 0.
        }
    }

    fn src(cwd: &str, provider: Provider) -> ProjectSource {
        ProjectSource {
            dir_name: cwd.into(),
            path: PathBuf::from(cwd),
            session_files: vec![PathBuf::from(format!("{cwd}/s.jsonl"))],
            cwd: Some(cwd.into()),
            source_root: PathBuf::from("/r"),
            provider,
        }
    }

    #[test]
    fn codex_worktree_and_claude_main_collapse_to_one_group() {
        use crate::config::identity::{IdentitySource, IdentityStore, ResolvedIdentity};
        let mut store = IdentityStore::default();
        let crab = ResolvedIdentity {
            remote_url: Some("github.com/lucaxiang/crab".into()),
            canonical_root: "/nonexistent-test/crab".into(),
            source: IdentitySource::CodexSeed,
        };
        // Claude main + a Codex worktree both resolve to the crab remote.
        // Use clearly-nonexistent paths so live git resolution returns None and
        // the seeded identities are used (keeps the test hermetic).
        store.insert("/nonexistent-test/crab".into(), crab.clone());
        store.insert("/nonexistent-test/crab-worktrees/x".into(), crab);
        let sources = vec![
            src("/nonexistent-test/crab", Provider::Claude),
            src("/nonexistent-test/crab-worktrees/x", Provider::Codex),
        ];
        let groups = group_with_store(sources, &mut store);
        assert_eq!(groups.len(), 1, "both providers in one crab group");
        let g = &groups[0];
        let cwds: Vec<&str> = g.sources.iter().filter_map(|s| s.cwd.as_deref()).collect();
        assert!(cwds.contains(&"/nonexistent-test/crab"));
        assert!(cwds.contains(&"/nonexistent-test/crab-worktrees/x"));
    }

    #[test]
    fn codex_only_repo_gets_its_own_group() {
        use crate::config::identity::{IdentitySource, IdentityStore, ResolvedIdentity};
        let mut store = IdentityStore::default();
        store.insert(
            "/nonexistent-test/obs".into(),
            ResolvedIdentity {
                remote_url: None,
                canonical_root: "/nonexistent-test/obs".into(),
                source: IdentitySource::PathFallback,
            },
        );
        let groups = group_with_store(vec![src("/nonexistent-test/obs", Provider::Codex)], &mut store);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].sources[0].provider, Provider::Codex);
    }
}
