//! OffLimits cog: `/offlimit add` makes a channel completely off-limits to the bot. It
//! refuses every command there, from staff too, and ignores everything that happens in it:
//! messages, edits, deletes, reactions and button presses, for every feature, logs and
//! security included. `/offlimit` itself works anywhere, so a channel can be lifted again.

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::{root_command, Context, Data, Error};

pub use crate::database::repositories::offlimit::*;

pub const COG_NAME: &str = "offlimits";
const COLOUR: u32 = 0x2b2d31;

/// Whether the channel is off-limits, with the cog enabled in its server.
fn is_offlimit(data: &Data, guild_id: Option<serenity::GuildId>, channel_id: serenity::ChannelId) -> bool {
    let Some(guild_id) = guild_id else { return false };
    data.is_cog_enabled(guild_id.get(), COG_NAME) && data.offlimit.lock().unwrap().is_offlimit(channel_id.get()).unwrap_or(false)
}

/// Whether the event happened in an off-limits channel, so no feature should see it. Slash
/// commands aren't covered: `command_check` refuses those, so `/offlimit` still works.
pub fn is_offlimit_event(data: &Data, event: &serenity::FullEvent) -> bool {
    use serenity::FullEvent as E;
    let (guild_id, channel_id) = match event {
        E::Message { new_message } => (new_message.guild_id, new_message.channel_id),
        E::MessageUpdate { event, .. } => (event.guild_id, event.channel_id),
        E::MessageDelete { guild_id, channel_id, .. } => (*guild_id, *channel_id),
        E::MessageDeleteBulk { guild_id, channel_id, .. } => (*guild_id, *channel_id),
        E::ReactionAdd { add_reaction: r } | E::ReactionRemove { removed_reaction: r } => (r.guild_id, r.channel_id),
        E::InteractionCreate { interaction: serenity::Interaction::Component(i) } => (i.guild_id, i.channel_id),
        E::InteractionCreate { interaction: serenity::Interaction::Modal(i) } => (i.guild_id, i.channel_id),
        _ => return false,
    };
    is_offlimit(data, guild_id, channel_id)
}

/// Refuses every command in off-limits channels, except `/offlimit`.
pub async fn command_check(ctx: Context<'_>) -> Result<(), Error> {
    if root_command(ctx).name != "offlimit" && is_offlimit(ctx.data(), ctx.guild_id(), ctx.channel_id()) {
        return Err("⛔ This channel is off-limits to me.".into());
    }
    Ok(())
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Make channels completely off-limits to the bot
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "OffLimits",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("add", "remove", "list"),
    subcommand_required
)]
pub async fn offlimit(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Make a channel off-limits: the bot does nothing at all there
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn add(
    ctx: Context<'_>,
    #[description = "The channel (default: this one)"]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let channel = channel.map_or(ctx.channel_id(), |c| c.id);
    if !ctx.data().offlimit.lock().unwrap().add(ctx.guild_id().unwrap().get(), channel.get())? {
        return say_private(ctx, format!("❌ <#{channel}> is already off-limits.")).await;
    }
    say_private(ctx, format!("⛔ <#{channel}> is now off-limits: I won't do anything there. Lift it with `/offlimit remove`.")).await
}

/// Lift an off-limits channel
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn remove(
    ctx: Context<'_>,
    #[description = "The channel (default: this one)"]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let channel = channel.map_or(ctx.channel_id(), |c| c.id);
    if !ctx.data().offlimit.lock().unwrap().remove(channel.get())? {
        return say_private(ctx, format!("❌ <#{channel}> isn't off-limits.")).await;
    }
    say_private(ctx, format!("✅ <#{channel}> is no longer off-limits.")).await
}

/// List this server's off-limits channels
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let channels = ctx.data().offlimit.lock().unwrap().list(ctx.guild_id().unwrap().get())?;
    if channels.is_empty() {
        return say_private(ctx, "No channels are off-limits in this server.").await;
    }
    let lines: Vec<String> = channels.iter().map(|id| format!("<#{id}>")).collect();
    let pages = crate::pagination::chunk_lines(&lines, 20)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().colour(COLOUR).title("Off-Limits Channels").description(text))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn channels_are_offlimit_and_cleaned_up() {
        let s = Store::open_path(":memory:").unwrap();
        assert!(!s.is_offlimit(5).unwrap());
        assert!(s.add(1, 5).unwrap());
        assert!(!s.add(1, 5).unwrap());
        assert!(s.add(1, 6).unwrap());
        assert!(s.is_offlimit(5).unwrap());
        assert_eq!(s.list(1).unwrap(), vec![5, 6]);
        assert!(s.list(2).unwrap().is_empty());

        // Channel 6 was deleted
        let current = HashMap::from([(1, HashSet::from([5]))]);
        s.cleanup_stale(&current).unwrap();
        assert_eq!(s.list(1).unwrap(), vec![5]);

        assert!(s.remove(5).unwrap());
        assert!(!s.remove(5).unwrap());
        assert!(!s.is_offlimit(5).unwrap());
    }
}
