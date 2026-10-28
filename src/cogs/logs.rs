//! Logs cog: posts what happens in the server to the log channels set with
//! `/server-setup logs`, one channel per type (Channel, Events, Emoji, Guild,
//! Join & Leave, Member, Message, Role, Sticker, Thread, User, Voice). A type
//! without a channel isn't logged. Where Discord's audit log records who made a
//! change (and the bot can view it), the log says who, and their reason.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use poise::serenity_prelude as serenity;
use serenity::audit_log::{ChannelAction, EmojiAction, MemberAction, MessageAction, RoleAction, ScheduledEventAction, StickerAction, ThreadAction};

use super::Data;
use crate::log;

pub const COG_NAME: &str = "logs";

const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;
const YELLOW: u32 = 0xfee75c;
const BLUE: u32 = 0x3498db;

/// Longest text quoted from a message, so the embed stays under Discord's limit
const MAX_QUOTE: usize = 1000;

/// Each server's emojis and stickers as last seen, as Discord only sends the new list
/// when they change. Filled in when the bot sees the server.
static EMOJIS: LazyLock<Mutex<HashMap<u64, HashMap<u64, String>>>> = LazyLock::new(Default::default);
static STICKERS: LazyLock<Mutex<HashMap<u64, HashMap<u64, String>>>> = LazyLock::new(Default::default);

/// How long after an event its audit log entry still counts as the one that caused it
const AUDIT_WINDOW_SECS: i64 = 20;
/// Discord writes the audit log entry just after sending the event
const AUDIT_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// The server's log channel for `kind` (a `logs.<kind>` setting), if it has one.
fn log_channel(data: &Data, guild_id: serenity::GuildId, kind: &str) -> Option<serenity::ChannelId> {
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return None;
    }
    match data.server_setup.lock().unwrap().get(guild_id.get(), &format!("logs.{kind}")) {
        Ok(channel) => channel.map(serenity::ChannelId::new),
        Err(e) => {
            log::error(format!("Error reading {kind} log channel: {e}"));
            None
        }
    }
}

/// Who did `action` to `target` just now, and their reason, from the audit log.
/// `None` if there's no recent entry (or the bot can't view the audit log).
async fn actor(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    action: serenity::audit_log::Action,
    target: u64,
) -> Option<(serenity::UserId, Option<String>)> {
    tokio::time::sleep(AUDIT_DELAY).await;
    let logs = guild_id.audit_logs(ctx, Some(action), None, None, Some(5)).await.ok()?;
    let since = serenity::Timestamp::now().unix_timestamp() - AUDIT_WINDOW_SECS;
    logs.entries
        .into_iter()
        .find(|e| e.target_id.is_some_and(|t| t.get() == target) && e.id.created_at().unix_timestamp() >= since)
        .map(|e| (e.user_id, e.reason))
}

/// Adds who did it, and why, to a log embed.
fn with_actor(embed: serenity::CreateEmbed, label: &str, actor: Option<(serenity::UserId, Option<String>)>) -> serenity::CreateEmbed {
    let Some((user, reason)) = actor else { return embed };
    let embed = embed.field(label, format!("<@{user}>"), true);
    match reason {
        Some(reason) => embed.field("Reason", super::truncate(&reason, 1000), true),
        None => embed,
    }
}

/// Posts `embed` in the server's log channel for `kind`, if it has one.
async fn send(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, kind: &str, embed: serenity::CreateEmbed) {
    if let Some(channel) = log_channel(data, guild_id, kind) {
        post(ctx, guild_id, kind, channel, embed).await;
    }
}

/// Like `send`, adding who did it from the audit log.
async fn send_by(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    kind: &str,
    embed: serenity::CreateEmbed,
    action: serenity::audit_log::Action,
    target: u64,
) {
    let Some(channel) = log_channel(data, guild_id, kind) else { return };
    let embed = with_actor(embed, "By", actor(ctx, guild_id, action, target).await);
    post(ctx, guild_id, kind, channel, embed).await;
}

async fn post(ctx: &serenity::Context, guild_id: serenity::GuildId, kind: &str, channel: serenity::ChannelId, embed: serenity::CreateEmbed) {
    let embed = embed.timestamp(serenity::Timestamp::now());
    if let Err(e) = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await {
        log::error(format!("Couldn't post a {kind} log in guild {guild_id}: {e}"));
    }
}

