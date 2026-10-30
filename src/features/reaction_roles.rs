//! ReactionRoles cog: `/reaction-roles add` puts an emoji on a message already in a
//! channel. Reacting with the emoji gives its role, and
//! taking the reaction off takes the role away.

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::{Context, Data, Error};
use crate::log;

pub use crate::database::repositories::reaction_roles::*;

pub const COG_NAME: &str = "reactionroles";
const COLOUR: u32 = 0x2b2d31;
/// Seconds the alert telling a member they got or lost a role stays up
const ALERT_SECS: u64 = 5;

/// Gives or takes the role for a reaction on a reaction role message.
pub async fn on_reaction(ctx: &serenity::Context, data: &Data, reaction: &serenity::Reaction, added: bool) {
    let (Some(guild_id), Some(user_id)) = (reaction.guild_id, reaction.user_id) else { return };
    if user_id == ctx.cache.current_user().id || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let message_id = reaction.message_id.get();
    let role_id = match data.reaction_roles.lock().unwrap().role_for(message_id, &emoji_key(&reaction.emoji)) {
        Ok(Some(role_id)) => serenity::RoleId::new(role_id),
        Ok(None) => return,
        Err(e) => return log::error(format!("Error reading reaction roles for message {message_id}: {e}")),
    };
    // Bots don't get reaction roles; the member is only sent with added reactions
    if reaction.member.as_ref().is_some_and(|m| m.user.bot) {
        return;
    }
    let reason = Some("Reaction role");
    let result = if added {
        ctx.http.add_member_role(guild_id, user_id, role_id, reason).await
    } else {
        ctx.http.remove_member_role(guild_id, user_id, role_id, reason).await
    };
    let text = match &result {
        Ok(()) if added => format!("✅ <@{user_id}>, you now have <@&{role_id}>."),
        Ok(()) => format!("✅ <@{user_id}>, <@&{role_id}> has been taken away."),
        Err(e) => {
            log::warning(format!("Could not {} reaction role {role_id} for {user_id} in guild {guild_id}: {e}", if added { "give" } else { "take" }));
            format!("❌ <@{user_id}>, I couldn't {} <@&{role_id}>. Ask staff to check my permissions.", if added { "give you" } else { "take away" })
        }
    };
    alert(ctx, reaction.channel_id, user_id, text).await;
}

/// Tells the member what happened in the channel, deleting it after `ALERT_SECS`.
async fn alert(ctx: &serenity::Context, channel: serenity::ChannelId, user_id: serenity::UserId, text: String) {
    // Only the member is pinged, never the role
    let message = serenity::CreateMessage::new()
        .content(text)
        .allowed_mentions(serenity::CreateAllowedMentions::new().users([user_id]));
    let sent = match channel.send_message(&ctx.http, message).await {
        Ok(sent) => sent,
        Err(e) => return log::warning(format!("Could not post a reaction role alert in channel {channel}: {e}")),
    };
    let http = ctx.http.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(ALERT_SECS)).await;
        // Already gone if someone deleted it
        let _ = sent.delete(&http).await;
    });
}

