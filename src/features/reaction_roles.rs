//! ReactionRoles cog: `/reaction-roles post` posts a message staff write, and
//! `/reaction-roles add` puts an emoji on it. Reacting with the emoji gives its role, and
//! taking the reaction off takes the role away.

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::{Context, Data, Error};
use crate::log;

pub use crate::database::repositories::reaction_roles::*;

pub const COG_NAME: &str = "reactionroles";
const COLOUR: u32 = 0x2b2d31;

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
    if let Err(e) = result {
        log::warning(format!("Could not {} reaction role {role_id} for {user_id} in guild {guild_id}: {e}", if added { "give" } else { "take" }));
    }
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

/// A message ID, or a message link (the ID is its last part).
fn parse_message_id(text: &str) -> Option<serenity::MessageId> {
    let id = text.trim().rsplit('/').next()?.parse::<u64>().ok()?;
    (id != 0).then(|| serenity::MessageId::new(id))
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
    subcommands("post", "edit", "add", "remove", "list"),
    subcommand_required
)]
pub async fn reaction_roles(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Post a message for reaction roles, with your own text
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn post(
    ctx: Context<'_>,
    #[description = "The message (use \\n for new lines)"]
    #[max_length = 4000]
    message: String,
    #[description = "A title above the message"]
    #[max_length = 256]
    title: Option<String>,
    #[description = "Where to post it (default: this channel)"] channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let channel = channel.map(|c| c.id).unwrap_or(ctx.channel_id());
    let mut embed = serenity::CreateEmbed::new().colour(COLOUR).description(message.replace("\\n", "\n"));
    if let Some(title) = title {
        embed = embed.title(title);
    }
    let posted = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await?;
    say_private(
        ctx,
        format!("✅ Posted {}. Add roles to it with `/reaction-roles add message:{}`.", posted.link(), posted.id),
    )
    .await
}

/// Change the text of a reaction roles message the bot posted
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn edit(
    ctx: Context<'_>,
    #[description = "The message's ID or link"] message: String,
    #[description = "The new message (use \\n for new lines)"]
    #[max_length = 4000]
    text: String,
    #[description = "The new title (leave empty to keep it)"]
    #[max_length = 256]
    title: Option<String>,
    #[description = "The channel it's in (default: this one)"] channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let Some(message_id) = parse_message_id(&message) else {
        return say_private(ctx, "❌ That isn't a message ID or link.").await;
    };
    let channel = channel.map(|c| c.id).unwrap_or(ctx.channel_id());
    let Ok(mut posted) = channel.message(ctx, message_id).await else {
        return say_private(ctx, "❌ I couldn't find that message in this channel. Pick its channel with `channel`.").await;
    };
    if posted.author.id != ctx.cache().current_user().id {
        return say_private(ctx, "❌ I can only edit messages I posted with `/reaction-roles post`.").await;
    }
    let old = posted.embeds.first();
    let title = title.or_else(|| old.and_then(|e| e.title.clone()));
    let mut embed = serenity::CreateEmbed::new().colour(COLOUR).description(text.replace("\\n", "\n"));
    if let Some(title) = title {
        embed = embed.title(title);
    }
    posted.edit(ctx, serenity::EditMessage::new().embed(embed)).await?;
    say_private(ctx, "✅ The message has been updated.").await
}

/// Make an emoji on a message give a role
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn add(
    ctx: Context<'_>,
    #[description = "The message's ID or link"] message: String,
    #[description = "The emoji to react with"] emoji: String,
    #[description = "The role it gives"] role: serenity::Role,
    #[description = "The channel the message is in (default: this one)"] channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let Some(message_id) = parse_message_id(&message) else {
        return say_private(ctx, "❌ That isn't a message ID or link.").await;
    };
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
    let channel = channel.map(|c| c.id).unwrap_or(ctx.channel_id());
    let Ok(target) = channel.message(ctx, message_id).await else {
        return say_private(ctx, "❌ I couldn't find that message in this channel. Pick its channel with `channel`.").await;
    };
    if target.react(ctx, reaction.clone()).await.is_err() {
        return say_private(ctx, "❌ I couldn't react with that emoji. Custom emojis have to be from a server I'm in.").await;
    }
    let key = emoji_key(&reaction);
    let reaction_role = ReactionRole { message_id: message_id.get(), channel_id: channel.get(), emoji: key, role_id: role.id.get() };
    ctx.data().reaction_roles.lock().unwrap().add(guild_id.get(), &reaction_role)?;
    say_private(ctx, format!("✅ Reacting with {reaction} on {} now gives <@&{}>.", target.link(), role.id)).await
}

/// Stop an emoji on a message giving a role
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_ROLES")]
async fn remove(
    ctx: Context<'_>,
    #[description = "The message's ID or link"] message: String,
    #[description = "The emoji"] emoji: String,
) -> Result<(), Error> {
    let Some(message_id) = parse_message_id(&message) else {
        return say_private(ctx, "❌ That isn't a message ID or link.").await;
    };
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
        return say_private(ctx, "This server has no reaction roles. Post a message with `/reaction-roles post`.").await;
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
    fn message_ids_come_from_ids_or_links() {
        assert_eq!(parse_message_id("123"), Some(serenity::MessageId::new(123)));
        assert_eq!(parse_message_id("https://discord.com/channels/1/2/345"), Some(serenity::MessageId::new(345)));
        assert_eq!(parse_message_id("nope"), None);
        assert_eq!(parse_message_id("0"), None);
    }
}