fn embed(colour: u32, title: &str, text: impl Into<String>) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new().colour(colour).title(title).description(text)
}

fn with_user(embed: serenity::CreateEmbed, user: &serenity::User) -> serenity::CreateEmbed {
    embed
        .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .footer(serenity::CreateEmbedFooter::new(format!("User ID: {}", user.id)))
}

/// Quoted message text, trimmed to fit.
fn quote(text: &str) -> String {
    if text.is_empty() {
        return "*no text*".to_string();
    }
    let text = if text.chars().count() > MAX_QUOTE { format!("{}…", super::truncate(text, MAX_QUOTE)) } else { text.to_string() };
    text.lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n")
}

/// "Old → New" lines for each field that changed.
fn changes(fields: &[(&str, String, String)]) -> Option<String> {
    let lines: Vec<String> =
        fields.iter().filter(|(_, old, new)| old != new).map(|(name, old, new)| format!("**{name}:** {old} → {new}")).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn or_none(text: Option<impl ToString>) -> String {
    text.map(|t| t.to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "*none*".to_string())
}

fn channel_kind(kind: serenity::ChannelType) -> &'static str {
    use serenity::ChannelType::*;
    match kind {
        Text => "Text channel",
        Voice => "Voice channel",
        Category => "Category",
        News => "Announcement channel",
        Stage => "Stage channel",
        Forum => "Forum",
        _ => "Channel",
    }
}

