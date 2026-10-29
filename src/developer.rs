//! Commands only the bot's owner can use: Discord tells the bot who owns it (or who's on the
//! team that does) when it starts. They belong to no cog, so they can't be disabled and work
//! in any channel. Also maintenance mode, when only the owner can use any command.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection};

use crate::cogs::{now_iso, truncate, Context, Data, Error};
use crate::log;

/// The category these commands are filed under
pub const CATEGORY: &str = "Developer";
/// Exists while the bot is in maintenance mode, so it stays on across restarts
const MAINTENANCE_FILE: &str = "database/maintenance";

static MAINTENANCE: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(std::path::Path::new(MAINTENANCE_FILE).exists()));

/// Whether the bot is in maintenance mode, when only its owner can use commands.
pub fn maintenance() -> bool {
    MAINTENANCE.load(Ordering::Relaxed)
}

const BLACKLIST_DB: &str = "database/blacklist.db";

/// What a blacklist entry is: a server the bot won't stay in, or a user whose servers it won't
/// stay in and who can't use its commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlacklistKind {
    Server,
    User,
}

impl BlacklistKind {
    fn key(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::User => "user",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "server" => Some(Self::Server),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// A blacklisted server or user: (kind, ID, name when it was added).
pub type BlacklistEntry = (BlacklistKind, u64, String);

pub struct Blacklist {
    conn: Connection,
}

impl Blacklist {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(BLACKLIST_DB)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blacklist (
                -- 'server' or 'user'
                kind TEXT NOT NULL,
                id INTEGER NOT NULL,
                name TEXT NOT NULL,
                added TEXT NOT NULL,
                PRIMARY KEY (kind, id)
            )",
        )?;
        Ok(Self { conn })
    }

    pub fn add(&self, kind: BlacklistKind, id: u64, name: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO blacklist (kind, id, name, added) VALUES (?, ?, ?, ?)",
            params![kind.key(), id, name, now_iso()],
        )?;
        Ok(())
    }

    /// Returns false if it wasn't blacklisted.
    pub fn remove(&self, kind: BlacklistKind, id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM blacklist WHERE kind = ? AND id = ?", params![kind.key(), id])? > 0)
    }

    pub fn contains(&self, kind: BlacklistKind, id: u64) -> rusqlite::Result<bool> {
        self.conn.query_row("SELECT COUNT(*) > 0 FROM blacklist WHERE kind = ? AND id = ?", params![kind.key(), id], |row| row.get(0))
    }

    pub fn list(&self) -> rusqlite::Result<Vec<BlacklistEntry>> {
        let mut stmt = self.conn.prepare("SELECT kind, id, name FROM blacklist ORDER BY added DESC")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.filter_map(Result::ok).filter_map(|(kind, id, name)| Some((BlacklistKind::from_key(&kind)?, id, name))).collect())
    }
}

/// Whether a user is blacklisted from using the bot's commands.
pub fn is_user_blacklisted(data: &Data, user_id: serenity::UserId) -> bool {
    data.blacklist.lock().unwrap().contains(BlacklistKind::User, user_id.get()).unwrap_or(false)
}

/// Leaves a server that's blacklisted, or owned by a blacklisted user. Run whenever the bot
/// joins or reconnects to a server, so it also leaves them on startup.
pub async fn enforce_blacklist(ctx: &serenity::Context, data: &Data, guild: &serenity::Guild) {
    let reason = {
        let blacklist = data.blacklist.lock().unwrap();
        if blacklist.contains(BlacklistKind::Server, guild.id.get()).unwrap_or(false) {
            Some("the server is blacklisted")
        } else if blacklist.contains(BlacklistKind::User, guild.owner_id.get()).unwrap_or(false) {
            Some("its owner is blacklisted")
        } else {
            None
        }
    };
    let Some(reason) = reason else { return };
    log::warning(format!("Leaving {} (ID: {}) as {reason}", guild.name, guild.id));
    if let Err(e) = guild.id.leave(ctx).await {
        log::error(format!("Error leaving blacklisted guild {}: {e}", guild.id));
    }
}

/// What `/blacklist add` blacklists
#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum BlacklistType {
    Server,
    #[name = "Server owner"]
    Owner,
}

/// Whether leaving a server also blacklists it or its owner
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlacklistChoice {
    No,
    Server,
    Owner,
}

/// How long the confirm buttons wait
const CONFIRM_TIME: std::time::Duration = std::time::Duration::from_secs(60);

/// Suggests the servers the bot is in, by name, with their ID as the value.
async fn autocomplete_server(ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    let mut servers: Vec<(String, serenity::GuildId)> = ctx
        .cache()
        .guilds()
        .into_iter()
        .filter_map(|id| ctx.cache().guild(id).map(|g| (g.name.clone(), id)))
        .filter(|(name, id)| name.to_lowercase().contains(&partial) || id.to_string().contains(&partial))
        .collect();
    servers.sort();
    servers
        .into_iter()
        .take(25)
        .map(|(name, id)| serenity::AutocompleteChoice::new(truncate(&format!("{name} ({id})"), 100), id.to_string()))
        .collect()
}

