//! Incidents cog: evidence collection. Whenever Discord's audit log records a ban, kick or
//! timeout (from the bot's commands, the honeypot, or staff doing it by hand), an incident
//! report is saved with the member's account details and every message of theirs the bot
//! had seen. Staff read reports with `/incident list`, `/incident view` and `/incident export`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::moderation::{staff_policy, suspicious_accounts};
use super::{format_time, now_iso, parse_iso, truncate, Context, Data, Error};
use crate::log;
use crate::shared::pagination::{chunk_lines, field_pages, paginate};

pub use crate::database::repositories::incidents::*;

pub const COG_NAME: &str = "incidents";

const COLOUR: u32 = 0xe67e22;
/// The most messages one report keeps (the newest)
const MAX_MESSAGES: usize = 200;
/// Longest message text shown in /incident view; /incident export has it all
const MAX_SHOWN: usize = 900;
/// The actions that open an incident
const ACTIONS: &[&str] = &["banned", "kicked", "timed out"];
/// How long a member who left is remembered, as a kick or ban's audit log entry can come in
/// after they've left the cache
const REMEMBER_LEFT: Duration = Duration::from_secs(5 * 60);

/// Members who just left, by (server, user), for their join date and roles.
static RECENTLY_LEFT: LazyLock<Mutex<HashMap<(u64, u64), (serenity::Member, Instant)>>> = LazyLock::new(Default::default);

/// Remembers a member who left for a few minutes, in case it was a kick or ban.
pub fn on_member_leave(guild_id: serenity::GuildId, member: Option<&serenity::Member>) {
    let mut left = RECENTLY_LEFT.lock().unwrap();
    left.retain(|_, (_, at)| at.elapsed() < REMEMBER_LEFT);
    if let Some(member) = member {
        left.insert((guild_id.get(), member.user.id.get()), (member.clone(), Instant::now()));
    }
}

/// Every message by `user_id` in the server's cached channels and threads, oldest first.
fn collect_messages(ctx: &serenity::Context, guild_id: serenity::GuildId, user_id: serenity::UserId) -> Vec<EvidenceMessage> {
    let channel_ids: Vec<serenity::ChannelId> = match ctx.cache.guild(guild_id) {
        Some(guild) => guild.channels.keys().copied().chain(guild.threads.iter().map(|t| t.id)).collect(),
        None => return vec![],
    };
    let mut found: Vec<EvidenceMessage> = channel_ids
        .into_iter()
        .filter_map(|channel_id| ctx.cache.channel_messages(channel_id).map(|messages| {
            messages
                .values()
                .filter(|m| m.author.id == user_id)
                .map(|m| EvidenceMessage {
                    message_id: m.id.get(),
                    channel_id: m.channel_id.get(),
                    content: m.content.clone(),
                    attachments: m.attachments.iter().map(|a| a.url.clone()).collect(),
                    sent_at: m.timestamp.to_rfc3339().unwrap_or_default(),
                })
                .collect::<Vec<_>>()
        }))
        .flatten()
        .collect();
    found.sort_by_key(|m| m.message_id);
    let extra = found.len().saturating_sub(MAX_MESSAGES);
    found.drain(..extra);
    found
}