/// Logs the events that have a log type. Called for every gateway event.
pub async fn on_event(ctx: &serenity::Context, data: &Data, event: &serenity::FullEvent) {
    use serenity::FullEvent::*;
    match event {
        GuildCreate { guild, .. } => {
            let emojis = guild.emojis.values().map(|e| (e.id.get(), e.name.clone())).collect();
            EMOJIS.lock().unwrap().insert(guild.id.get(), emojis);
            let stickers = guild.stickers.values().map(|s| (s.id.get(), s.name.clone())).collect();
            STICKERS.lock().unwrap().insert(guild.id.get(), stickers);
        }

        // Channel
        ChannelCreate { channel } | CategoryCreate { category: channel } => {
            let text = format!("{} <#{}> (**{}**) was created.", channel_kind(channel.kind), channel.id, channel.name);
            send_by(ctx, data, channel.guild_id, "channel", embed(GREEN, "Channel Created", text), serenity::audit_log::Action::Channel(ChannelAction::Create), channel.id.get()).await;
        }
        ChannelDelete { channel, .. } | CategoryDelete { category: channel } => {
            let text = format!("{} **{}** was deleted.", channel_kind(channel.kind), channel.name);
            send_by(ctx, data, channel.guild_id, "channel", embed(RED, "Channel Deleted", text), serenity::audit_log::Action::Channel(ChannelAction::Delete), channel.id.get()).await;
        }
        ChannelUpdate { old: Some(old), new } => {
            let fields = [
                ("Name", old.name.clone(), new.name.clone()),
                ("Topic", or_none(old.topic.as_ref()), or_none(new.topic.as_ref())),
                ("NSFW", old.nsfw.to_string(), new.nsfw.to_string()),
                ("Slowmode", format!("{}s", old.rate_limit_per_user.unwrap_or(0)), format!("{}s", new.rate_limit_per_user.unwrap_or(0))),
                ("Category", or_none(old.parent_id.map(|id| format!("<#{id}>"))), or_none(new.parent_id.map(|id| format!("<#{id}>")))),
            ];
            let mut text = changes(&fields);
            if old.permission_overwrites != new.permission_overwrites {
                text = Some(format!("{}**Permissions** were changed.", text.map(|t| t + "\n").unwrap_or_default()));
            }
            if let Some(text) = text {
                send_by(ctx, data, new.guild_id, "channel", embed(YELLOW, "Channel Updated", format!("<#{}>\n{text}", new.id)), serenity::audit_log::Action::Channel(ChannelAction::Update), new.id.get()).await;
            }
        }

        // Events
        GuildScheduledEventCreate { event } => {
            let text = format!("**{}** was scheduled for <t:{}:F>.", event.name, event.start_time.unix_timestamp());
            send_by(ctx, data, event.guild_id, "events", embed(GREEN, "Event Created", text), serenity::audit_log::Action::ScheduledEvent(ScheduledEventAction::Create), event.id.get()).await;
        }
        GuildScheduledEventUpdate { event } => {
            let text = format!("**{}** was updated. Starts <t:{}:F>, status: {:?}.", event.name, event.start_time.unix_timestamp(), event.status);
            send_by(ctx, data, event.guild_id, "events", embed(YELLOW, "Event Updated", text), serenity::audit_log::Action::ScheduledEvent(ScheduledEventAction::Update), event.id.get()).await;
        }
        GuildScheduledEventDelete { event } => {
            send_by(ctx, data, event.guild_id, "events", embed(RED, "Event Deleted", format!("**{}** was cancelled.", event.name)), serenity::audit_log::Action::ScheduledEvent(ScheduledEventAction::Delete), event.id.get()).await;
        }

        // Emoji
        GuildEmojisUpdate { guild_id, current_state } => {
            let new: HashMap<u64, String> = current_state.values().map(|e| (e.id.get(), e.name.clone())).collect();
            let old = EMOJIS.lock().unwrap().insert(guild_id.get(), new.clone()).unwrap_or_default();
            for (id, title, colour, text) in diff(&old, &new, |id, name| format!("<:{name}:{id}> `:{name}:`")) {
                send_by(ctx, data, *guild_id, "emoji", embed(colour, &format!("Emoji {title}"), text), serenity::audit_log::Action::Emoji(emoji_action(title)), id).await;
            }
        }

        // Sticker
        GuildStickersUpdate { guild_id, current_state } => {
            let new: HashMap<u64, String> = current_state.values().map(|s| (s.id.get(), s.name.clone())).collect();
            let old = STICKERS.lock().unwrap().insert(guild_id.get(), new.clone()).unwrap_or_default();
            for (id, title, colour, text) in diff(&old, &new, |_, name| format!("**{name}**")) {
                send_by(ctx, data, *guild_id, "sticker", embed(colour, &format!("Sticker {title}"), text), serenity::audit_log::Action::Sticker(sticker_action(title)), id).await;
            }
        }

        // Guild
        GuildUpdate { old_data_if_available: Some(old), new_data: new } => {
            let fields = [
                ("Name", old.name.clone(), new.name.clone()),
                ("Icon", or_none(old.icon_url()), or_none(new.icon_url())),
                ("Banner", or_none(old.banner.as_ref()), or_none(new.banner.as_ref())),
                ("Owner", format!("<@{}>", old.owner_id), format!("<@{}>", new.owner_id)),
                ("Verification level", format!("{:?}", old.verification_level), format!("{:?}", new.verification_level)),
                ("AFK channel", or_none(old.afk_metadata.as_ref().map(|a| format!("<#{}>", a.afk_channel_id))), or_none(new.afk_metadata.as_ref().map(|a| format!("<#{}>", a.afk_channel_id)))),
                ("System channel", or_none(old.system_channel_id.map(|id| format!("<#{id}>"))), or_none(new.system_channel_id.map(|id| format!("<#{id}>")))),
                ("Vanity URL", or_none(old.vanity_url_code.as_ref()), or_none(new.vanity_url_code.as_ref())),
            ];
            if let Some(text) = changes(&fields) {
                send_by(ctx, data, new.id, "guild", embed(YELLOW, "Server Updated", text), serenity::audit_log::Action::GuildUpdate, new.id.get()).await;
            }
        }

        // Join & Leave
        GuildMemberAddition { new_member } => {
            let user = &new_member.user;
            let text = format!("<@{}> joined. Account created <t:{}:R>.", user.id, user.created_at().unix_timestamp());
            send(ctx, data, new_member.guild_id, "joinleave", with_user(embed(GREEN, "Member Joined", text), user)).await;
        }
        GuildMemberRemoval { guild_id, user, member_data_if_available } => {
            let joined = member_data_if_available
                .as_ref()
                .and_then(|m| m.joined_at)
                .map(|t| format!(" They joined <t:{}:R>.", t.unix_timestamp()))
                .unwrap_or_default();
            let Some(channel) = log_channel(data, *guild_id, "joinleave") else { return };
            // Leaving and being kicked look the same, apart from the audit log
            let kicked = actor(ctx, *guild_id, serenity::audit_log::Action::Member(MemberAction::Kick), user.id.get()).await;
            let (title, text) = match kicked {
                Some(_) => ("Member Kicked", format!("<@{}> was kicked.{joined}", user.id)),
                None => ("Member Left", format!("<@{}> left.{joined}", user.id)),
            };
            let embed = with_actor(with_user(embed(RED, title, text), user), "Kicked by", kicked);
            post(ctx, *guild_id, "joinleave", channel, embed).await;
        }

        // Member (and User, for changes to the account itself)
        GuildMemberUpdate { old_if_available: Some(old), new: Some(new), .. } => {
            let mut lines = vec![];
            if old.nick != new.nick {
                lines.push(format!("**Nickname:** {} → {}", or_none(old.nick.as_ref()), or_none(new.nick.as_ref())));
            }
            let added: Vec<String> = new.roles.iter().filter(|r| !old.roles.contains(r)).map(|r| format!("<@&{r}>")).collect();
            let removed: Vec<String> = old.roles.iter().filter(|r| !new.roles.contains(r)).map(|r| format!("<@&{r}>")).collect();
            if !added.is_empty() {
                lines.push(format!("**Roles added:** {}", added.join(", ")));
            }
            if !removed.is_empty() {
                lines.push(format!("**Roles removed:** {}", removed.join(", ")));
            }
            if old.communication_disabled_until != new.communication_disabled_until {
                lines.push(match new.communication_disabled_until {
                    Some(until) if until > serenity::Timestamp::now() => format!("**Timed out** until <t:{}:F>", until.unix_timestamp()),
                    _ => "**Timeout removed**".to_string(),
                });
            }
            if old.avatar != new.avatar {
                lines.push("**Server avatar** changed".to_string());
            }
            if !lines.is_empty() {
                let member_action = serenity::audit_log::Action::Member(if added.is_empty() && removed.is_empty() {
                    MemberAction::Update
                } else {
                    MemberAction::RoleUpdate
                });
                let text = format!("<@{}>\n{}", new.user.id, lines.join("\n"));
                send_by(ctx, data, new.guild_id, "member", with_user(embed(YELLOW, "Member Updated", text), &new.user), member_action, new.user.id.get()).await;
            }

            let fields = [
                ("Username", old.user.name.clone(), new.user.name.clone()),
                ("Display name", or_none(old.user.global_name.as_ref()), or_none(new.user.global_name.as_ref())),
            ];
            let mut text = changes(&fields);
            if old.user.avatar != new.user.avatar {
                text = Some(format!("{}**Avatar** changed", text.map(|t| t + "\n").unwrap_or_default()));
            }
            if let Some(text) = text {
                let embed = with_user(embed(BLUE, "User Updated", format!("<@{}>\n{text}", new.user.id)), &new.user)
                    .thumbnail(new.user.face());
                send(ctx, data, new.guild_id, "user", embed).await;
            }
        }
        GuildBanAddition { guild_id, banned_user } => {
            let text = format!("<@{}> was banned.", banned_user.id);
            send_by(ctx, data, *guild_id, "member", with_user(embed(RED, "Member Banned", text), banned_user), serenity::audit_log::Action::Member(MemberAction::BanAdd), banned_user.id.get()).await;
        }
        GuildBanRemoval { guild_id, unbanned_user } => {
            let text = format!("<@{}> was unbanned.", unbanned_user.id);
            send_by(ctx, data, *guild_id, "member", with_user(embed(GREEN, "Member Unbanned", text), unbanned_user), serenity::audit_log::Action::Member(MemberAction::BanRemove), unbanned_user.id.get()).await;
        }

        // Message
        MessageDelete { channel_id, deleted_message_id, guild_id: Some(guild_id) } => {
            let Some(log) = log_channel(data, *guild_id, "message") else { return };
            let cached = ctx.cache.message(*channel_id, *deleted_message_id).map(|m| m.clone());
            let author = cached.as_ref().map(|m| m.author.id);
            let embed = match cached {
                // Bots' messages (like this cog's) aren't worth logging
                Some(message) if message.author.bot => return,
                Some(message) => {
                    let mut text = format!("Message by <@{}> deleted in <#{channel_id}>:\n{}", message.author.id, quote(&message.content));
                    if !message.attachments.is_empty() {
                        let names: Vec<&str> = message.attachments.iter().map(|a| a.filename.as_str()).collect();
                        text = format!("{text}\n**Attachments:** {}", names.join(", "));
                    }
                    with_user(embed(RED, "Message Deleted", text), &message.author)
                }
                None => embed(RED, "Message Deleted", format!("A message was deleted in <#{channel_id}> (it was sent before the bot could see it).")),
            };
            // Only someone deleting another member's message is in the audit log; no entry
            // means the author deleted it themselves
            let by = match author {
                Some(author) => actor(ctx, *guild_id, serenity::audit_log::Action::Message(MessageAction::Delete), author.get()).await,
                None => None,
            };
            post(ctx, *guild_id, "message", log, with_actor(embed, "Deleted by", by)).await;
        }
        MessageDeleteBulk { channel_id, multiple_deleted_messages_ids, guild_id: Some(guild_id) } => {
            let text = format!("{} messages were deleted in <#{channel_id}>.", multiple_deleted_messages_ids.len());
            send_by(ctx, data, *guild_id, "message", embed(RED, "Messages Bulk Deleted", text), serenity::audit_log::Action::Message(MessageAction::BulkDelete), channel_id.get()).await;
        }
        MessageUpdate { old_if_available: Some(old), new: Some(new), .. } => {
            let Some(guild_id) = new.guild_id else { return };
            // Embeds loading in also count as edits, so only log changed text
            if new.author.bot || old.content == new.content {
                return;
            }
            let text = format!(
                "[Message]({}) by <@{}> edited in <#{}>\n**Before:**\n{}\n**After:**\n{}",
                new.link(),
                new.author.id,
                new.channel_id,
                quote(&old.content),
                quote(&new.content)
            );
            send(ctx, data, guild_id, "message", with_user(embed(YELLOW, "Message Edited", text), &new.author)).await;
        }

        // Role
        GuildRoleCreate { new } => {
            send_by(ctx, data, new.guild_id, "role", embed(GREEN, "Role Created", format!("<@&{}> (**{}**) was created.", new.id, new.name)), serenity::audit_log::Action::Role(RoleAction::Create), new.id.get()).await;
        }
        GuildRoleDelete { guild_id, removed_role_id, removed_role_data_if_available } => {
            let name = removed_role_data_if_available.as_ref().map(|r| format!("**{}**", r.name)).unwrap_or_else(|| format!("`{removed_role_id}`"));
            send_by(ctx, data, *guild_id, "role", embed(RED, "Role Deleted", format!("Role {name} was deleted.")), serenity::audit_log::Action::Role(RoleAction::Delete), removed_role_id.get()).await;
        }
        GuildRoleUpdate { old_data_if_available: Some(old), new } => {
            let fields = [
                ("Name", old.name.clone(), new.name.clone()),
                ("Colour", format!("#{:06x}", old.colour.0), format!("#{:06x}", new.colour.0)),
                ("Shown separately", old.hoist.to_string(), new.hoist.to_string()),
                ("Mentionable", old.mentionable.to_string(), new.mentionable.to_string()),
            ];
            let mut text = changes(&fields);
            if old.permissions != new.permissions {
                let added = new.permissions - old.permissions;
                let removed = old.permissions - new.permissions;
                let mut lines = vec![];
                if !added.is_empty() {
                    lines.push(format!("**Permissions added:** {added}"));
                }
                if !removed.is_empty() {
                    lines.push(format!("**Permissions removed:** {removed}"));
                }
                text = Some(format!("{}{}", text.map(|t| t + "\n").unwrap_or_default(), lines.join("\n")));
            }
            if let Some(text) = text {
                send_by(ctx, data, new.guild_id, "role", embed(YELLOW, "Role Updated", format!("<@&{}>\n{text}", new.id)), serenity::audit_log::Action::Role(RoleAction::Update), new.id.get()).await;
            }
        }

        // Thread
        ThreadCreate { thread } => {
            let parent = thread.parent_id.map(|id| format!(" in <#{id}>")).unwrap_or_default();
            let text = format!("<#{}> (**{}**) was created{parent}.", thread.id, thread.name);
            send_by(ctx, data, thread.guild_id, "thread", embed(GREEN, "Thread Created", text), serenity::audit_log::Action::Thread(ThreadAction::Create), thread.id.get()).await;
        }
        ThreadDelete { thread, full_thread_data } => {
            let name = full_thread_data.as_ref().map(|t| format!("**{}**", t.name)).unwrap_or_else(|| format!("`{}`", thread.id));
            send_by(ctx, data, thread.guild_id, "thread", embed(RED, "Thread Deleted", format!("Thread {name} in <#{}> was deleted.", thread.parent_id)), serenity::audit_log::Action::Thread(ThreadAction::Delete), thread.id.get()).await;
        }
        ThreadUpdate { old: Some(old), new } => {
            let archived = |t: &serenity::GuildChannel| t.thread_metadata.is_some_and(|m| m.archived).to_string();
            let locked = |t: &serenity::GuildChannel| t.thread_metadata.is_some_and(|m| m.locked).to_string();
            let fields = [
                ("Name", old.name.clone(), new.name.clone()),
                ("Archived", archived(old), archived(new)),
                ("Locked", locked(old), locked(new)),
            ];
            if let Some(text) = changes(&fields) {
                send_by(ctx, data, new.guild_id, "thread", embed(YELLOW, "Thread Updated", format!("<#{}>\n{text}", new.id)), serenity::audit_log::Action::Thread(ThreadAction::Update), new.id.get()).await;
            }
        }

        // Voice
        VoiceStateUpdate { old, new } => {
            let Some(guild_id) = new.guild_id else { return };
            let before = old.as_ref().and_then(|o| o.channel_id);
            let user = new.user_id;
            let (colour, title, text) = match (before, new.channel_id) {
                (None, Some(to)) => (GREEN, "Joined Voice", format!("<@{user}> joined <#{to}>.")),
                (Some(from), None) => (RED, "Left Voice", format!("<@{user}> left <#{from}>.")),
                (Some(from), Some(to)) if from != to => (YELLOW, "Moved Voice", format!("<@{user}> moved from <#{from}> to <#{to}>.")),
                _ => {
                    // Same channel: only server mutes and deafens are worth logging
                    let Some(old) = old else { return };
                    let mut lines = vec![];
                    if old.mute != new.mute {
                        lines.push(if new.mute { "was server muted" } else { "was server unmuted" });
                    }
                    if old.deaf != new.deaf {
                        lines.push(if new.deaf { "was server deafened" } else { "was server undeafened" });
                    }
                    if lines.is_empty() {
                        return;
                    }
                    (YELLOW, "Voice Updated", format!("<@{user}> {}.", lines.join(" and ")))
                }
            };
            send(ctx, data, guild_id, "voice", embed(colour, title, text)).await;
        }
        _ => {}
    }
}

