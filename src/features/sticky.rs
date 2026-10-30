//! Sticky cog: `/sticky stick` keeps a message near the bottom of a channel. Once
//! enough messages have been posted below it, the old copy is deleted and it's
//! posted again at the bottom.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use poise::serenity_prelude as serenity;

use super::{Context, Data, Error};
use crate::log;

pub use crate::database::repositories::sticky::*;

pub const COG_NAME: &str = "sticky";
/// Messages posted below the sticky before it's moved down, if not given
const DEFAULT_COUNT: u32 = 3;
const FOOTER: &str = "This is a sticky message";
const MAX_DELAY_SECS: u32 = 300;
const COLOUR: u32 = 0x2b2d31;

fn embed(text: &str) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .colour(COLOUR)
        .description(text)
        .footer(serenity::CreateEmbedFooter::new(FOOTER))
        .timestamp(serenity::Timestamp::now())
}

/// Channels whose sticky is being moved down right now, so it only moves once.
static MOVING: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Default::default);

/// Moves the channel's sticky to the bottom once enough messages are posted below it.
pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild_id) = message.guild_id else { return };
    // The sticky itself doesn't count
    let is_sticky = message.author.id == ctx.cache.current_user().id
        && message.embeds.iter().any(|e| e.footer.as_ref().is_some_and(|f| f.text == FOOTER));
    if is_sticky || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let channel_id = message.channel_id.get();
    match data.sticky.lock().unwrap().bump(channel_id) {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(e) => return log::error(format!("Error reading sticky for channel {channel_id}: {e}")),
    }
    if !MOVING.lock().unwrap().insert(channel_id) {
        return;
    }

    let delay = data.sticky.lock().unwrap().delay(guild_id.get()).unwrap_or(DEFAULT_DELAY_SECS);
    tokio::time::sleep(std::time::Duration::from_secs(delay.into())).await;
    // It may have been removed while waiting
    let sticky = match data.sticky.lock().unwrap().get(channel_id) {
        Ok(Some(sticky)) => sticky,
        Ok(None) => {
            MOVING.lock().unwrap().remove(&channel_id);
            return;
        }
        Err(e) => {
            MOVING.lock().unwrap().remove(&channel_id);
            return log::error(format!("Error reading sticky for channel {channel_id}: {e}"));
        }
    };
    let channel = message.channel_id;
    // Already gone if someone deleted it
    let _ = channel.delete_message(ctx, sticky.message_id).await;
    match channel.send_message(ctx, serenity::CreateMessage::new().embed(embed(&sticky.text))).await {
        Ok(new) => {
            if let Err(e) = data.sticky.lock().unwrap().set_message(channel_id, new.id.get()) {
                log::error(format!("Error saving sticky for channel {channel_id}: {e}"));
            }
        }
        Err(e) => log::error(format!("Error reposting sticky in channel {channel_id}: {e}")),
    }
    MOVING.lock().unwrap().remove(&channel_id);
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Keep a message at the bottom of a channel
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    category = "Sticky",
    required_permissions = "MANAGE_MESSAGES",
    default_member_permissions = "MANAGE_MESSAGES",
    subcommands("stick", "unstick", "list", "delay"),
    subcommand_required
)]
pub async fn sticky(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Stick a message to the bottom of this channel
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn stick(
    ctx: Context<'_>,
    #[description = "The message to keep at the bottom (use \\n for new lines)"]
    #[max_length = 4000]
    message: String,
    #[description = "How many messages can be posted below it before it moves down (default 3)"]
    #[min = 1]
    #[max = 100]
    count: Option<u32>,
) -> Result<(), Error> {
    let channel = ctx.channel_id();
    if ctx.data().sticky.lock().unwrap().get(channel.get())?.is_some() {
        return say_private(ctx, "❌ This channel already has a sticky message. Remove it with `/sticky unstick` first.").await;
    }
    let text = message.replace("\\n", "\n");
    let posted = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed(&text))).await?;
    let sticky = Sticky { channel_id: channel.get(), text, max_count: count.unwrap_or(DEFAULT_COUNT).max(1), message_id: posted.id.get() };
    ctx.data().sticky.lock().unwrap().add(ctx.guild_id().unwrap().get(), &sticky)?;
    say_private(ctx, "✅ The sticky message has been set up!").await
}

/// Remove this channel's sticky message
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn unstick(ctx: Context<'_>) -> Result<(), Error> {
    let channel = ctx.channel_id();
    let Some(sticky) = ctx.data().sticky.lock().unwrap().get(channel.get())? else {
        return say_private(ctx, "❌ This channel doesn't have a sticky message. Make one with `/sticky stick`.").await;
    };
    ctx.data().sticky.lock().unwrap().remove(channel.get())?;
    // Already gone if someone deleted it
    let _ = channel.delete_message(ctx, sticky.message_id).await;
    say_private(ctx, "✅ The sticky message in this channel has been removed.").await
}

/// Set how long every sticky waits before moving down once it's due
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn delay(
    ctx: Context<'_>,
    #[description = "Seconds to wait (default 5, 0 to move straight away)"]
    #[min = 0]
    #[max = 300]
    seconds: u32,
) -> Result<(), Error> {
    let seconds = seconds.min(MAX_DELAY_SECS);
    ctx.data().sticky.lock().unwrap().set_delay(ctx.guild_id().unwrap().get(), seconds)?;
    let text = if seconds == 0 {
        "✅ Sticky messages will now move down straight away once they're due.".to_string()
    } else {
        format!("✅ Sticky messages will now wait {seconds} second{} before moving down once they're due.", if seconds == 1 { "" } else { "s" })
    };
    say_private(ctx, text).await
}

/// List this server's sticky messages
#[poise::command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_MESSAGES")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let stickies = ctx.data().sticky.lock().unwrap().list(ctx.guild_id().unwrap().get())?;
    if stickies.is_empty() {
        return say_private(ctx, "This server has no sticky messages.").await;
    }
    let lines: Vec<String> = stickies
        .iter()
        .map(|s| {
            let preview = s.text.lines().next().unwrap_or_default();
            let preview = if preview.chars().count() > 80 { format!("{}…", super::truncate(preview, 80)) } else { preview.to_string() };
            format!("<#{}> (moves after {} messages): {preview}", s.channel_id, s.max_count)
        })
        .collect();
    let pages = crate::pagination::chunk_lines(&lines, 10)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().colour(COLOUR).title("Sticky Messages").description(text))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_moves_after_enough_messages() {
        let s = Store::open_path(":memory:").unwrap();
        let sticky = Sticky { channel_id: 5, text: "Read the rules".into(), max_count: 3, message_id: 100 };
        s.add(1, &sticky).unwrap();
        assert_eq!(s.bump(6).unwrap(), None);
        assert_eq!(s.bump(5).unwrap(), None);
        assert_eq!(s.bump(5).unwrap(), None);
        assert_eq!(s.bump(5).unwrap(), Some(sticky.clone()));
        // The count starts again
        assert_eq!(s.bump(5).unwrap(), None);

        assert_eq!(s.delay(1).unwrap(), DEFAULT_DELAY_SECS);
        s.set_delay(1, 30).unwrap();
        assert_eq!(s.delay(1).unwrap(), 30);
        assert_eq!(s.delay(2).unwrap(), DEFAULT_DELAY_SECS);

        s.set_message(5, 101).unwrap();
        assert_eq!(s.list(1).unwrap()[0].message_id, 101);
        assert!(s.remove(5).unwrap());
        assert!(!s.remove(5).unwrap());
    }
}