/// Opens an incident when the audit log records a ban, kick or timeout.
pub async fn on_audit_entry(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, entry: &serenity::AuditLogEntry) {
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let Some(action) = staff_policy::native_action(entry).filter(|a| ACTIONS.contains(a)) else { return };
    let Some(user_id) = entry.target_id.map(|id| serenity::UserId::new(id.get())) else { return };
    let user = match user_id.to_user(ctx).await {
        Ok(user) => user,
        Err(e) => return log::error(format!("Couldn't read user {user_id} for an incident in guild {guild_id}: {e}")),
    };
    // Still in the server (a timeout), or just left (a kick or ban)
    let member = guild_id
        .to_guild_cached(&ctx.cache)
        .and_then(|guild| guild.members.get(&user_id).cloned())
        .or_else(|| RECENTLY_LEFT.lock().unwrap().remove(&(guild_id.get(), user_id.get())).map(|(member, _)| member));
    let display_name = member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| user.global_name.clone())
        .filter(|name| *name != user.name);
    let incident = Incident {
        id: 0,
        guild_id: guild_id.get(),
        user_id: user_id.get(),
        action: action.to_string(),
        reason: entry.reason.clone().filter(|r| !r.trim().is_empty()),
        moderator_id: entry.user_id.get(),
        created_at: now_iso(),
        username: user.name.clone(),
        display_name,
        joined_at: member.as_ref().and_then(|m| m.joined_at).and_then(|t| t.to_rfc3339()),
        roles: member.as_ref().map(|m| m.roles.iter().map(|r| r.get()).collect()).unwrap_or_default(),
    };
    let messages = collect_messages(ctx, guild_id, user_id);
    match data.incidents.lock().unwrap().create(&incident, &messages) {
        Ok(id) => log::info(format!("Opened incident #{id} in guild {guild_id}: {} was {action} ({} messages)", user.name, messages.len())),
        Err(e) => log::error(format!("Error saving an incident in guild {guild_id}: {e}")),
    }
}

/// `<t:..>` for a stored timestamp, in Discord's `style`.
fn discord_time(timestamp: &str, style: char) -> String {
    parse_iso(timestamp).map_or_else(|| timestamp.to_string(), |t| format!("<t:{}:{style}>", t.timestamp()))
}

/// One line about an incident, for lists.
fn summary(incident: &Incident) -> String {
    format!(
        "`#{}` **{}** <@{}> by <@{}> {}",
        incident.id,
        incident.action,
        incident.user_id,
        incident.moderator_id,
        discord_time(&incident.created_at, 'R')
    )
}

/// The member's other incidents and honeypot catches on this server, newest first.
fn history(data: &Data, incident: &Incident) -> Result<Vec<String>, Error> {
    let others = data.incidents.lock().unwrap().list(incident.guild_id, Some(incident.user_id))?;
    let mut lines: Vec<String> = others.iter().filter(|i| i.id != incident.id).map(summary).collect();
    let catches = data.honeypot.lock().unwrap().activity_logs(incident.guild_id, Some(incident.user_id), Some(10))?;
    lines.extend(catches.iter().map(|c| format!("🍯 Honeypot: **{}** {}", c.action.replace('_', " "), discord_time(&c.timestamp, 'R'))));
    Ok(lines)
}

/// A channel or thread's name from the cache, or its ID if it's gone.
fn channel_name(ctx: Context<'_>, channel_id: u64) -> String {
    let id = serenity::ChannelId::new(channel_id);
    ctx.guild()
        .and_then(|g| g.channels.get(&id).map(|c| c.name.clone()).or_else(|| g.threads.iter().find(|t| t.id == id).map(|t| t.name.clone())))
        .unwrap_or_else(|| channel_id.to_string())
}

fn account_created(user_id: u64) -> i64 {
    serenity::UserId::new(user_id).created_at().unix_timestamp()
}