/// (id, title, colour, text) for each emoji or sticker added, removed or renamed.
fn diff(old: &HashMap<u64, String>, new: &HashMap<u64, String>, show: impl Fn(u64, &str) -> String) -> Vec<(u64, &'static str, u32, String)> {
    let mut out = vec![];
    for (&id, name) in new {
        match old.get(&id) {
            None if !old.is_empty() => out.push((id, "Added", GREEN, format!("{} was added.", show(id, name)))),
            Some(old_name) if old_name != name => {
                out.push((id, "Renamed", YELLOW, format!("{} was renamed from `{old_name}`.", show(id, name))));
            }
            _ => {}
        }
    }
    for (&id, name) in old {
        if !new.contains_key(&id) {
            out.push((id, "Removed", RED, format!("`{name}` was removed.")));
        }
    }
    out
}

fn emoji_action(title: &str) -> EmojiAction {
    match title {
        "Added" => EmojiAction::Create,
        "Removed" => EmojiAction::Delete,
        _ => EmojiAction::Update,
    }
}

fn sticker_action(title: &str) -> StickerAction {
    match title {
        "Added" => StickerAction::Create,
        "Removed" => StickerAction::Delete,
        _ => StickerAction::Update,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_spot_added_removed_and_renamed() {
        let old = HashMap::from([(1, "cat".to_string()), (2, "dog".to_string())]);
        let new = HashMap::from([(1, "kitty".to_string()), (3, "fox".to_string())]);
        let mut found: Vec<&str> = diff(&old, &new, |_, n| n.to_string()).iter().map(|d| d.1).collect();
        found.sort();
        assert_eq!(found, ["Added", "Removed", "Renamed"]);
        // Before the server's emojis are known, nothing looks added
        assert!(diff(&HashMap::new(), &new, |_, n| n.to_string()).is_empty());
    }

    #[test]
    fn only_changed_fields_are_listed() {
        let fields = [("Name", "a".to_string(), "b".to_string()), ("Topic", "x".to_string(), "x".to_string())];
        assert_eq!(changes(&fields).unwrap(), "**Name:** a → b");
        assert_eq!(changes(&fields[1..]), None);
        assert_eq!(quote("hi\nthere"), "> hi\n> there");
    }
}
