//! Staff Policy cog: moderation has to go through the bot. When a member kicks, bans, unbans,
//! times out or ends a timeout with Discord's own menus, they're warned by DM and their roles
//! are taken away (saved, so `/staffpolicy restore` can give them back). What they did is
//! also undone, so it has to be redone through the bot. The bot's own actions, other bots and the server
//! owner are left alone.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};
use serenity::audit_log::{Action, Change, MemberAction};

use super::{now_iso, Context, Data, Error};
use crate::log;

pub const COG_NAME: &str = "staffpolicy";
const DB_FILE: &str = "database/staff_policy.db";
const COLOUR: u32 = 0xed4245;
/// How long `/staffpolicy setup`'s menus keep working after the last change
const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Roles taken from a staff member: (user ID, role IDs, when).
pub type Removed = (u64, Vec<u64>, String);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Given to staff in place of the roles taken
    pub demoted_role: Option<u64>,
    /// Where cases are posted; the Logs cog's member log channel if not set
    pub log_channel: Option<u64>,
    /// The roles to take; every role the bot can take if empty
    pub staff_roles: Vec<u64>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS removed_roles (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                -- Comma-separated role IDs
                role_ids TEXT NOT NULL,
                removed_at TEXT NOT NULL,
                PRIMARY KEY (guild_id, user_id)
            );
            CREATE TABLE IF NOT EXISTS settings (
                guild_id INTEGER PRIMARY KEY,
                -- Given to staff in place of the roles taken
                demoted_role INTEGER
            )",
        )?;
        // Added after the table was first made
        for (column, kind) in [("log_channel", "INTEGER"), ("staff_roles", "TEXT NOT NULL DEFAULT ''")] {
            let exists: bool = conn.query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('settings') WHERE name = ?",
                [column],
                |row| row.get(0),
            )?;
            if !exists {
                conn.execute_batch(&format!("ALTER TABLE settings ADD COLUMN {column} {kind}"))?;
            }
        }
        Ok(Self { conn })
    }

    fn parse(ids: &str) -> Vec<u64> {
        ids.split(',').filter_map(|id| id.parse().ok()).collect()
    }

    /// Saves roles taken from a member, adding to any taken before and not yet restored.
    pub fn add(&self, guild_id: u64, user_id: u64, roles: &[u64]) -> rusqlite::Result<()> {
        let mut all = self.get(guild_id, user_id)?.unwrap_or_default();
        for role in roles {
            if !all.contains(role) {
                all.push(*role);
            }
        }
        let ids: Vec<String> = all.iter().map(u64::to_string).collect();
        self.conn.execute(
            "INSERT OR REPLACE INTO removed_roles (guild_id, user_id, role_ids, removed_at) VALUES (?, ?, ?, ?)",
            params![guild_id, user_id, ids.join(","), now_iso()],
        )?;
        Ok(())
    }

    pub fn get(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Vec<u64>>> {
        self.conn
            .query_row("SELECT role_ids FROM removed_roles WHERE guild_id = ? AND user_id = ?", [guild_id, user_id], |row| {
                Ok(Self::parse(&row.get::<_, String>(0)?))
            })
            .optional()
    }

    pub fn remove(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM removed_roles WHERE guild_id = ? AND user_id = ?", [guild_id, user_id])?;
        Ok(())
    }

    /// Forgets every saved record for a guild, returning how many there were.
    pub fn clear(&self, guild_id: u64) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM removed_roles WHERE guild_id = ?", [guild_id])
    }

    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<Removed>> {
        let mut stmt = self.conn.prepare("SELECT user_id, role_ids, removed_at FROM removed_roles WHERE guild_id = ? ORDER BY removed_at DESC")?;
        let rows = stmt.query_map([guild_id], |row| Ok((row.get(0)?, Self::parse(&row.get::<_, String>(1)?), row.get(2)?)))?;
        rows.collect()
    }

    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        Ok(self
            .conn
            .query_row("SELECT demoted_role, log_channel, staff_roles FROM settings WHERE guild_id = ?", [guild_id], |row| {
                Ok(Settings { demoted_role: row.get(0)?, log_channel: row.get(1)?, staff_roles: Self::parse(&row.get::<_, String>(2)?) })
            })
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_settings(&self, guild_id: u64, settings: &Settings) -> rusqlite::Result<()> {
        let roles: Vec<String> = settings.staff_roles.iter().map(u64::to_string).collect();
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, demoted_role, log_channel, staff_roles) VALUES (?, ?, ?, ?)",
            params![guild_id, settings.demoted_role, settings.log_channel, roles.join(",")],
        )?;
        Ok(())
    }

    /// Removes records for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!(
            "DELETE FROM removed_roles WHERE guild_id NOT IN ({ids}); DELETE FROM settings WHERE guild_id NOT IN ({ids})"
        ))
    }
}