/// Shows incidents. Staff only, as reports hold members' messages.
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "Incidents",
    required_permissions = "MODERATE_MEMBERS",
    default_member_permissions = "MODERATE_MEMBERS",
    subcommands("list", "view", "export"),
    subcommand_required
)]
pub async fn incident(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// List incident reports, newest first
#[command(slash_command, guild_only, ephemeral, required_permissions = "MODERATE_MEMBERS")]
async fn list(ctx: Context<'_>, #[description = "Only this member's incidents"] user: Option<serenity::User>) -> Result<(), Error> {
    let incidents = ctx.data().incidents.lock().unwrap().list(ctx.guild_id().unwrap().get(), user.as_ref().map(|u| u.id.get()))?;
    if incidents.is_empty() {
        let text = match &user {
            Some(user) => format!("<@{}> has no incidents on this server.", user.id),
            None => "This server has no incidents yet. They're opened whenever someone is banned, kicked or timed out.".to_string(),
        };
        ctx.send(CreateReply::default().embed(serenity::CreateEmbed::new().colour(COLOUR).description(text))).await?;
        return Ok(());
    }
    let lines: Vec<String> = incidents.iter().map(summary).collect();
    let title = format!("Incidents ({})", incidents.len());
    let pages = chunk_lines(&lines, 10)
        .into_iter()
        .map(|page| serenity::CreateEmbed::new().title(&title).colour(COLOUR).description(format!("{page}\n\nSee one with `/incident view`.")))
        .collect();
    paginate(ctx, pages).await
}

/// Read an incident report: the action, the member's account, their history and messages
#[command(slash_command, guild_only, ephemeral, required_permissions = "MODERATE_MEMBERS")]
async fn view(ctx: Context<'_>, #[description = "The incident's number"] #[min = 1] id: i64) -> Result<(), Error> {
    let Some(incident) = ctx.data().incidents.lock().unwrap().get(ctx.guild_id().unwrap().get(), id)? else {
        ctx.say(format!("❌ There's no incident #{id} on this server.")).await?;
        return Ok(());
    };
    let messages = ctx.data().incidents.lock().unwrap().messages(id)?;
    let history = history(ctx.data(), &incident)?;
    let guild = serenity::GuildId::new(incident.guild_id);
    let suspicion = suspicious_accounts::score_for(ctx.serenity_context(), ctx.data(), guild, serenity::UserId::new(incident.user_id)).await;

    let name = match &incident.display_name {
        Some(display) => format!("{display} (@{})", incident.username),
        None => format!("@{}", incident.username),
    };
    let created = account_created(incident.user_id);
    let roles = if incident.roles.is_empty() { "None recorded".to_string() } else { incident.roles.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" ") };
    let history_text = if history.is_empty() { "No other incidents.".to_string() } else { truncate(&history.join("\n"), 1000) };
    let title = format!("Incident #{id}: {}", incident.action);
    let make_page = || serenity::CreateEmbed::new().title(&title).colour(COLOUR);

    let mut summary = make_page()
        .field("Member", format!("<@{}> {name}\nID `{}`", incident.user_id, incident.user_id), true)
        .field("By", format!("<@{}>", incident.moderator_id), true)
        .field("When", discord_time(&incident.created_at, 'f'), true)
        .field("Reason", incident.reason.as_deref().unwrap_or("No reason given"), false)
        .field("Account created", format!("<t:{created}:f> (<t:{created}:R>)"), true)
        .field("Joined", incident.joined_at.as_deref().map_or_else(|| "Not known".to_string(), |t| format!("{} ({})", discord_time(t, 'f'), discord_time(t, 'R'))), true)
        .field("Roles", truncate(&roles, 1000), false)
        .field("History", history_text, false)
        .field("Messages", format!("{} saved; see the next pages, or `/incident export` for all of them in full.", messages.len()), false);
    // Worked out now, so it reflects everything known about them since
    if let Some(score) = &suspicion {
        let reasons = truncate(&suspicious_accounts::reasons_text(score), 900);
        summary = summary.field("Suspicion score (now)", format!("{}\n{reasons}", suspicious_accounts::label(score)), false);
    }
    let mut pages = vec![summary];

    let guild_id = incident.guild_id;
    let fields = messages
        .iter()
        .map(|m| {
            let channel = channel_name(ctx, m.channel_id);
            let when = parse_iso(&m.sent_at).map_or_else(|| m.sent_at.clone(), |t| t.format("%Y-%m-%d %H:%M UTC").to_string());
            let mut text = if m.content.chars().count() > MAX_SHOWN { format!("{}…", truncate(&m.content, MAX_SHOWN)) } else { m.content.clone() };
            if text.is_empty() {
                text = "*(no text)*".to_string();
            }
            for url in &m.attachments {
                text.push_str(&format!("\n📎 {url}"));
            }
            text.push_str(&format!("\n[Jump](https://discord.com/channels/{guild_id}/{}/{})", m.channel_id, m.message_id));
            (format!("#{channel} · {when}"), truncate(&text, 1024))
        })
        .collect();
    pages.extend(field_pages(make_page, fields, 4, false));
    paginate(ctx, pages).await
}

