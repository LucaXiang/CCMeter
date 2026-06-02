//! Canonical git identity resolution shared by Claude + Codex discovery:
//! normalize remote URLs, collapse worktree paths, and persist resolved
//! identities so deleted worktrees still group correctly.

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
}