/// What a native moderation entry did, like "banned", or `None` if it isn't one the policy covers.
fn native_action(entry: &serenity::AuditLogEntry) -> Option<&'static str> {
    match entry.action {
        Action::Member(MemberAction::Kick) => Some("kicked"),
        Action::Member(MemberAction::BanAdd) => Some("banned"),
        Action::Member(MemberAction::BanRemove) => Some("unbanned"),
        Action::Member(MemberAction::Update) => entry.changes.as_ref()?.iter().find_map(|change| match change {
            Change::CommunicationDisabledUntil { new, .. } => {
                Some(if new.is_some_and(|until| until > serenity::Timestamp::now()) { "timed out" } else { "ended the timeout of" })
            }
            _ => None,
        }),
        _ => None,
    }
}

fn title_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

/// Reverses a native action on `target`, returning what was done, like "the ban has been
/// lifted", or `None` if it couldn't be undone.
async fn undo(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    entry: &serenity::AuditLogEntry,
    target: serenity::UserId,
    reason: &str,
) -> Option<String> {
    let result = match entry.action {
        Action::Member(MemberAction::BanAdd) => ctx.http.remove_ban(guild_id, target, Some(reason)).await.map(|_| "the ban has been lifted".to_string()),
        Action::Member(MemberAction::BanRemove) => {
            guild_id.ban_with_reason(ctx, target, 0, reason).await.map(|_| "the member has been banned again".to_string())
        }
        // A kicked member can't be put back, so they're sent a one-use invite instead
        Action::Member(MemberAction::Kick) => {
            let channel = ctx.cache.guild(guild_id).and_then(|g| {
                g.system_channel_id.or_else(|| g.channels.values().filter(|c| c.kind == serenity::ChannelType::Text).min_by_key(|c| c.position).map(|c| c.id))
            })?;
            let invite = serenity::CreateInvite::new().max_uses(1).max_age(7 * 24 * 60 * 60).unique(true).audit_log_reason(reason);
            match channel.create_invite(ctx, invite).await {
                Ok(invite) => {
                    let guild_name = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
                    let text = format!("You were kicked from **{guild_name}** against its staff policy, so you're welcome back: {}", invite.url());
                    match target.create_dm_channel(ctx).await {
                        Ok(dm) => dm.say(ctx, text).await.map(|_| "the kicked member has been sent an invite back".to_string()),
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e),
            }
        }
        Action::Member(MemberAction::Update) => {
            let (old, new) = entry.changes.as_ref()?.iter().find_map(|change| match change {
                Change::CommunicationDisabledUntil { old, new } => Some((*old, *new)),
                _ => None,
            })?;
            let now = serenity::Timestamp::now();
            if new.is_some_and(|until| until > now) {
                let edit = serenity::EditMember::new().enable_communication().audit_log_reason(reason);
                guild_id.edit_member(ctx, target, edit).await.map(|_| "the timeout has been removed".to_string())
            } else {
                // Put the timeout back, if it hadn't run out anyway
                let until = old.filter(|until| *until > now)?;
                let edit = serenity::EditMember::new().disable_communication_until_datetime(until).audit_log_reason(reason);
                guild_id.edit_member(ctx, target, edit).await.map(|_| "the timeout has been put back".to_string())
            }
        }
        _ => return None,
    };
    match result {
        Ok(undone) => Some(undone),
        Err(e) => {
            log::error(format!("Error undoing native moderation of {target} in guild {guild_id}: {e}"));
            None
        }
    }
}

/// Punishes moderation done with Discord's own menus instead of the bot.
pub async fn on_audit_entry(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, entry: &serenity::AuditLogEntry) {
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let Some(action) = native_action(entry) else { return };
    let staff_id = entry.user_id;
    if staff_id == ctx.cache.current_user().id {
        return;
    }
    let Ok(staff) = guild_id.member(ctx, staff_id).await else { return };
    let settings = match data.staff_policy.lock().unwrap().settings(guild_id.get()) {
        Ok(settings) => settings,
        Err(e) => {
            log::error(format!("Error reading staff policy settings for guild {guild_id}: {e}"));
            Settings::default()
        }
    };
    let demoted = settings.demoted_role.map(serenity::RoleId::new);
    let Some((owner_id, guild_name, roles, demoted)) = ctx.cache.guild(guild_id).map(|guild| {
        let bot_top = guild.members.get(&ctx.cache.current_user().id).and_then(|m| guild.member_highest_role(m)).map_or(0, |r| r.position);
        let manageable = |id: &serenity::RoleId| guild.roles.get(id).is_some_and(|r| !r.managed && r.position < bot_top);
        // Roles the bot can take away: not managed by an integration, below its own, not the
        // demoted role, and one of the chosen staff roles if any are chosen
        let chosen = |id: &serenity::RoleId| settings.staff_roles.is_empty() || settings.staff_roles.contains(&id.get());
        let removable: Vec<serenity::RoleId> =
            staff.roles.iter().filter(|id| manageable(id) && chosen(id) && Some(**id) != demoted).copied().collect();
        // A demoted role that was deleted or moved above the bot's can't be given
        (guild.owner_id, guild.name.clone(), removable, demoted.filter(|id| manageable(id)))
    }) else {
        return;
    };
    // Other bots, and the owner, who can't be punished
    if staff.user.bot || staff_id == owner_id {
        return;
    }
    let target = entry.target_id.map(|id| serenity::UserId::new(id.get()));
    let target_text = target.map_or("someone".to_string(), |id| format!("<@{id}>"));
    let reason = format!("Staff policy: {} {action} {} outside the bot", staff.user.name, target.map_or("someone".to_string(), |id| id.to_string()));

    let undone = match target {
        Some(target) => undo(ctx, guild_id, entry, target, &reason).await,
        None => None,
    };

    let mut roles_removed = false;
    let gets_demoted = demoted.is_some_and(|role| !staff.roles.contains(&role));
    if !roles.is_empty() || gets_demoted {
        let mut kept: Vec<serenity::RoleId> = staff.roles.iter().filter(|id| !roles.contains(id)).copied().collect();
        kept.extend(demoted.filter(|_| gets_demoted));
        let edit = serenity::EditMember::new().roles(kept).audit_log_reason(&reason);
        match guild_id.edit_member(ctx, staff_id, edit).await {
            Ok(_) => {
                roles_removed = true;
                let ids: Vec<u64> = roles.iter().map(|r| r.get()).collect();
                if let Err(e) = data.staff_policy.lock().unwrap().add(guild_id.get(), staff_id.get(), &ids) {
                    log::error(format!("Error saving roles taken from {staff_id} in guild {guild_id}: {e}"));
                }
            }
            Err(e) => log::error(format!("Error taking roles from {staff_id} in guild {guild_id}: {e}")),
        }
    }

    let mut consequences = vec![];
    if roles_removed {
        consequences.push(match demoted {
            Some(role) => format!("your staff roles have been replaced with the <@&{role}> role until an admin restores them"),
            None => "your staff roles have been removed until an admin restores them".to_string(),
        });
    }
    if let Some(undone) = &undone {
        consequences.push(undone.clone());
    }
    let dm = format!(
        "⚠️ **Staff policy warning from {guild_name}**\n\nYou {action} a member using Discord's own menus. \
         All moderation in this server must be done through the bot's commands, so it's logged properly.{}",
        if consequences.is_empty() { String::new() } else { format!("\n\nAs a result, {}.", consequences.join(" and ")) }
    );
    let _ = staff.user.direct_message(ctx, serenity::CreateMessage::new().content(dm)).await;

    let channel = settings.log_channel.map(serenity::ChannelId::new).or_else(|| super::logs::log_channel(data, guild_id, "member"));
    if let Some(channel) = channel {
        let mut text = format!("<@{staff_id}> {action} {target_text} using Discord's own menus instead of the bot.");
        text += &format!("\n**Roles removed:** {}", if roles_removed { roles.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" ") } else { "None".into() });
        if let Some(role) = demoted.filter(|_| roles_removed) {
            text += &format!("\n**Given:** <@&{role}>");
        }
        text += &format!("\n**Undone:** {}", undone.as_deref().map_or("Nothing (it couldn't be undone)".to_string(), title_first));
        let embed = serenity::CreateEmbed::new().colour(COLOUR).title("Staff Policy Broken").description(text).timestamp(serenity::Timestamp::now());
        if let Err(e) = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await {
            log::error(format!("Error posting staff policy log in guild {guild_id}: {e}"));
        }
    }
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Manage staff who broke the moderation policy
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    rename = "staffpolicy",
    category = "StaffPolicy",
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR",
    subcommands("setup", "restore", "list", "clear"),
    subcommand_required
)]
pub async fn staff_policy(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

fn settings_embed(settings: &Settings) -> serenity::CreateEmbed {
    let roles: Vec<String> = settings.staff_roles.iter().map(|r| format!("<@&{r}>")).collect();
    serenity::CreateEmbed::new()
        .colour(COLOUR)
        .title("Staff Policy Settings")
        .field("Demoted Role", settings.demoted_role.map_or("None (roles are only taken)".to_string(), |r| format!("<@&{r}>")), false)
        .field("Log Channel", settings.log_channel.map_or("The member log channel".to_string(), |c| format!("<#{c}>")), false)
        .field(
            "Roles to Remove",
            if roles.is_empty() { "Every role the bot can remove".to_string() } else { roles.join(" ") },
            false,
        )
        .footer(serenity::CreateEmbedFooter::new("Pick the roles to remove below, or leave it empty to remove every role."))
}

/// Set the demoted role, the log channel and which roles are removed
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn setup(
    ctx: Context<'_>,
    #[description = "The role to give in place of the roles taken, like Demoted"] demoted_role: Option<serenity::Role>,
    #[description = "Where to log cases (default: the member log channel)"]
    #[channel_types("Text")]
    log_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let mut settings = ctx.data().staff_policy.lock().unwrap().settings(guild_id.get())?;

    if let Some(role) = demoted_role {
        let problem = {
            let guild = ctx.guild().ok_or("server isn't cached")?;
            let bot_top = guild.members.get(&ctx.cache().current_user().id).and_then(|m| guild.member_highest_role(m)).map_or(0, |r| r.position);
            if role.managed || role.id.get() == guild_id.get() {
                Some("❌ That role can't be given to members.")
            } else if role.position >= bot_top {
                Some("❌ That role is the same as or above my highest role, so I can't give it. Move my role above it first.")
            } else {
                None
            }
        };
        if let Some(problem) = problem {
            return say_private(ctx, problem).await;
        }
        settings.demoted_role = Some(role.id.get());
    }
    if let Some(channel) = log_channel {
        settings.log_channel = Some(channel.id.get());
    }
    ctx.data().staff_policy.lock().unwrap().save_settings(guild_id.get(), &settings)?;

    let prefix = ctx.id();
    let roles_id = format!("{prefix}sp_roles");
    let components = |settings: &Settings| {
        let defaults = settings.staff_roles.iter().map(|r| serenity::RoleId::new(*r)).collect();
        let roles = serenity::CreateSelectMenu::new(&roles_id, serenity::CreateSelectMenuKind::Role { default_roles: Some(defaults) })
            .placeholder("Roles to remove (none picked = every role)")
            .min_values(0)
            .max_values(25);
        vec![serenity::CreateActionRow::SelectMenu(roles)]
    };
    let reply = ctx.send(poise::CreateReply::default().embed(settings_embed(&settings)).components(components(&settings))).await?;

    loop {
        let id = roles_id.clone();
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .filter(move |i| i.data.custom_id == id)
            .timeout(SETUP_TIMEOUT)
            .await;
        let Some(press) = press else { break };
        let serenity::ComponentInteractionDataKind::RoleSelect { values } = &press.data.kind else { continue };
        settings.staff_roles = values.iter().map(|r| r.get()).collect();
        ctx.data().staff_policy.lock().unwrap().save_settings(guild_id.get(), &settings)?;
        let update = serenity::CreateInteractionResponseMessage::new().embed(settings_embed(&settings)).components(components(&settings));
        press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
    }

    // Already gone if someone dismissed it
    let _ = reply.edit(ctx, poise::CreateReply::default().embed(settings_embed(&settings)).components(vec![])).await;
    Ok(())
}

/// Give a staff member back the roles the policy took from them
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn restore(ctx: Context<'_>, #[description = "The staff member"] member: serenity::Member) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let Some(saved) = ctx.data().staff_policy.lock().unwrap().get(guild_id.get(), member.user.id.get())? else {
        return say_private(ctx, format!("❌ No roles were taken from <@{}>.", member.user.id)).await;
    };
    // Roles deleted since are skipped
    let roles: Vec<serenity::RoleId> = {
        let guild = ctx.guild().ok_or("server isn't cached")?;
        saved.iter().map(|id| serenity::RoleId::new(*id)).filter(|id| guild.roles.contains_key(id)).collect()
    };
    let reason = format!("Staff policy: roles restored by {}", ctx.author().name);
    let demoted = ctx.data().staff_policy.lock().unwrap().settings(guild_id.get())?.demoted_role.map(serenity::RoleId::new);
    let mut all: Vec<serenity::RoleId> = member.roles.iter().filter(|r| Some(**r) != demoted).copied().collect();
    all.extend(roles.iter().filter(|r| !all.contains(r)).copied().collect::<Vec<_>>());
    guild_id.edit_member(ctx, member.user.id, serenity::EditMember::new().roles(all).audit_log_reason(&reason)).await?;
    ctx.data().staff_policy.lock().unwrap().remove(guild_id.get(), member.user.id.get())?;
    let mentions: Vec<String> = roles.iter().map(|r| format!("<@&{r}>")).collect();
    say_private(ctx, format!("✅ Restored {} role(s) to <@{}>: {}", roles.len(), member.user.id, mentions.join(" "))).await
}