/// Make the bot leave a server (bot owner only)
#[poise::command(
    slash_command,
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn leave(
    ctx: Context<'_>,
    #[description = "The server to leave"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    leave_server(ctx, server, BlacklistChoice::No).await
}

/// Manage the servers and users the bot won't serve (bot owner only)
#[poise::command(
    slash_command,
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR",
    subcommands("blacklist_add", "blacklist_remove", "blacklist_list"),
    subcommand_required
)]
pub async fn blacklist(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Leave a server and blacklist it or its owner
#[poise::command(slash_command, rename = "add", owners_only, ephemeral)]
async fn blacklist_add(
    ctx: Context<'_>,
    #[description = "The server to leave"]
    #[autocomplete = "autocomplete_server"]
    server: String,
    #[description = "Server: leave it if re-added. Owner: leave all their servers and block their commands"]
    #[rename = "type"]
    kind: BlacklistType,
) -> Result<(), Error> {
    let choice = match kind {
        BlacklistType::Server => BlacklistChoice::Server,
        BlacklistType::Owner => BlacklistChoice::Owner,
    };
    leave_server(ctx, server, choice).await
}

/// Leaves a server after a confirm button, blacklisting it or its owner first if asked.
async fn leave_server(ctx: Context<'_>, server: String, blacklist: BlacklistChoice) -> Result<(), Error> {
    let name = server
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .map(serenity::GuildId::new)
        .and_then(|id| ctx.cache().guild(id).map(|g| (id, g.name.clone(), g.owner_id)));
    let Some((guild_id, name, owner_id)) = name else {
        ctx.say("❌ I'm not in that server. Pick one from the list.").await?;
        return Ok(());
    };

    let (yes_id, no_id) = (format!("{}remove_yes", ctx.id()), format!("{}remove_no", ctx.id()));
    let buttons = |enabled: bool| {
        vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&yes_id).label("Leave").style(serenity::ButtonStyle::Danger).disabled(!enabled),
            serenity::CreateButton::new(&no_id).label("Cancel").style(serenity::ButtonStyle::Secondary).disabled(!enabled),
        ])]
    };
    let embed = |colour: u32, text: String| serenity::CreateEmbed::new().colour(colour).description(text);
    // Servers owned by a bot owner are never left or blacklisted, and bot owners can't be locked out
    if ctx.framework().options().owners.contains(&owner_id) {
        match blacklist {
            BlacklistChoice::Owner => {
                ctx.say("❌ That server's owner is a bot owner, so they can't be blacklisted.").await?;
                return Ok(());
            }
            BlacklistChoice::Server => {
                ctx.say("❌ That server is owned by a bot owner, so it can't be blacklisted.").await?;
                return Ok(());
            }
            BlacklistChoice::No => {
                ctx.say("❌ That server is owned by a bot owner, so I won't leave it.").await?;
                return Ok(());
            }
        }
    }
    let owner_name = match blacklist {
        BlacklistChoice::Owner => owner_id.to_user(ctx).await.map_or_else(|_| owner_id.to_string(), |u| u.tag()),
        _ => String::new(),
    };
    let blacklist_text = match blacklist {
        BlacklistChoice::No => String::new(),
        BlacklistChoice::Server => "\nIt will be **blacklisted**, so I'll leave straight away if I'm added again.".to_string(),
        BlacklistChoice::Owner => format!(
            "\nIts owner **{owner_name}** (`{owner_id}`) will be **blacklisted**: I'll leave any server they own and they can't use my commands."
        ),
    };
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed(0xed4245, format!("Are you sure you want me to leave **{name}** (`{guild_id}`)?{blacklist_text}")))
                .components(buttons(true)),
        )
        .await?;

    let (yes, no) = (yes_id.clone(), no_id.clone());
    let author = ctx.author().id;
    let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .filter(move |i| (i.data.custom_id == yes || i.data.custom_id == no) && i.user.id == author)
        .timeout(CONFIRM_TIME)
        .await;
    if let Some(press) = &press {
        press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
    }

    let confirmed = press.is_some_and(|press| press.data.custom_id == yes_id);
    // Blacklisted first, so it holds even if leaving fails
    let blacklisted = match blacklist {
        _ if !confirmed => Ok(String::new()),
        BlacklistChoice::No => Ok(String::new()),
        BlacklistChoice::Server => ctx
            .data()
            .blacklist
            .lock()
            .unwrap()
            .add(BlacklistKind::Server, guild_id.get(), &name)
            .map(|_| " The server has been blacklisted.".to_string()),
        BlacklistChoice::Owner => ctx
            .data()
            .blacklist
            .lock()
            .unwrap()
            .add(BlacklistKind::User, owner_id.get(), &owner_name)
            .map(|_| format!(" Its owner **{owner_name}** has been blacklisted.")),
    };
    let blacklisted = blacklisted.unwrap_or_else(|e| format!(" ❌ Couldn't blacklist: {e}"));
    let result = match confirmed {
        true => match guild_id.leave(ctx).await {
            Ok(()) => embed(0x2ecc71, format!("✅ Left **{name}** (`{guild_id}`).{blacklisted}")),
            Err(e) => embed(0xed4245, format!("❌ Couldn't leave **{name}**: {e}.{blacklisted}")),
        },
        _ => embed(0x95a5a6, format!("Cancelled. I'm still in **{name}** (`{guild_id}`).")),
    };
    // Already gone if it was in the server being left
    let _ = reply.edit(ctx, poise::CreateReply::default().embed(result).components(buttons(false))).await;
    Ok(())
}

