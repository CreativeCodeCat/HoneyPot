//! What each cog keeps in the database, for the dashboard's cog info pages. Only tables with
//! a `guild_id` column are shown, and only the server's own rows, so one server's managers
//! never see another's data. Everything is opened read-only.

use std::collections::HashSet;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

/// Rows on a page of a table
pub const PAGE_SIZE: usize = 50;

/// A table a cog keeps data in.
struct Source {
    file: &'static str,
    /// None for every table in the file
    table: Option<&'static str>,
    /// Extra condition for tables shared with other cogs (fixed SQL, never from a request)
    filter: Option<&'static str>,
}

const fn file(file: &'static str) -> Source {
    Source { file, table: None, filter: None }
}

/// The server-setup settings a cog owns.
const fn setting(filter: &'static str) -> Source {
    Source { file: "server_setup.db", table: Some("settings"), filter: Some(filter) }
}

/// A `'static` list of sources (calls to `file` and `setting` aren't promoted on their own).
macro_rules! sources {
    ($($source:expr),* $(,)?) => {{
        const SOURCES: &[Source] = &[$($source),*];
        SOURCES
    }};
}

/// The data each cog keeps, by its name in `COGS`.
fn sources(cog: &str) -> &'static [Source] {
    match cog {
        "AFK" => sources![file("afk.db")],
        "Announcements" => sources![setting("key = 'channel.announcements'")],
        "Applications" => sources![file("applications.db")],
        "CogManager" => sources![file("cog_config.db"), file("cooldowns.db")],
        "Economy" => sources![file("economy.db")],
        "Games" => sources![file("counting.db"), file("guessing.db"), setting("key = 'channel.games'")],
        "HoneyPot" => sources![file("honeypot.db")],
        "Incidents" => sources![file("incidents.db")],
        "Levels" => sources![file("levels.db")],
        "LiveStatus" => sources![file("live_status.db")],
        "Lockdown" => sources![file("lockdown.db")],
        "Logs" => sources![setting("key LIKE 'logs.%'")],
        "Pets" => sources![file("pets.db")],
        "Quarantine" => sources![file("quarantine.db")],
        "RaidProtection" => sources![file("raid_protection.db")],
        "ReactionRoles" => sources![file("reaction_roles.db")],
        "Rules" => sources![file("rules.db")],
        "StaffPolicy" => sources![file("staff_policy.db")],
        "Sticky" => sources![file("sticky.db")],
        "Suggestions" => sources![file("suggestions.db")],
        "SuspiciousAccounts" => sources![file("suspicious_accounts.db")],
        "Tickets" => sources![file("tickets.db")],
        _ => sources![],
    }
}