/// Clear the list of staff whose roles were taken, without giving any back
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn clear(ctx: Context<'_>) -> Result<(), Error> {
    let cleared = ctx.data().staff_policy.lock().unwrap().clear(ctx.guild_id().unwrap().get())?;
    if cleared == 0 {
        return say_private(ctx, "The list is already empty.").await;
    }
    say_private(ctx, format!("✅ Cleared {cleared} record(s). Their roles can no longer be restored with `/staffpolicy restore`.")).await
}

/// List staff whose roles the policy has taken
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let removed = ctx.data().staff_policy.lock().unwrap().list(ctx.guild_id().unwrap().get())?;
    if removed.is_empty() {
        return say_private(ctx, "Nobody has roles waiting to be restored.").await;
    }
    let lines: Vec<String> = removed
        .iter()
        .map(|(user_id, roles, when)| {
            let roles: Vec<String> = roles.iter().map(|r| format!("<@&{r}>")).collect();
            format!("<@{user_id}> ({}): {}", super::format_time(when), roles.join(" "))
        })
        .collect();
    let pages = crate::pagination::chunk_lines(&lines, 10)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().colour(COLOUR).title("Roles Taken by the Staff Policy").description(text))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_saved() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
        let settings = Settings { demoted_role: Some(5), log_channel: Some(6), staff_roles: vec![7, 8] };
        s.save_settings(1, &settings).unwrap();
        assert_eq!(s.settings(1).unwrap(), settings);
        s.save_settings(1, &Settings::default()).unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
    }

    #[test]
    fn removed_roles_are_saved_merged_and_restored() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.get(1, 2).unwrap(), None);
        s.add(1, 2, &[10, 11]).unwrap();
        s.add(1, 2, &[11, 12]).unwrap();
        assert_eq!(s.get(1, 2).unwrap(), Some(vec![10, 11, 12]));
        assert_eq!(s.list(1).unwrap().len(), 1);
        s.add(1, 3, &[10]).unwrap();
        assert_eq!(s.clear(1).unwrap(), 2);
        assert!(s.list(1).unwrap().is_empty());
        s.add(1, 2, &[10, 11, 12]).unwrap();
        s.remove(1, 2).unwrap();
        assert_eq!(s.get(1, 2).unwrap(), None);
    }
}