/// Download an incident report in full, as a text file
#[command(slash_command, guild_only, ephemeral, required_permissions = "MODERATE_MEMBERS")]
async fn export(ctx: Context<'_>, #[description = "The incident's number"] #[min = 1] id: i64) -> Result<(), Error> {
    let Some(incident) = ctx.data().incidents.lock().unwrap().get(ctx.guild_id().unwrap().get(), id)? else {
        ctx.say(format!("❌ There's no incident #{id} on this server.")).await?;
        return Ok(());
    };
    let messages = ctx.data().incidents.lock().unwrap().messages(id)?;
    let others = ctx.data().incidents.lock().unwrap().list(incident.guild_id, Some(incident.user_id))?;
    let catches = ctx.data().honeypot.lock().unwrap().activity_logs(incident.guild_id, Some(incident.user_id), None)?;
    let role_name = |id: u64| {
        ctx.guild().and_then(|g| g.roles.get(&serenity::RoleId::new(id)).map(|r| r.name.clone())).unwrap_or_else(|| id.to_string())
    };

    let mut out = format!("Incident #{id}: {} was {}\n", incident.username, incident.action);
    out += &format!("Opened: {}\n", format_time(&incident.created_at));
    out += &format!("By: {}\n", incident.moderator_id);
    out += &format!("Reason: {}\n\n", incident.reason.as_deref().unwrap_or("No reason given"));
    out += "ACCOUNT\n";
    out += &format!("Username: {}\n", incident.username);
    if let Some(display) = &incident.display_name {
        out += &format!("Display name: {display}\n");
    }
    out += &format!("ID: {}\n", incident.user_id);
    out += &format!("Created: {}\n", serenity::UserId::new(incident.user_id).created_at().format("%Y-%m-%d %H:%M:%S"));
    out += &format!("Joined: {}\n", incident.joined_at.as_deref().map_or_else(|| "Not known".to_string(), format_time));
    let roles: Vec<String> = incident.roles.iter().map(|r| role_name(*r)).collect();
    out += &format!("Roles: {}\n\n", if roles.is_empty() { "None recorded".to_string() } else { roles.join(", ") });

    let user_id = serenity::UserId::new(incident.user_id);
    if let Some(score) = suspicious_accounts::score_for(ctx.serenity_context(), ctx.data(), serenity::GuildId::new(incident.guild_id), user_id).await {
        out += &format!("SUSPICION SCORE (NOW): {}/100\n", score.total);
        for reason in &score.reasons {
            out += &format!("{:+} {}\n", reason.points, reason.text);
        }
        out += "\n";
    }
    out += "HISTORY\n";
    let mut any_history = false;
    for other in others.iter().filter(|i| i.id != id) {
        any_history = true;
        out += &format!("{}  #{} {} by {}: {}\n", format_time(&other.created_at), other.id, other.action, other.moderator_id, other.reason.as_deref().unwrap_or("No reason given"));
    }
    for catch in &catches {
        any_history = true;
        out += &format!("{}  Honeypot: {}\n", format_time(&catch.timestamp), catch.action.replace('_', " "));
    }
    if !any_history {
        out += "No other incidents.\n";
    }

    out += &format!("\nMESSAGES ({})\n", messages.len());
    for m in &messages {
        out += &format!("\n[{}] #{} (message {})\n{}\n", format_time(&m.sent_at), channel_name(ctx, m.channel_id), m.message_id, m.content);
        for url in &m.attachments {
            out += &format!("Attachment: {url}\n");
        }
    }

    let file = serenity::CreateAttachment::bytes(out.into_bytes(), format!("incident-{id}.txt"));
    ctx.send(CreateReply::default().content(format!("📄 Incident #{id} in full.")).attachment(file)).await?;
    Ok(())
}