/// Suggests the blacklist's entries, with "server:<id>" or "user:<id>" as the value.
async fn autocomplete_blacklisted(ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    let entries = ctx.data().blacklist.lock().unwrap().list().unwrap_or_default();
    entries
        .into_iter()
        .map(|(kind, id, name)| {
            let label = match kind {
                BlacklistKind::Server => format!("Server: {name} ({id})"),
                BlacklistKind::User => format!("User: {name} ({id})"),
            };
            (label, format!("{}:{id}", kind.key()))
        })
        .filter(|(label, _)| label.to_lowercase().contains(&partial))
        .take(25)
        .map(|(label, value)| serenity::AutocompleteChoice::new(truncate(&label, 100), value))
        .collect()
}

/// Take a server or user off the blacklist
#[poise::command(slash_command, rename = "remove", owners_only, ephemeral)]
async fn blacklist_remove(
    ctx: Context<'_>,
    #[description = "The server or user to take off the blacklist"]
    #[autocomplete = "autocomplete_blacklisted"]
    entry: String,
) -> Result<(), Error> {
    let parsed = entry.split_once(':').and_then(|(kind, id)| Some((BlacklistKind::from_key(kind)?, id.parse::<u64>().ok()?)));
    let Some((kind, id)) = parsed else {
        ctx.say("❌ Pick an entry from the list.").await?;
        return Ok(());
    };
    if ctx.data().blacklist.lock().unwrap().remove(kind, id)? {
        ctx.say(format!("✅ `{id}` has been taken off the blacklist.")).await?;
    } else {
        ctx.say(format!("❌ `{id}` isn't blacklisted.")).await?;
    }
    Ok(())
}

/// List the blacklisted servers and users
#[poise::command(slash_command, rename = "list", owners_only, ephemeral)]
async fn blacklist_list(ctx: Context<'_>) -> Result<(), Error> {
    let entries = ctx.data().blacklist.lock().unwrap().list()?;
    if entries.is_empty() {
        ctx.say("Nothing is blacklisted.").await?;
        return Ok(());
    }
    let lines: Vec<String> = entries
        .iter()
        .map(|(kind, id, name)| match kind {
            BlacklistKind::Server => format!("🏠 **{name}** (`{id}`)"),
            BlacklistKind::User => format!("👤 **{name}** (`{id}`) <@{id}>"),
        })
        .collect();
    let pages = crate::pagination::chunk_lines(&lines, 15)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().colour(0xed4245).title(format!("Blacklist [{}]", entries.len())).description(text))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

/// Turn maintenance mode on or off: while it's on, only the bot owner can use commands (bot owner only)
#[poise::command(
    slash_command,
    rename = "maintenance",
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn maintenance_mode(ctx: Context<'_>) -> Result<(), Error> {
    let on = !maintenance();
    if on {
        std::fs::write(MAINTENANCE_FILE, "")?;
    } else if let Err(e) = std::fs::remove_file(MAINTENANCE_FILE) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    MAINTENANCE.store(on, Ordering::Relaxed);
    crate::presence::apply(ctx.serenity_context());

    let (title, text) = if on {
        ("Maintenance mode **enabled**", "👷 The bot is in maintenance mode. Only you can use its commands until you run `/maintenance` again.")
    } else {
        ("Maintenance mode **disabled**", "👷 The bot is out of maintenance mode, and everyone can use its commands again.")
    };
    let embed = serenity::CreateEmbed::new().colour(0x2ecc71).title(title).description(text).timestamp(serenity::Timestamp::now());
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_entries_are_added_and_removed() {
        let b = Blacklist::open_path(":memory:").unwrap();
        assert!(!b.contains(BlacklistKind::Server, 1).unwrap());
        b.add(BlacklistKind::Server, 1, "Spam Server").unwrap();
        b.add(BlacklistKind::User, 2, "spammer").unwrap();
        assert!(b.contains(BlacklistKind::Server, 1).unwrap());
        // The same ID as a different kind isn't blacklisted
        assert!(!b.contains(BlacklistKind::User, 1).unwrap());
        assert_eq!(b.list().unwrap().len(), 2);
        assert!(b.remove(BlacklistKind::Server, 1).unwrap());
        assert!(!b.remove(BlacklistKind::Server, 1).unwrap());
        assert_eq!(b.list().unwrap(), vec![(BlacklistKind::User, 2, "spammer".to_string())]);
    }
}
