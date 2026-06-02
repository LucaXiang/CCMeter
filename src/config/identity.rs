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