/// Forgets the reaction roles on a deleted message.
pub fn on_message_delete(data: &Data, message_id: serenity::MessageId) {
    if let Err(e) = data.reaction_roles.lock().unwrap().remove_message(message_id.get()) {
        log::error(format!("Error removing reaction roles for deleted message {message_id}: {e}"));
    }
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// A message link (`…/channels/<guild>/<channel>/<message>`) as its guild, channel and
/// message, or a bare message ID, which has no guild or channel.
fn parse_message(text: &str) -> Option<(Option<u64>, Option<u64>, u64)> {
    let parts: Vec<&str> = text.trim().split('/').collect();
    let id = |s: &str| s.parse::<u64>().ok().filter(|&id| id != 0);
    match parts.as_slice() {
        [message] => Some((None, None, id(message)?)),
        [.., "channels", guild, channel, message] => Some((id(guild), Some(id(channel)?), id(message)?)),
        _ => None,
    }
}

/// Finds the message a link or ID points at: a link anywhere in this server, an ID in
/// this channel. Replies with why if it can't be found.
async fn find_message(ctx: Context<'_>, text: &str) -> Result<Option<serenity::Message>, Error> {
    let Some((guild, channel, message_id)) = parse_message(text) else {
        say_private(ctx, "❌ That isn't a message link or ID. Right-click the message and choose **Copy Message Link**.").await?;
        return Ok(None);
    };
    if guild.is_some_and(|g| g != ctx.guild_id().unwrap().get()) {
        say_private(ctx, "❌ That message is in another server.").await?;
        return Ok(None);
    }
    let from_link = channel.is_some();
    let channel = channel.map(serenity::ChannelId::new).unwrap_or(ctx.channel_id());
    match channel.message(ctx, message_id).await {
        Ok(message) => Ok(Some(message)),
        Err(_) => {
            let text = if !from_link {
                "❌ I couldn't find that message in this channel. Use its link to pick one in another channel."
            } else {
                "❌ I couldn't find that message. Check the link, and that I can see its channel."
            };
            say_private(ctx, text).await?;
            Ok(None)
        }
    }
}

/// Give members a role when they react to a message
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "ReactionRoles",
    rename = "reaction-roles",
    required_permissions = "MANAGE_ROLES",
    default_member_permissions = "MANAGE_ROLES",
    subcommands("add", "remove", "list"),
    subcommand_required
)]
pub async fn reaction_roles(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Make an emoji on a message give a role
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn add(
    ctx: Context<'_>,
    #[description = "The message's link (or ID, if it's in this channel)"] message: String,
    #[description = "The emoji to react with"] emoji: String,
    #[description = "The role it gives"] role: serenity::Role,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let Ok(reaction) = serenity::ReactionType::try_from(emoji.trim()) else {
        return say_private(ctx, "❌ That isn't an emoji.").await;
    };
    if role.id.get() == guild_id.get() || role.managed {
        return say_private(ctx, "❌ That role can't be given to members.").await;
    }
    let bot_top = ctx.guild().and_then(|g| {
        let bot = g.members.get(&ctx.cache().current_user().id)?;
        bot.roles.iter().filter_map(|r| g.roles.get(r)).map(|r| r.position).max()
    });
    if bot_top.is_some_and(|top| role.position >= top) {
        return say_private(ctx, format!("❌ <@&{}> is above my highest role, so I can't give it. Move my role above it first.", role.id)).await;
    }
    let Some(target) = find_message(ctx, &message).await? else { return Ok(()) };
    if target.react(ctx, reaction.clone()).await.is_err() {
        return say_private(ctx, "❌ I couldn't react with that emoji. Custom emojis have to be from a server I'm in.").await;
    }
    let key = emoji_key(&reaction);
    let reaction_role = ReactionRole { message_id: target.id.get(), channel_id: target.channel_id.get(), emoji: key, role_id: role.id.get() };
    ctx.data().reaction_roles.lock().unwrap().add(guild_id.get(), &reaction_role)?;
    say_private(ctx, format!("✅ Reacting with {reaction} on {} now gives <@&{}>.", target.link(), role.id)).await
}

