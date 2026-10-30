//! Numbers and lists for the dashboard's server pages: the server's security status and its
//! leaderboards. Read straight from the database files, read-only, so a missing file (a
//! feature never used) just counts as nothing.

use rusqlite::{Connection, OptionalExtension, Row};
use serde_json::{json, Value};

use super::cog_data::open;
use crate::database::repositories::leveling;

/// Members on each leaderboard
const LEADERBOARD_SIZE: usize = 10;
/// Recent incidents shown
const RECENT: usize = 8;
/// Honeypot log rows read for the recent catches; one catch can write several (the message,
/// known-offender matches and the punishment)
const RECENT_HONEYPOT_ROWS: usize = 60;

/// The first column of the first row, or the default if the file, table or row is missing.
fn one<T: rusqlite::types::FromSql + Default>(file: &str, sql: &str, guild_id: u64) -> T {
    open(file)
        .and_then(|conn| conn.query_row(sql, [guild_id as i64], |row| row.get(0)).optional())
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Every row, read with `read`; none if the file or table is missing.
fn all(file: &str, sql: &str, guild_id: u64, read: impl Fn(&Row) -> rusqlite::Result<Value>) -> Vec<Value> {
    let run = |conn: Connection| -> rusqlite::Result<Vec<Value>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([guild_id as i64], |row| read(row))?;
        rows.collect()
    };
    open(file).and_then(run).unwrap_or_default()
}

/// IDs as strings, as they're too big for JavaScript numbers.
fn id(row: &Row, i: usize) -> rusqlite::Result<Value> {
    Ok(row.get::<_, Option<i64>>(i)?.map_or(Value::Null, |id| Value::String(id.to_string())))
}

/// What's protecting the server and what it has caught.
pub fn security(guild_id: u64) -> Value {
    let raid_started: Option<i64> = one("raid_protection.db", "SELECT started_at FROM raids WHERE guild_id = ?1", guild_id);
    let recent_catches = all(
        "honeypot.db",
        &format!("SELECT user_id, username, action, details, timestamp FROM activity_logs WHERE guild_id = ?1 ORDER BY id DESC LIMIT {RECENT_HONEYPOT_ROWS}"),
        guild_id,
        |row| Ok(json!({ "user_id": id(row, 0)?, "username": row.get::<_, Option<String>>(1)?, "action": row.get::<_, Option<String>>(2)?, "details": row.get::<_, Option<String>>(3)?, "at": row.get::<_, Option<String>>(4)? })),
    );
    let recent_incidents = all(
        "incidents.db",
        &format!("SELECT id, user_id, username, action, reason, moderator_id, created_at FROM incidents WHERE guild_id = ?1 ORDER BY id DESC LIMIT {RECENT}"),
        guild_id,
        |row| Ok(json!({ "id": row.get::<_, i64>(0)?, "user_id": id(row, 1)?, "username": row.get::<_, String>(2)?, "action": row.get::<_, String>(3)?, "reason": row.get::<_, Option<String>>(4)?, "moderator_id": id(row, 5)?, "at": row.get::<_, String>(6)? })),
    );
    json!({
        "honeypot": {
            "channels": one::<i64>("honeypot.db", "SELECT COUNT(*) FROM honeypot_channels WHERE guild_id = ?1 AND enabled", guild_id),
            "bans": one::<i64>("honeypot.db", "SELECT ban_count FROM guilds WHERE guild_id = ?1", guild_id),
            "catches": one::<i64>("honeypot.db", "SELECT COUNT(*) FROM activity_logs WHERE guild_id = ?1", guild_id),
            "recent": recent_catches,
        },
        "incidents": {
            "total": one::<i64>("incidents.db", "SELECT COUNT(*) FROM incidents WHERE guild_id = ?1", guild_id),
            "recent": recent_incidents,
        },
        "open_tickets": one::<i64>("tickets.db", "SELECT COUNT(*) FROM tickets WHERE guild_id = ?1 AND NOT closed", guild_id),
        "quarantined": one::<i64>("quarantine.db", "SELECT COUNT(*) FROM quarantined WHERE guild_id = ?1", guild_id),
        "lockdowns": one::<i64>("lockdown.db", "SELECT COUNT(*) FROM lockdowns WHERE guild_id = ?1", guild_id),
        "raid": raid_started.map(|started| json!({ "started_at": started })),
        "members_ranked": one::<i64>("levels.db", "SELECT COUNT(*) FROM members WHERE guild_id = ?1", guild_id),
        "economy_accounts": one::<i64>("economy.db", "SELECT COUNT(*) FROM accounts WHERE guild_id = ?1", guild_id),
    })
}

/// The server's top members by XP and by money.
pub fn leaderboards(guild_id: u64) -> Value {
    let rate: i64 = one("levels.db", "SELECT rate FROM settings WHERE guild_id = ?1", guild_id);
    let rate = if rate > 0 { rate } else { leveling::Settings::default().rate };
    let levels = all(
        "levels.db",
        &format!("SELECT user_id, xp, rep FROM members WHERE guild_id = ?1 ORDER BY xp DESC LIMIT {LEADERBOARD_SIZE}"),
        guild_id,
        |row| {
            let xp: i64 = row.get(1)?;
            Ok(json!({ "user_id": id(row, 0)?, "xp": xp, "level": leveling::level_for(xp, rate), "rep": row.get::<_, i64>(2)? }))
        },
    );
    let economy = all(
        "economy.db",
        &format!("SELECT user_id, wallet, bank FROM accounts WHERE guild_id = ?1 ORDER BY wallet + bank DESC LIMIT {LEADERBOARD_SIZE}"),
        guild_id,
        |row| {
            let (wallet, bank): (i64, i64) = (row.get(1)?, row.get(2)?);
            Ok(json!({ "user_id": id(row, 0)?, "wallet": wallet, "bank": bank, "total": wallet + bank }))
        },
    );
    json!({ "levels": levels, "economy": economy })
}
