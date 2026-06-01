//! Read `~/.code-insights/data.db` (Code Insights, a third-party Claude Code
//! analytics tool). Its `messages.usage` carries per-message delta token
//! counts back to ~2026-03-25, richer than stats-cache (has project_path).
//! Read-only; if the DB is absent/locked/unrecognized we return nothing and
//! let stats-cache cover those dates.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, NaiveDate};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use super::{BackfillDay, CODE_INSIGHTS_ROOT};
use crate::data::cache::DayEntry;
use crate::data::models::cost_from_tokens;

#[derive(Deserialize)]
struct CiUsage {
    #[serde(rename = "inputTokens", default)]
    input: u64,
    #[serde(rename = "outputTokens", default)]
    output: u64,
    #[serde(rename = "cacheReadTokens", default)]
    cache_read: u64,
    #[serde(rename = "cacheCreationTokens", default)]
    cache_creation: u64,
    #[serde(default)]
    model: String,
}

pub fn read_code_insights(db_path: &Path) -> Vec<BackfillDay> {
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return Vec::new();
    };
    read_from_conn(&conn)
}

fn read_from_conn(conn: &Connection) -> Vec<BackfillDay> {
    let mut acc: HashMap<(String, NaiveDate), DayEntry> = HashMap::new();

    let sql = "SELECT m.timestamp, m.usage, s.project_path \
               FROM messages m JOIN sessions s ON m.session_id = s.id \
               WHERE m.usage IS NOT NULL AND m.usage != ''";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };

    for row in rows.flatten() {
        let (ts_str, usage_str, project_path) = row;
        if project_path.is_empty() {
            continue;
        }
        let Ok(usage) = serde_json::from_str::<CiUsage>(&usage_str) else {
            continue;
        };
        if usage.input + usage.output + usage.cache_read + usage.cache_creation == 0 {
            continue;
        }
        let Ok(ts) = DateTime::parse_from_rfc3339(&ts_str) else {
            continue;
        };
        let date = ts.with_timezone(&chrono::Local).date_naive();

        let e = acc.entry((project_path, date)).or_default();
        e.input += usage.input;
        e.output += usage.output;
        e.cache_read += usage.cache_read;
        e.cache_creation += usage.cache_creation;
        e.cost += cost_from_tokens(
            &usage.model,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_creation,
        );
    }

    acc.into_iter()
        .map(|((cwd, date), entry)| BackfillDay {
            root: CODE_INSIGHTS_ROOT.to_string(),
            cwd,
            date,
            entry,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, project_path TEXT);
             CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, usage TEXT, timestamp TEXT);
             INSERT INTO sessions VALUES ('s1','/Users/x/proj');
             INSERT INTO messages VALUES ('m1','s1','assistant',
               '{\"inputTokens\":10,\"outputTokens\":7,\"cacheReadTokens\":100,\"cacheCreationTokens\":5,\"model\":\"claude-opus-4-6\"}',
               '2026-03-25T12:00:00.000Z');
             INSERT INTO messages VALUES ('m2','s1','assistant',
               '{\"inputTokens\":20,\"outputTokens\":3,\"cacheReadTokens\":0,\"cacheCreationTokens\":0,\"model\":\"claude-opus-4-6\"}',
               '2026-03-25T13:00:00.000Z');
             INSERT INTO messages VALUES ('m3','s1','user','', '2026-03-25T13:05:00.000Z');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn aggregates_messages_by_cwd_and_date() {
        let days = read_from_conn(&seed());
        assert_eq!(days.len(), 1);
        let d = &days[0];
        assert_eq!(d.root, crate::data::backfill::CODE_INSIGHTS_ROOT);
        assert_eq!(d.cwd, "/Users/x/proj");
        assert_eq!(d.entry.input, 30);
        assert_eq!(d.entry.output, 10);
        assert_eq!(d.entry.cache_read, 100);
        assert_eq!(d.entry.cache_creation, 5);
        assert!(d.entry.cost > 0.0);
    }

    #[test]
    fn missing_db_returns_empty() {
        let days = read_code_insights(std::path::Path::new("/no/such/file.db"));
        assert!(days.is_empty());
    }
}