/// Stop an emoji on a message giving a role
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn remove(
    ctx: Context<'_>,
    #[description = "The message's link or ID"] message: String,
    #[description = "The emoji"] emoji: String,
) -> Result<(), Error> {
    // Not looked up, so it works on deleted messages too
    let Some((_, _, message_id)) = parse_message(&message) else {
        return say_private(ctx, "❌ That isn't a message link or ID.").await;
    };
    let message_id = serenity::MessageId::new(message_id);
    let Ok(reaction) = serenity::ReactionType::try_from(emoji.trim()) else {
        return say_private(ctx, "❌ That isn't an emoji.").await;
    };
    let key = emoji_key(&reaction);
    let guild_id = ctx.guild_id().unwrap().get();
    let found = ctx.data().reaction_roles.lock().unwrap().list(guild_id)?.into_iter().find(|r| r.message_id == message_id.get() && r.emoji == key);
    let Some(found) = found else {
        return say_private(ctx, "❌ That emoji doesn't give a role on that message. See them with `/reaction-roles list`.").await;
    };
    ctx.data().reaction_roles.lock().unwrap().remove(message_id.get(), &key)?;
    // The message may be gone, or the reaction already removed
    let _ = ctx
        .http()
        .delete_reaction_me(serenity::ChannelId::new(found.channel_id), message_id, &reaction)
        .await;
    say_private(ctx, format!("✅ {reaction} no longer gives <@&{}>.", found.role_id)).await
}

/// List this server's reaction roles
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let roles = ctx.data().reaction_roles.lock().unwrap().list(guild_id.get())?;
    if roles.is_empty() {
        return say_private(ctx, "This server has no reaction roles. Add one with `/reaction-roles add`.").await;
    }
    let mut lines = Vec::new();
    let mut last = None;
    for r in &roles {
        if last != Some(r.message_id) {
            let link = serenity::MessageId::new(r.message_id).link(serenity::ChannelId::new(r.channel_id), Some(guild_id));
            lines.push(format!("**{link}**"));
            last = Some(r.message_id);
        }
        lines.push(format!("{} → <@&{}>", emoji_display(&r.emoji), r.role_id));
    }
    let pages = crate::pagination::chunk_lines(&lines, 15)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().colour(COLOUR).title("Reaction Roles").description(text))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaction_roles_are_stored_per_message_and_emoji() {
        let s = Store::open_path(":memory:").unwrap();
        let red = ReactionRole { message_id: 10, channel_id: 5, emoji: "🔴".into(), role_id: 100 };
        let custom = ReactionRole { message_id: 10, channel_id: 5, emoji: "123".into(), role_id: 101 };
        s.add(1, &red).unwrap();
        s.add(1, &custom).unwrap();
        assert_eq!(s.role_for(10, "🔴").unwrap(), Some(100));
        assert_eq!(s.role_for(11, "🔴").unwrap(), None);
        // Adding the emoji again changes its role
        s.add(1, &ReactionRole { role_id: 102, ..red.clone() }).unwrap();
        assert_eq!(s.role_for(10, "🔴").unwrap(), Some(102));
        assert_eq!(s.list(1).unwrap().len(), 2);
        assert!(s.list(2).unwrap().is_empty());

        assert!(s.remove(10, "🔴").unwrap());
        assert!(!s.remove(10, "🔴").unwrap());
        assert_eq!(s.remove_message(10).unwrap(), 1);
    }

    #[test]
    fn emoji_keys_ignore_variation_selectors_and_custom_names() {
        let plain = serenity::ReactionType::try_from("❤").unwrap();
        let selected = serenity::ReactionType::try_from("❤\u{fe0f}").unwrap();
        assert_eq!(emoji_key(&plain), emoji_key(&selected));
        let custom = serenity::ReactionType::try_from("<:wave:123456789012345678>").unwrap();
        assert_eq!(emoji_key(&custom), "123456789012345678");
        assert_eq!(emoji_display("123456789012345678"), "<:e:123456789012345678>");
    }

    #[test]
    fn messages_come_from_ids_or_links() {
        assert_eq!(parse_message("123"), Some((None, None, 123)));
        assert_eq!(parse_message("https://discord.com/channels/1/2/345"), Some((Some(1), Some(2), 345)));
        assert_eq!(parse_message("nope"), None);
        assert_eq!(parse_message("0"), None);
        assert_eq!(parse_message("https://discord.com/2/345"), None);
    }
}
