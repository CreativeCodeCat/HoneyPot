//! Per-guild command cooldowns. Admins set how many seconds each member must wait between uses
//! of a command; commands without a setting fall back to `DEFAULTS`, or no cooldown at all.

use std::collections::HashMap;

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use super::{games, Context, Data, Error};

const DB_FILE: &str = "database/cooldowns.db";
/// Longest cooldown that can be set: one week
const MAX_COOLDOWN_SECS: u64 = 7 * 24 * 60 * 60;

/// Cooldowns for commands that had one before they could be configured
const DEFAULTS: &[(&str, u64)] = &[
    ("ping", 300),
    ("hack", games::HACK_COOLDOWN_SECS as u64),
    ("heist", games::heist::HEIST_COOLDOWN_SECS as u64),
    ("fight", games::fight::FIGHT_COOLDOWN_SECS as u64),
];

/// Commands that start their own cooldown (after checking their input), so the
/// framework-wide check leaves them alone. `heist` cools down for the whole server
/// once a heist ends, and `fight` for both fighters once a fight ends.
const SELF_MANAGED: &[&str] = &["hack", "heist", "fight"];

/// Configured cooldowns, cached in memory, and when each member last used each command,
/// both persisted to SQLite so running cooldowns survive restarts.
pub struct Cooldowns {
    conn: Connection,
    config: HashMap<(u64, String), u64>,
}

impl Cooldowns {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS command_cooldowns (
                guild_id INTEGER NOT NULL,
                command TEXT NOT NULL,
                seconds INTEGER NOT NULL,
                PRIMARY KEY (guild_id, command)
            );
            CREATE TABLE IF NOT EXISTS last_used (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                command TEXT NOT NULL,
                -- Unix time the member last used the command
                used_at INTEGER NOT NULL,
                PRIMARY KEY (guild_id, user_id, command)
            )",
        )?;
        let config = {
            let mut stmt = conn.prepare("SELECT guild_id, command, seconds FROM command_cooldowns")?;
            let rows = stmt.query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Self { conn, config })
    }

    /// Seconds between uses of `command`, by its qualified name (e.g. "eco work").
    pub fn seconds(&self, guild_id: u64, command: &str) -> u64 {
        self.config.get(&(guild_id, command.to_string())).copied().unwrap_or_else(|| default_for(command))
    }

    pub fn set(&mut self, guild_id: u64, command: &str, seconds: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO command_cooldowns (guild_id, command, seconds) VALUES (?, ?, ?)",
            params![guild_id, command, seconds],
        )?;
        self.config.insert((guild_id, command.to_string()), seconds);
        Ok(())
    }

    /// The Unix time the member can use the command again, if its cooldown is still running at `now`.
    pub fn ready_at(&self, guild_id: u64, user_id: u64, command: &str, now: i64) -> rusqlite::Result<Option<i64>> {
        let cooldown = self.seconds(guild_id, command) as i64;
        if cooldown == 0 {
            return Ok(None);
        }
        let last: Option<i64> = self
            .conn
            .query_row(
                "SELECT used_at FROM last_used WHERE guild_id = ? AND user_id = ? AND command = ?",
                params![guild_id, user_id, command],
                |row| row.get(0),
            )
            .optional()?;
        Ok(last.map(|last| last + cooldown).filter(|ready| now < *ready))
    }

    /// Records a use at Unix time `now`, or returns the Unix time the member can use the command again.
    pub fn start(&self, guild_id: u64, user_id: u64, command: &str, now: i64) -> rusqlite::Result<Result<(), i64>> {
        if self.seconds(guild_id, command) == 0 {
            return Ok(Ok(()));
        }
        if let Some(ready) = self.ready_at(guild_id, user_id, command, now)? {
            return Ok(Err(ready));
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO last_used (guild_id, user_id, command, used_at) VALUES (?, ?, ?, ?)",
            params![guild_id, user_id, command, now],
        )?;
        Ok(Ok(()))
    }

    /// Removes settings for guilds the bot is no longer in.
    pub fn cleanup_stale(&mut self, current_guilds: &[serenity::GuildId]) {
        let in_guild = |id: u64| current_guilds.iter().any(|g| g.get() == id);
        let stale = self.conn.prepare("SELECT guild_id FROM command_cooldowns UNION SELECT guild_id FROM last_used").and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, u64>(0))?.collect::<rusqlite::Result<Vec<u64>>>()
        });
        let stale = match stale {
            Ok(ids) => ids.into_iter().filter(|g| !in_guild(*g)),
            Err(e) => return crate::log::error(format!("Error cleaning up stale cooldowns: {e}")),
        };
        for guild_id in stale {
            let deleted = self.conn.execute("DELETE FROM command_cooldowns WHERE guild_id = ?", [guild_id]).and_then(|_| {
                self.conn.execute("DELETE FROM last_used WHERE guild_id = ?", [guild_id])
            });
            if let Err(e) = deleted {
                crate::log::error(format!("Error cleaning up stale cooldowns: {e}"));
                continue;
            }
            self.config.retain(|(g, _), _| *g != guild_id);
        }
    }
}

