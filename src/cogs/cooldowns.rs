//! Per-guild command cooldowns. Admins set how many seconds must pass between uses of a
//! command, either for each member or for the whole server at once; commands without a
//! setting fall back to `DEFAULTS` (per member), or no cooldown at all.

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
    ("suggest", 300),
    ("hack", games::HACK_COOLDOWN_SECS as u64),
    ("heist", games::heist::HEIST_COOLDOWN_SECS as u64),
    ("fight", games::fight::FIGHT_COOLDOWN_SECS as u64),
    // Guesses are messages, so this is started by the guessing game itself
    ("guessing", games::guessing::GUESS_COOLDOWN_SECS as u64),
];

/// Things with a cooldown that aren't commands, so `/cooldown set` accepts them too
const NON_COMMANDS: &[&str] = &["guessing"];

/// The `last_used` row a server-wide cooldown is kept under
const EVERYONE: u64 = 0;

/// Who a cooldown applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, poise::ChoiceParameter)]
pub enum Scope {
    /// Each member waits on their own
    #[default]
    User,
    /// One use makes the whole server wait
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Setting {
    seconds: u64,
    scope: Scope,
}

/// Commands that start their own cooldown (after checking their input), so the
/// framework-wide check leaves them alone. `heist` cools down for the whole server
/// once a heist ends, and `fight` for both fighters once a fight ends.
const SELF_MANAGED: &[&str] = &["hack", "heist", "fight"];

/// Configured cooldowns, cached in memory, and when each member last used each command,
/// both persisted to SQLite so running cooldowns survive restarts.
pub struct Cooldowns {
    conn: Connection,
    config: HashMap<(u64, String), Setting>,
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
                -- One cooldown for the whole server instead of one per member
                global BOOLEAN NOT NULL DEFAULT 0,
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
        // Databases created before cooldowns could be server-wide
        let has_global: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('command_cooldowns') WHERE name = 'global'",
            [],
            |row| row.get(0),
        )?;
        if !has_global {
            conn.execute_batch("ALTER TABLE command_cooldowns ADD COLUMN global BOOLEAN NOT NULL DEFAULT 0")?;
        }
        let config = {
            let mut stmt = conn.prepare("SELECT guild_id, command, seconds, global FROM command_cooldowns")?;
            let rows = stmt.query_map([], |row| {
                let scope = if row.get(3)? { Scope::Global } else { Scope::User };
                Ok(((row.get(0)?, row.get(1)?), Setting { seconds: row.get(2)?, scope }))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Self { conn, config })
    }

    /// Seconds between uses of `command`, by its qualified name (e.g. "eco work").
    pub fn seconds(&self, guild_id: u64, command: &str) -> u64 {
        self.setting(guild_id, command).seconds
    }

    pub fn scope(&self, guild_id: u64, command: &str) -> Scope {
        self.setting(guild_id, command).scope
    }

    fn setting(&self, guild_id: u64, command: &str) -> Setting {
        self.config
            .get(&(guild_id, command.to_string()))
            .copied()
            .unwrap_or_else(|| Setting { seconds: default_for(command), scope: Scope::User })
    }

    pub fn set(&mut self, guild_id: u64, command: &str, seconds: u64, scope: Scope) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO command_cooldowns (guild_id, command, seconds, global) VALUES (?, ?, ?, ?)",
            params![guild_id, command, seconds, scope == Scope::Global],
        )?;
        self.config.insert((guild_id, command.to_string()), Setting { seconds, scope });
        Ok(())
    }

    /// Whose last use counts: the member's, or anyone's for a server-wide cooldown.
    fn user_key(&self, guild_id: u64, user_id: u64, command: &str) -> u64 {
        if self.scope(guild_id, command) == Scope::Global { EVERYONE } else { user_id }
    }

    /// The Unix time the member can use the command again, if its cooldown is still running at `now`.
    pub fn ready_at(&self, guild_id: u64, user_id: u64, command: &str, now: i64) -> rusqlite::Result<Option<i64>> {
        let user_id = self.user_key(guild_id, user_id, command);
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
        let user_id = self.user_key(guild_id, user_id, command);
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
    let (started, scope) = {
        let cooldowns = ctx.data().cooldowns.lock().unwrap();
        let started = cooldowns.start(guild_id.get(), ctx.author().id.get(), command, chrono::Utc::now().timestamp())?;
        (started, cooldowns.scope(guild_id.get(), command))
    };
    let whom = if scope == Scope::Global { " for everyone" } else { "" };
    started.map_err(|ready| format!("⏱️ This command is on cooldown{whom}. Try again <t:{ready}:R>.").into())
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
    names.extend(NON_COMMANDS.iter().map(|name| name.to_string()));
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

/// Set how many seconds must pass between uses of a command (0 removes it)
#[poise::command(slash_command, prefix_command, guild_only, ephemeral, rename = "set", required_permissions = "ADMINISTRATOR")]
async fn cooldown_set(
    ctx: Context<'_>,
    #[description = "Command, e.g. \"ping\" or \"eco work\""]
    #[autocomplete = "autocomplete_command"]
    command: String,
    #[description = "Cooldown in seconds (0 for none)"]
    #[max = 604800]
    seconds: u64,
    #[description = "User: each member waits on their own (default). Global: one use makes everyone wait"] scope: Option<Scope>,
) -> Result<(), Error> {
    let scope = scope.unwrap_or_default();
    let command = command.trim().to_lowercase();
    if !all_command_names(ctx).contains(&command) {
        ctx.say(format!("❌ Unknown command '{command}'.")).await?;
        return Ok(());
    }
    let seconds = seconds.min(MAX_COOLDOWN_SECS);
    ctx.data().cooldowns.lock().unwrap().set(ctx.guild_id().unwrap().get(), &command, seconds, scope)?;
    let whom = if scope == Scope::Global { "the whole server" } else { "each member" };
    ctx.say(format!("✅ Cooldown for `/{command}` set to {} for {whom}.", format_secs(seconds))).await?;
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
                let scope = if cooldowns.scope(guild_id, &name) == Scope::Global { "global" } else { "per user" };
                (secs > 0).then(|| format!("`/{name}` — {} ({scope})", format_secs(secs)))
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
        c.set(1, "ping", 0, Scope::User).unwrap();
        c.set(1, "eco work", 10, Scope::User).unwrap();
        assert_eq!(c.seconds(1, "ping"), 0);
        assert_eq!(c.seconds(1, "eco work"), 10);
        assert_eq!(c.seconds(2, "ping"), 300);
    }

    #[test]
    fn cooldown_is_per_member_server_and_command() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        c.set(1, "rank", 10, Scope::User).unwrap();
        assert_eq!(c.start(1, 5, "rank", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(1, 5, "rank", 1004).unwrap(), Err(1010));
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Ok(()));
        assert_eq!(c.start(2, 5, "hack", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(2, 5, "hack", 1299).unwrap(), Err(1300));
        assert_eq!(c.start(1, 5, "eco work", 1004).unwrap(), Ok(()));
        assert_eq!(c.start(1, 5, "rank", 1010).unwrap(), Ok(()));
    }

    #[test]
    fn global_cooldowns_are_shared() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        c.set(1, "rank", 10, Scope::Global).unwrap();
        assert_eq!(c.start(1, 5, "rank", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Err(1010));
        assert_eq!(c.ready_at(1, 7, "rank", 1004).unwrap(), Some(1010));
        // Other servers have their own
        assert_eq!(c.start(2, 6, "rank", 1004).unwrap(), Ok(()));
        // Back to per member
        c.set(1, "rank", 10, Scope::User).unwrap();
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Ok(()));
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