/// What a table holds for a server that hasn't saved anything, from the cog's own defaults.
enum Defaults {
    /// One row of settings, shown when the server has none saved
    Row(Vec<(&'static str, Value)>),
    /// One row per key in `column`, filling in the keys the server hasn't saved
    Keys { column: &'static str, keys: Vec<String>, values: Vec<(&'static str, Value)> },
}

/// The defaults for one of a cog's tables, if it has any.
fn defaults(cog: &str, file: &str, table: &str) -> Option<Defaults> {
    use crate::database::repositories::{economy, leveling, raid_protection, sticky, suspicious_accounts};
    use crate::features::{normalize_cog_name, server_setup, COGS, COG_MANAGER};
    let setting = |key: &str| Defaults::Keys { column: "key", keys: vec![key.to_string()], values: vec![("value", Value::Null)] };
    Some(match (file, table) {
        ("cog_config.db", "cog_config") => Defaults::Keys {
            column: "cog_name",
            keys: COGS.iter().map(|c| normalize_cog_name(c)).filter(|c| c != COG_MANAGER).collect(),
            values: vec![("enabled", Value::Bool(false))],
        },
        ("server_setup.db", "settings") => match cog {
            "Logs" => Defaults::Keys {
                column: "key",
                keys: server_setup::LOG_TYPES.iter().map(|&log| server_setup::log_key(log)).collect(),
                values: vec![("value", Value::Null)],
            },
            "Games" => setting("channel.games"),
            "Announcements" => setting("channel.announcements"),
            _ => return None,
        },
        ("levels.db", "settings") => {
            let d = leveling::Settings::default();
            Defaults::Row(vec![
                ("rate", json!(d.rate)),
                ("min_xp", json!(d.base_xp)),
                ("max_xp", json!(d.max_xp)),
                ("levelup_channel_id", json!(d.levelup_channel_id)),
                ("engagement_window_mins", json!(d.engagement_window_mins)),
                ("xp_cooldown_secs", json!(d.xp_cooldown_secs)),
            ])
        }
        ("raid_protection.db", "settings") => {
            let d = raid_protection::Settings::default();
            Defaults::Row(vec![
                ("joins", json!(d.joins)),
                ("seconds", json!(d.seconds)),
                ("quiet_minutes", json!(d.quiet_minutes)),
                ("lock_channels", json!(d.lock_channels)),
                ("raise_verification", json!(d.raise_verification)),
                ("joiner_action", json!(d.joiner_action.key())),
                ("timeout_minutes", json!(d.timeout_minutes)),
                ("alert_channel", json!(d.alert_channel)),
            ])
        }
        ("suspicious_accounts.db", "settings") => {
            let d = suspicious_accounts::Settings::default();
            Defaults::Row(vec![
                ("threshold", json!(d.threshold)),
                ("alert_channel", json!(d.alert_channel)),
                ("auto_quarantine", json!(d.auto_quarantine)),
            ])
        }
        ("sticky.db", "settings") => Defaults::Row(vec![("delay_secs", json!(sticky::DEFAULT_DELAY_SECS))]),
        ("economy.db", "daily_rates") => {
            let d = economy::DEFAULT_DAILY;
            Defaults::Row(vec![("role_id", json!(d.role_id)), ("min_amount", json!(d.min)), ("max_amount", json!(d.max))])
        }
        // Nothing set: no channels or roles
        ("quarantine.db", "settings") | ("staff_policy.db", "settings") => Defaults::Row(Vec::new()),
        _ => return None,
    })
}

/// One of a cog's tables, with how many rows the server has in it (counting defaults).
pub struct Table {
    /// "file:table", as the info page asks for it
    pub id: String,
    pub name: String,
    pub rows: usize,
    file: &'static str,
    table: String,
    filter: Option<&'static str>,
    defaults: Option<Defaults>,
}

fn open(file: &str) -> rusqlite::Result<Connection> {
    let path = format!("{}/{file}", crate::database::connection::DIR);
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn condition(filter: Option<&str>) -> String {
    match filter {
        Some(filter) => format!("guild_id = ?1 AND ({filter})"),
        None => "guild_id = ?1".to_string(),
    }
}

/// The cog's tables that hold rows for the server, skipping files that don't exist yet.
pub fn tables(cog: &str, guild_id: u64) -> Vec<Table> {
    let sources = sources(cog);
    let several_files = sources.iter().map(|s| s.file).collect::<HashSet<_>>().len() > 1;
    let mut tables = Vec::new();
    for source in sources {
        let Ok(conn) = open(source.file) else { continue };
        let names: Vec<String> = match source.table {
            Some(table) => vec![table.to_string()],
            None => {
                let Ok(mut stmt) = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY rowid") else { continue };
                let Ok(rows) = stmt.query_map([], |row| row.get(0)) else { continue };
                rows.filter_map(Result::ok).collect()
            }
        };
        for name in names {
            let has_guild = conn
                .query_row("SELECT COUNT(*) > 0 FROM pragma_table_info(?1) WHERE name = 'guild_id'", [&name], |row| row.get(0))
                .unwrap_or(false);
            if !has_guild {
                continue;
            }
            let sql = format!("SELECT COUNT(*) FROM {} WHERE {}", quote(&name), condition(source.filter));
            let saved: usize = conn.query_row(&sql, [guild_id as i64], |row| row.get(0)).unwrap_or(0);
            let defaults = defaults(cog, source.file, &name);
            let missing = match &defaults {
                None => 0,
                Some(Defaults::Row(_)) => usize::from(saved == 0),
                Some(Defaults::Keys { column, keys, .. }) => {
                    let saved_keys = saved_keys(&conn, &name, column, source.filter, guild_id);
                    keys.iter().filter(|k| !saved_keys.contains(*k)).count()
                }
            };
            let stem = source.file.trim_end_matches(".db");
            let label = if several_files && stem != name { format!("{stem} · {name}") } else { name.clone() };
            tables.push(Table {
                id: format!("{}:{name}", source.file),
                name: label,
                rows: saved + missing,
                file: source.file,
                table: name,
                filter: source.filter,
                defaults,
            });
        }
    }
    tables
}

/// The keys in `column` the server has rows for.
fn saved_keys(conn: &Connection, table: &str, column: &str, filter: Option<&str>, guild_id: u64) -> HashSet<String> {
    let sql = format!("SELECT {} FROM {} WHERE {}", quote(column), quote(table), condition(filter));
    let Ok(mut stmt) = conn.prepare(&sql) else { return HashSet::new() };
    stmt.query_map([guild_id as i64], |row| row.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Whether the cog has any rows for the server, or defaults to show.
pub fn has_data(cog: &str, guild_id: u64) -> bool {
    tables(cog, guild_id).iter().any(|t| t.rows > 0)
}

/// A cell as JSON. IDs are too big for JavaScript numbers, so large integers are strings.
fn cell(value: ValueRef) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) if n.unsigned_abs() > (1 << 53) => Value::String(n.to_string()),
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(format!("<{} bytes>", b.len())),
    }
}

/// A page of the server's rows in one of the cog's tables. Defaults for what the server
/// hasn't saved come after its own rows, and their indexes on the page are in `defaults`.
pub fn page(table: &Table, guild_id: u64, page: usize) -> rusqlite::Result<Value> {
    let conn = open(table.file)?;
    let pages = table.rows.div_ceil(PAGE_SIZE).max(1);
    let page = page.clamp(1, pages);
    // Tables with defaults are small settings tables, so they're read whole and paged here
    let limit = match table.defaults {
        Some(_) => String::new(),
        None => format!("LIMIT {PAGE_SIZE} OFFSET {}", (page - 1) * PAGE_SIZE),
    };
    let sql = format!("SELECT * FROM {} WHERE {} ORDER BY rowid {limit}", quote(&table.table), condition(table.filter));
    // SQLite stores yes/no columns as 0 and 1; their declared type says which they are
    let boolean: HashSet<String> = conn
        .prepare("SELECT name FROM pragma_table_info(?1) WHERE upper(type) LIKE 'BOOL%'")?
        .query_map([&table.table], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&sql)?;
    // The guild ID is the same on every row, and the page is already that server's
    let shown: Vec<(usize, String)> =
        stmt.column_names().iter().enumerate().filter(|(_, c)| **c != "guild_id").map(|(i, c)| (i, c.to_string())).collect();
    let columns: Vec<&String> = shown.iter().map(|(_, c)| c).collect();
    let mut rows: Vec<Value> = stmt
        .query_map([guild_id as i64], |row| {
            let cells = shown.iter().map(|&(i, ref name)| match row.get_ref(i) {
                Ok(ValueRef::Integer(n)) if boolean.contains(name) => Value::Bool(n != 0),
                Ok(value) => cell(value),
                Err(_) => Value::Null,
            });
            Ok(Value::Array(cells.collect()))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let Some(defaults) = &table.defaults else {
        return Ok(json!({ "columns": columns, "rows": rows, "defaults": [], "page": page, "pages": pages, "total": table.rows }));
    };
    let default_row = |values: &[(&str, Value)], key: Option<(&str, &str)>| -> Value {
        let cells = columns.iter().map(|c| match key {
            Some((column, key)) if column == c.as_str() => Value::String(key.to_string()),
            _ => values.iter().find(|(name, _)| name == c).map(|(_, v)| v.clone()).unwrap_or(Value::Null),
        });
        Value::Array(cells.collect())
    };
    let saved = rows.len();
    match defaults {
        Defaults::Row(values) if rows.is_empty() => rows.push(default_row(values, None)),
        Defaults::Row(_) => {}
        Defaults::Keys { column, keys, values } => {
            let at = columns.iter().position(|c| c == column);
            let saved_keys: HashSet<String> =
                rows.iter().filter_map(|r| r.get(at?)?.as_str().map(str::to_string)).collect();
            for key in keys.iter().filter(|k| !saved_keys.contains(*k)) {
                rows.push(default_row(values, Some((column, key))));
            }
        }
    }
    let start = (page - 1) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(rows.len());
    let defaults: Vec<usize> = (start..end).filter(|&i| i >= saved).map(|i| i - start).collect();
    let rows: Vec<Value> = rows.into_iter().skip(start).take(PAGE_SIZE).collect();
    Ok(json!({ "columns": columns, "rows": rows, "defaults": defaults, "page": page, "pages": pages, "total": table.rows }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cog_with_data_is_a_real_cog() {
        let known = ["AFK", "Announcements", "Applications", "CogManager", "Economy", "Games", "HoneyPot", "Incidents", "Levels", "LiveStatus", "Lockdown", "Logs", "Pets", "Quarantine", "RaidProtection", "ReactionRoles", "Rules", "StaffPolicy", "Sticky", "Suggestions", "SuspiciousAccounts", "Tickets"];
        for cog in known {
            assert!(crate::features::COGS.contains(&cog), "{cog} isn't in COGS");
            assert!(!sources(cog).is_empty());
        }
    }

    #[test]
    fn big_ids_are_strings() {
        assert_eq!(cell(ValueRef::Integer(1162534077681913887)), json!("1162534077681913887"));
        assert_eq!(cell(ValueRef::Integer(42)), json!(42));
    }
}
