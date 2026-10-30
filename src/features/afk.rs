//! AFK cog: `/afk [reason]` marks a member as away. Anyone who mentions them is told why
//! and since when, and the next time they send a message they're welcomed back and their
//! AFK is cleared. Running `/afk` again while away clears it too.

use std::time::Duration;

use poise::serenity_prelude as serenity;

use super::{parse_iso, truncate, Context, Data, Error};
use crate::log;

pub use crate::database::repositories::afk::*;

pub const COG_NAME: &str = "afk";
/// How long the bot's AFK notices stay before they're deleted, so they don't clutter the chat
const NOTICE_TIME: Duration = Duration::from_secs(10);
/// Most AFK members one message reports on
const MAX_NOTICES: usize = 5;
const DEFAULT_REASON: &str = "No reason given";

const YELLOW: u32 = 0xf1c40f;
const CYAN: u32 = 0x1abc9c;

/// A Discord timestamp like "2 hours ago" for an ISO time saved by `now_iso`.
fn relative(since: &str) -> String {
    parse_iso(since).map_or_else(|| "a while ago".to_string(), |time| format!("<t:{}:R>", time.timestamp()))
}

/// Posts a notice in the channel and deletes it after a short while.
async fn notice(ctx: &serenity::Context, channel: serenity::ChannelId, embed: serenity::CreateEmbed) {
    match channel.send_message(ctx, serenity::CreateMessage::new().embed(embed)).await {
        Ok(message) => {
            let http = ctx.http.clone();
            tokio::spawn(async move {
                tokio::time::sleep(NOTICE_TIME).await;
                let _ = message.delete(&http).await;
            });
        }
        Err(e) => log::error(format!("Error posting AFK notice in channel {channel}: {e}")),
    }
}

/// Welcomes back an AFK member who sends a message, and tells anyone who mentions an AFK
/// member that they're away.
pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild_id) = message.guild_id else { return };
    if message.author.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let guild = guild_id.get();

    let back = match data.afk.lock().unwrap().clear(guild, message.author.id.get()) {
        Ok(back) => back,
        Err(e) => return log::error(format!("Error clearing AFK: {e}")),
    };
    if let Some(since) = back {
        let embed = serenity::CreateEmbed::new()
            .colour(CYAN)
            .title(format!("Welcome back, {}!", message.author.display_name()))
            .description(format!("You're no longer AFK. You went AFK {}.", relative(&since)));
        notice(ctx, message.channel_id, embed).await;
    }

    let mut seen = vec![message.author.id];
    let mut away = vec![];
    for user in &message.mentions {
        if user.bot || seen.contains(&user.id) {
            continue;
        }
        seen.push(user.id);
        match data.afk.lock().unwrap().get(guild, user.id.get()) {
            Ok(Some((reason, since))) => away.push(format!("🟡 **{}** is AFK: {reason} (since {})", user.display_name(), relative(&since))),
            Ok(None) => {}
            Err(e) => log::error(format!("Error reading AFK: {e}")),
        }
        if away.len() == MAX_NOTICES {
            break;
        }
    }
    if !away.is_empty() {
        let embed = serenity::CreateEmbed::new().colour(YELLOW).description(away.join("\n"));
        notice(ctx, message.channel_id, embed).await;
    }
}

/// Set yourself as AFK, or clear it if you already are
#[poise::command(slash_command, guild_only, ephemeral, category = "AFK")]
pub async fn afk(
    ctx: Context<'_>,
    #[description = "Why you're going AFK"]
    #[max_length = 200]
    reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let user_id = ctx.author().id.get();
    if ctx.data().afk.lock().unwrap().clear(guild_id, user_id)?.is_some() {
        ctx.say("✅ You're no longer AFK.").await?;
        return Ok(());
    }
    let reason = reason.map(|r| truncate(r.trim(), 200)).filter(|r| !r.is_empty()).unwrap_or_else(|| DEFAULT_REASON.to_string());
    ctx.data().afk.lock().unwrap().set(guild_id, user_id, &reason)?;
    ctx.say(format!(
        "💤 You're now AFK: **{reason}**\nAnyone who mentions you will be told, and it clears the next time you send a message."
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn afk_is_set_read_and_cleared() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.get(1, 2).unwrap(), None);
        assert_eq!(s.clear(1, 2).unwrap(), None);
        s.set(1, 2, "Lunch").unwrap();
        assert_eq!(s.get(1, 2).unwrap().unwrap().0, "Lunch");
        // Only in that server
        assert_eq!(s.get(3, 2).unwrap(), None);
        assert!(s.clear(1, 2).unwrap().is_some());
        assert_eq!(s.get(1, 2).unwrap(), None);
    }
}