fn default_for(command: &str) -> u64 {
    DEFAULTS.iter().find(|(name, _)| *name == command).map_or(0, |(_, secs)| *secs)
}

/// Framework-wide check: starts the member's cooldown, or fails with a message if it's still running.
pub fn check(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let command = &ctx.command().qualified_name;
    if SELF_MANAGED.contains(&command.as_str()) {
        return Ok(());
    }
    let started = ctx.data().cooldowns.lock().unwrap().start(guild_id.get(), ctx.author().id.get(), command, chrono::Utc::now().timestamp())?;
    started.map_err(|ready| format!("⏱️ This command is on cooldown. Try again <t:{ready}:R>.").into())
}

/// Qualified names of every runnable command, subcommands included.
fn command_names(commands: &[poise::Command<Data, Error>], out: &mut Vec<String>) {
    for command in commands {
        if command.subcommands.is_empty() {
            out.push(command.qualified_name.clone());
        } else {
            command_names(&command.subcommands, out);
        }
    }
}

fn all_command_names(ctx: Context<'_>) -> Vec<String> {
    let mut names = Vec::new();
    command_names(&ctx.framework().options().commands, &mut names);
    names.sort();
    names
}

async fn autocomplete_command(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let partial = partial.to_lowercase();
    all_command_names(ctx).into_iter().filter(|name| name.contains(&partial)).take(25).collect()
}

fn format_secs(secs: u64) -> String {
    match secs {
        0 => "none".to_string(),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// Manage command cooldowns for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR",
    subcommands("cooldown_set", "cooldown_list")
)]
pub async fn cooldown(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set how many seconds members must wait between uses of a command (0 removes it)
#[poise::command(slash_command, prefix_command, guild_only, ephemeral, rename = "set", required_permissions = "ADMINISTRATOR")]
async fn cooldown_set(
    ctx: Context<'_>,
    #[description = "Command, e.g. \"ping\" or \"eco work\""]
    #[autocomplete = "autocomplete_command"]
    command: String,
    #[description = "Cooldown in seconds (0 for none)"]
    #[max = 604800]
    seconds: u64,
) -> Result<(), Error> {
    let command = command.trim().to_lowercase();
    if !all_command_names(ctx).contains(&command) {
        ctx.say(format!("❌ Unknown command '{command}'.")).await?;
        return Ok(());
    }
    let seconds = seconds.min(MAX_COOLDOWN_SECS);
    ctx.data().cooldowns.lock().unwrap().set(ctx.guild_id().unwrap().get(), &command, seconds)?;
    ctx.say(format!("✅ Cooldown for `/{command}` set to {}.", format_secs(seconds))).await?;
    Ok(())
}

/// List the commands that have a cooldown on this server
#[poise::command(slash_command, prefix_command, guild_only, ephemeral, rename = "list", required_permissions = "ADMINISTRATOR")]
async fn cooldown_list(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let lines: Vec<String> = {
        let cooldowns = ctx.data().cooldowns.lock().unwrap();
        all_command_names(ctx)
            .into_iter()
            .filter_map(|name| {
                let secs = cooldowns.seconds(guild_id, &name);
                (secs > 0).then(|| format!("`/{name}` — {}", format_secs(secs)))
            })
            .collect()
    };
    let description = if lines.is_empty() { "No commands have a cooldown.".to_string() } else { lines.join("\n") };
    let embed = serenity::CreateEmbed::new().title("Command Cooldowns").description(description).colour(0x3498db);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        assert_eq!(c.seconds(1, "ping"), 300);
        assert_eq!(c.seconds(1, "eco work"), 0);
        c.set(1, "ping", 0).unwrap();
        c.set(1, "eco work", 10).unwrap();
        assert_eq!(c.seconds(1, "ping"), 0);
        assert_eq!(c.seconds(1, "eco work"), 10);
        assert_eq!(c.seconds(2, "ping"), 300);
    }

    #[test]
    fn cooldown_is_per_member_server_and_command() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        c.set(1, "rank", 10).unwrap();
        assert_eq!(c.start(1, 5, "rank", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(1, 5, "rank", 1004).unwrap(), Err(1010));
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Ok(()));
        assert_eq!(c.start(2, 5, "hack", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(2, 5, "hack", 1299).unwrap(), Err(1300));
        assert_eq!(c.start(1, 5, "eco work", 1004).unwrap(), Ok(()));
        assert_eq!(c.start(1, 5, "rank", 1010).unwrap(), Ok(()));
    }

    #[test]
    fn cooldown_survives_reopening() {
        let path = std::env::temp_dir().join(format!("cooldowns-test-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        Cooldowns::open_path(path).unwrap().start(1, 10, "hack", 1000).unwrap().unwrap();
        assert_eq!(Cooldowns::open_path(path).unwrap().start(1, 10, "hack", 1100).unwrap(), Err(1300));
        std::fs::remove_file(path).ok();
    }
}
