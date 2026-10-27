//! Honeypot configuration (cached in memory) and its SQLite persistence.
//! The schema is unchanged from the Python bot, so existing databases keep working.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::cogs::now_iso;

const DB_FILE: &str = "database/honeypot.db";

#[derive(Clone, Debug)]
pub struct ChannelConfig {
    pub channel_id: u64,
    pub enabled: bool,
    pub auto_ban: bool,
    pub custom_message: Option<String>,
    pub punishment_type: String,
    pub punishment_duration: i64,
    pub whitelisted_users: Vec<u64>,
    pub whitelisted_roles: Vec<u64>,
    pub min_account_age_days: i64,
    pub check_patterns: bool,
}

impl ChannelConfig {
    pub fn new(channel_id: u64) -> Self {
        Self {
            channel_id,
            enabled: true,
            auto_ban: false,
            custom_message: None,
            punishment_type: "ban".to_string(),
            punishment_duration: 0,
            whitelisted_users: vec![],
            whitelisted_roles: vec![],
            min_account_age_days: 0,
            check_patterns: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GuildData {
    pub ban_count: i64,
    pub admin_role_id: Option<u64>,
    pub notification_channel_id: Option<u64>,
    pub notification_dm: bool,
    pub admin_user_ids: Vec<u64>,
    pub channels: Vec<ChannelConfig>,
}

impl GuildData {
    pub fn channel_mut(&mut self, channel_id: u64) -> Option<&mut ChannelConfig> {
        self.channels.iter_mut().find(|c| c.channel_id == channel_id)
    }
}

pub struct ActivityLog {
    pub user_id: u64,
    pub username: String,
    pub action: String,
    pub details: Value,
    pub timestamp: String,
}

pub struct AuditEntry {
    pub admin_id: u64,
    pub action: String,
    pub details: Value,
    pub timestamp: String,
}

pub struct Offender {
    pub username: String,
    pub first_offense: String,
    pub last_offense: String,
    pub offense_count: i64,
    pub servers: Vec<u64>,
}

fn id_list(json: Option<String>) -> Vec<u64> {
    json.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn json_value(json: Option<String>) -> Value {
    json.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| Value::Object(Default::default()))
}

pub struct Store {
    conn: Connection,
    pub guilds: HashMap<u64, GuildData>,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS guilds (
                guild_id INTEGER PRIMARY KEY,
                ban_count INTEGER DEFAULT 0,
                admin_role_id INTEGER,
                notification_channel_id INTEGER,
                notification_dm BOOLEAN DEFAULT 0,
                admin_user_ids TEXT
            );
            CREATE TABLE IF NOT EXISTS honeypot_channels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER,
                channel_id INTEGER,
                enabled BOOLEAN DEFAULT 1,
                auto_ban BOOLEAN DEFAULT 0,
                custom_message TEXT,
                punishment_type TEXT DEFAULT 'ban',
                punishment_duration INTEGER DEFAULT 0,
                whitelisted_users TEXT,
                whitelisted_roles TEXT,
                min_account_age_days INTEGER DEFAULT 0,
                check_patterns BOOLEAN DEFAULT 1,
                FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
            );
            CREATE TABLE IF NOT EXISTS activity_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER,
                user_id INTEGER,
                username TEXT,
                action TEXT,
                details TEXT,
                timestamp TEXT,
                FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
            );
            CREATE INDEX IF NOT EXISTS idx_user_id ON activity_logs(user_id);
            CREATE INDEX IF NOT EXISTS idx_guild_id ON activity_logs(guild_id);
            CREATE TABLE IF NOT EXISTS offenders (
                user_id INTEGER PRIMARY KEY,
                username TEXT,
                first_offense TEXT,
                last_offense TEXT,
                offense_count INTEGER DEFAULT 0,
                servers TEXT
            );
            CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER,
                admin_id INTEGER,
                action TEXT,
                details TEXT,
                timestamp TEXT,
                FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
            );",
        )?;

        let mut store = Self { conn, guilds: HashMap::new() };
        store.load()?;
        crate::log::info(format!("[HONEYPOT] Loaded data for {} guilds", store.guilds.len()));
        Ok(store)
    }

    fn load(&mut self) -> rusqlite::Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT guild_id, ban_count, admin_role_id, notification_channel_id, notification_dm, admin_user_ids FROM guilds",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                GuildData {
                    ban_count: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    admin_role_id: row.get(2)?,
                    notification_channel_id: row.get(3)?,
                    notification_dm: row.get::<_, Option<bool>>(4)?.unwrap_or(false),
                    admin_user_ids: id_list(row.get(5)?),
                    channels: vec![],
                },
            ))
        })?;
        for row in rows {
            let (guild_id, data) = row?;
            self.guilds.insert(guild_id, data);
        }

        let mut stmt = self.conn.prepare(
            "SELECT guild_id, channel_id, enabled, auto_ban, custom_message, punishment_type, punishment_duration,
                    whitelisted_users, whitelisted_roles, min_account_age_days, check_patterns
             FROM honeypot_channels",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                ChannelConfig {
                    channel_id: row.get(1)?,
                    enabled: row.get::<_, Option<bool>>(2)?.unwrap_or(true),
                    auto_ban: row.get::<_, Option<bool>>(3)?.unwrap_or(false),
                    custom_message: row.get(4)?,
                    punishment_type: row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "ban".to_string()),
                    punishment_duration: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                    whitelisted_users: id_list(row.get(7)?),
                    whitelisted_roles: id_list(row.get(8)?),
                    min_account_age_days: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                    check_patterns: row.get::<_, Option<bool>>(10)?.unwrap_or(true),
                },
            ))
        })?;
        for row in rows {
            let (guild_id, channel) = row?;
            if let Some(guild) = self.guilds.get_mut(&guild_id) {
                guild.channels.push(channel);
            }
        }
        Ok(())
    }

    /// Writes one guild's in-memory config (including its channels) to the database.
    pub fn save_guild(&mut self, guild_id: u64) -> rusqlite::Result<()> {
        let Some(guild) = self.guilds.get(&guild_id) else { return Ok(()) };
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO guilds
             (guild_id, ban_count, admin_role_id, notification_channel_id, notification_dm, admin_user_ids)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                guild_id,
                guild.ban_count,
                guild.admin_role_id,
                guild.notification_channel_id,
                guild.notification_dm,
                serde_json::to_string(&guild.admin_user_ids).unwrap(),
            ],
        )?;
        tx.execute("DELETE FROM honeypot_channels WHERE guild_id = ?", [guild_id])?;
        for channel in &guild.channels {
            tx.execute(
                "INSERT INTO honeypot_channels
                 (guild_id, channel_id, enabled, auto_ban, custom_message, punishment_type, punishment_duration,
                  whitelisted_users, whitelisted_roles, min_account_age_days, check_patterns)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    guild_id,
                    channel.channel_id,
                    channel.enabled,
                    channel.auto_ban,
                    channel.custom_message,
                    channel.punishment_type,
                    channel.punishment_duration,
                    serde_json::to_string(&channel.whitelisted_users).unwrap(),
                    serde_json::to_string(&channel.whitelisted_roles).unwrap(),
                    channel.min_account_age_days,
                    channel.check_patterns,
                ],
            )?;
        }
        tx.commit()
    }

    /// Removes data for guilds the bot has left and honeypot channels that were deleted.
    /// `current` maps each guild the bot is in to its channel IDs.
    pub fn cleanup_stale(&mut self, current: &HashMap<u64, HashSet<u64>>) -> rusqlite::Result<()> {
        let guild_ids: Vec<u64> = self.guilds.keys().copied().collect();
        for guild_id in guild_ids {
            let Some(channels) = current.get(&guild_id) else {
                crate::log::warning(format!("[HONEYPOT] Guild {guild_id} no longer exists, removing from database"));
                let tx = self.conn.transaction()?;
                for table in ["guilds", "honeypot_channels", "activity_logs", "audit_log"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE guild_id = ?"), [guild_id])?;
                }
                tx.commit()?;
                self.guilds.remove(&guild_id);
                continue;
            };

            let guild = self.guilds.get_mut(&guild_id).unwrap();
            let before = guild.channels.len();
            guild.channels.retain(|c| {
                let exists = channels.contains(&c.channel_id);
                if !exists {
                    crate::log::warning(format!(
                        "[HONEYPOT] Channel {} in guild {guild_id} no longer exists, removing from database",
                        c.channel_id
                    ));
                }
                exists
            });
            if guild.channels.len() != before {
                self.save_guild(guild_id)?;
            }
        }
        Ok(())
    }

    pub fn add_audit_log(&self, guild_id: u64, admin_id: u64, action: &str, details: Value) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO audit_log (guild_id, admin_id, action, details, timestamp) VALUES (?, ?, ?, ?, ?)",
            params![guild_id, admin_id, action, details.to_string(), now_iso()],
        )?;
        // Keep only the last 100 entries per guild
        self.conn.execute(
            "DELETE FROM audit_log
             WHERE id NOT IN (SELECT id FROM audit_log WHERE guild_id = ? ORDER BY timestamp DESC LIMIT 100)
             AND guild_id = ?",
            params![guild_id, guild_id],
        )?;
        Ok(())
    }

    pub fn log_activity(&self, guild_id: u64, user_id: u64, username: &str, action: &str, details: Value) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO activity_logs (guild_id, user_id, username, action, details, timestamp) VALUES (?, ?, ?, ?, ?, ?)",
            params![guild_id, user_id, username, action, details.to_string(), now_iso()],
        )?;
        Ok(())
    }

    /// Activity logs for a guild, newest first. `user_id` narrows to one user.
    pub fn activity_logs(&self, guild_id: u64, user_id: Option<u64>, limit: Option<i64>) -> rusqlite::Result<Vec<ActivityLog>> {
        let mut stmt = self.conn.prepare(
            "SELECT user_id, username, action, details, timestamp FROM activity_logs
             WHERE guild_id = ?1 AND (?2 IS NULL OR user_id = ?2)
             ORDER BY timestamp DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![guild_id, user_id, limit.unwrap_or(-1)], |row| {
            Ok(ActivityLog {
                user_id: row.get::<_, Option<u64>>(0)?.unwrap_or(0),
                username: row.get::<_, Option<String>>(1)?.unwrap_or_else(|| "Unknown".to_string()),
                action: row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "Unknown".to_string()),
                details: json_value(row.get(3)?),
                timestamp: row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "Unknown".to_string()),
            })
        })?;
        rows.collect()
    }

    /// Raw activity log rows for CSV export: (timestamp, user_id, username, action, details).
    pub fn export_rows(&self, guild_id: u64) -> rusqlite::Result<Vec<[String; 5]>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, user_id, username, action, details FROM activity_logs WHERE guild_id = ? ORDER BY timestamp DESC",
        )?;
        let rows = stmt.query_map([guild_id], |row| {
            Ok([
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                row.get::<_, Option<u64>>(1)?.map(|id| id.to_string()).unwrap_or_default(),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            ])
        })?;
        rows.collect()
    }

    pub fn clear_activity_logs(&self, guild_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM activity_logs WHERE guild_id = ?", [guild_id])?;
        Ok(())
    }

    /// Guilds that have activity logged for this user.
    pub fn guilds_with_activity(&self, user_id: u64) -> rusqlite::Result<Vec<u64>> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT guild_id FROM activity_logs WHERE user_id = ?")?;
        let rows = stmt.query_map([user_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Guilds where this user was previously banned by the honeypot.
    pub fn previous_ban_guilds(&self, user_id: u64) -> rusqlite::Result<Vec<u64>> {
        let mut stmt =
            self.conn.prepare("SELECT DISTINCT guild_id FROM activity_logs WHERE user_id = ? AND action = 'user_banned'")?;
        let rows = stmt.query_map([user_id], |row| row.get(0))?;
        rows.collect()
    }

    pub fn audit_log(&self, guild_id: u64, limit: i64) -> rusqlite::Result<Vec<AuditEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT admin_id, action, details, timestamp FROM audit_log WHERE guild_id = ? ORDER BY timestamp DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![guild_id, limit], |row| {
            Ok(AuditEntry {
                admin_id: row.get::<_, Option<u64>>(0)?.unwrap_or(0),
                action: row.get::<_, Option<String>>(1)?.unwrap_or_else(|| "Unknown".to_string()),
                details: json_value(row.get(2)?),
                timestamp: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "Unknown".to_string()),
            })
        })?;
        rows.collect()
    }

    pub fn add_offender(&self, user_id: u64, username: &str, guild_id: u64) -> rusqlite::Result<()> {
        let now = now_iso();
        match self.offender(user_id)? {
            Some(mut offender) => {
                if !offender.servers.contains(&guild_id) {
                    offender.servers.push(guild_id);
                }
                self.conn.execute(
                    "UPDATE offenders SET username = ?, last_offense = ?, offense_count = ?, servers = ? WHERE user_id = ?",
                    params![username, now, offender.offense_count + 1, serde_json::to_string(&offender.servers).unwrap(), user_id],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO offenders (user_id, username, first_offense, last_offense, offense_count, servers)
                     VALUES (?, ?, ?, ?, 1, ?)",
                    params![user_id, username, now, now, serde_json::to_string(&[guild_id]).unwrap()],
                )?;
            }
        }
        Ok(())
    }

    pub fn offender(&self, user_id: u64) -> rusqlite::Result<Option<Offender>> {
        self.conn
            .query_row(
                "SELECT username, first_offense, last_offense, offense_count, servers FROM offenders WHERE user_id = ?",
                [user_id],
                |row| {
                    Ok(Offender {
                        username: row.get::<_, Option<String>>(0)?.unwrap_or_else(|| "Unknown".to_string()),
                        first_offense: row.get::<_, Option<String>>(1)?.unwrap_or_else(|| "Unknown".to_string()),
                        last_offense: row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "Unknown".to_string()),
                        offense_count: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        servers: id_list(row.get(4)?),
                    })
                },
            )
            .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads a copy of the real database (if present) and checks a save round-trips.
    #[test]
    fn loads_and_round_trips_existing_database() {
        let Ok(_) = std::fs::metadata(DB_FILE) else { return };
        let copy = std::env::temp_dir().join(format!("honeypot-test-{}.db", std::process::id()));
        std::fs::copy(DB_FILE, &copy).unwrap();

        let mut store = Store::open_path(copy.to_str().unwrap()).unwrap();
        let ids: Vec<u64> = store.guilds.keys().copied().collect();
        for id in &ids {
            store.save_guild(*id).unwrap();
        }
        let reloaded = Store::open_path(copy.to_str().unwrap()).unwrap();
        for id in &ids {
            let (a, b) = (&store.guilds[id], &reloaded.guilds[id]);
            assert_eq!(a.channels.len(), b.channels.len());
            assert_eq!(a.ban_count, b.ban_count);
            assert_eq!(a.admin_user_ids, b.admin_user_ids);
        }
        for id in &ids {
            store.activity_logs(*id, None, Some(10)).unwrap();
            store.audit_log(*id, 10).unwrap();
            store.export_rows(*id).unwrap();
        }
        std::fs::remove_file(copy).ok();
    }
}
