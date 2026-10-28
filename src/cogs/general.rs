use chrono::Utc;
use poise::serenity_prelude as serenity;

use super::{Context, Error};

/// Check bot latency and uptime
#[poise::command(
    slash_command,
    prefix_command,
    ephemeral,
    category = "General"
)]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    // Time from the triggering message/interaction to now
    let command_latency = (Utc::now() - *ctx.created_at()).num_milliseconds();
    // WebSocket heartbeat latency
    let bot_latency = ctx.ping().await.as_millis();

    let uptime = ctx.data().start_time.elapsed().as_secs();
    let (days, hours, minutes, seconds) = (
        uptime / 86400,
        uptime % 86400 / 3600,
        uptime % 3600 / 60,
        uptime % 60,
    );

    let embed = serenity::CreateEmbed::new()
        .title("🏓 Bot Status")
        .colour(0x3498db)
        .field("`Command Latency`", format!("{command_latency} ms"), false)
        .field("`Bot Latency`", format!("{bot_latency} ms"), false)
        .field(
            "`Bot Uptime`",
            format!("{days} Days, {hours} Hrs, {minutes} Min, {seconds} Sec"),
            false,
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Most messages `/cleanup` deletes, and looks through, at once
const MAX_CLEANUP: usize = 1000;
/// Discord only bulk deletes messages newer than this
const BULK_DELETE_MAX_AGE_DAYS: i64 = 14;

/// Delete recent messages in this channel
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "General",
    required_permissions = "MANAGE_MESSAGES",
    default_member_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "MANAGE_MESSAGES | READ_MESSAGE_HISTORY"
)]
pub async fn cleanup(
    ctx: Context<'_>,
    #[description = "How many messages to delete (up to 1000)"]
    #[min = 1]
    #[max = 1000]
    amount: usize,
    #[description = "Only delete messages from this member"] user: Option<serenity::User>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let amount = amount.clamp(1, MAX_CLEANUP);
    let channel = ctx.channel_id();
    let cutoff = Utc::now() - chrono::Duration::days(BULK_DELETE_MAX_AGE_DAYS);

    // Look back through the newest messages until enough match, or they get too old to bulk delete
    let mut matched = vec![];
    let mut too_old = false;
    let mut before = None;
    let mut scanned = 0;
    'scan: while matched.len() < amount && scanned < MAX_CLEANUP {
        let mut request = serenity::GetMessages::new().limit(100);
        if let Some(id) = before {
            request = request.before(id);
        }
        let batch = channel.messages(ctx, request).await?;
        let Some(last) = batch.last() else { break };
        before = Some(last.id);
        for message in &batch {
            scanned += 1;
            if *message.timestamp < cutoff {
                too_old = true;
                break 'scan;
            }
            // A prefix command's own message is deleted along with the rest
            if user.as_ref().is_none_or(|u| message.author.id == u.id) {
                matched.push(message.id);
                if matched.len() == amount {
                    break 'scan;
                }
            }
        }
    }

    // Bulk deletes take 2 to 100 messages
    for chunk in matched.chunks(100) {
        match chunk {
            [id] => channel.delete_message(ctx, *id).await?,
            ids => channel.delete_messages(ctx, ids).await?,
        }
    }

    let from = user.map(|u| format!(" from <@{}>", u.id)).unwrap_or_default();
    let mut text = format!("🧹 Deleted {} message{}{from}.", matched.len(), if matched.len() == 1 { "" } else { "s" });
    if too_old && matched.len() < amount {
        text = format!("{text}\nMessages older than {BULK_DELETE_MAX_AGE_DAYS} days can't be bulk deleted, so they were left.");
    }
    let embed = serenity::CreateEmbed::new().colour(0x3498db).description(text);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// How long the confirm buttons on `/resetchannel` wait
const RESET_CONFIRM_TIME: std::time::Duration = std::time::Duration::from_secs(30);

/// Recreate a channel with the same settings and permissions, then delete the old one
#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    category = "General",
    required_permissions = "MANAGE_CHANNELS",
    default_member_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS | MANAGE_ROLES"
)]
pub async fn resetchannel(
    ctx: Context<'_>,
    #[description = "Channel to reset (default: this one)"]
    #[channel_types("Text", "News", "Voice", "Stage", "Forum")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let old = match channel {
        Some(channel) => channel,
        None => match ctx.channel_id().to_channel(ctx).await?.guild() {
            Some(channel) => channel,
            None => {
                ctx.say("❌ This isn't a server channel.").await?;
                return Ok(());
            }
        },
    };
    use serenity::ChannelType::*;
    if !matches!(old.kind, Text | News | Voice | Stage | Forum) {
        ctx.say("❌ Only text, announcement, voice, stage and forum channels can be reset.").await?;
        return Ok(());
    }

    // Deleting a channel can't be undone, so ask first
    let (confirm_id, cancel_id) = (format!("{}reset_confirm", ctx.id()), format!("{}reset_cancel", ctx.id()));
    let buttons = |disabled: bool| {
        vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&confirm_id).label("Reset").style(serenity::ButtonStyle::Danger).disabled(disabled),
            serenity::CreateButton::new(&cancel_id).label("Cancel").style(serenity::ButtonStyle::Secondary).disabled(disabled),
        ])]
    };
    let warning = format!(
        "⚠️ This makes a copy of <#{}> with the same settings and permissions, then **deletes the old one and all its messages**. \
         Anything set up to use the old channel (logs, suggestions, tickets, honeypots...) will need setting up again.",
        old.id
    );
    let reply = ctx.send(poise::CreateReply::default().content(warning).components(buttons(false))).await?;
    let (confirm, cancel) = (confirm_id.clone(), cancel_id.clone());
    let author = ctx.author().id;
    let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .filter(move |i| (i.data.custom_id == confirm || i.data.custom_id == cancel) && i.user.id == author)
        .timeout(RESET_CONFIRM_TIME)
        .await;
    let confirmed = match press {
        Some(press) => {
            press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
            press.data.custom_id == confirm_id
        }
        None => false,
    };
    if !confirmed {
        reply.edit(ctx, poise::CreateReply::default().content("Reset cancelled.").components(buttons(true))).await?;
        return Ok(());
    }

    let mut copy = serenity::CreateChannel::new(&old.name)
        .kind(old.kind)
        .nsfw(old.nsfw)
        .position(old.position)
        .permissions(old.permission_overwrites.clone());
    if let Some(parent) = old.parent_id {
        copy = copy.category(parent);
    }
    if let Some(topic) = &old.topic {
        copy = copy.topic(topic);
    }
    if let Some(seconds) = old.rate_limit_per_user {
        copy = copy.rate_limit_per_user(seconds);
    }
    if let Some(bitrate) = old.bitrate {
        copy = copy.bitrate(bitrate);
    }
    if let Some(limit) = old.user_limit {
        copy = copy.user_limit(limit);
    }
    if let Some(region) = &old.rtc_region {
        copy = copy.rtc_region(region.clone());
    }
    if let Some(mode) = old.video_quality_mode {
        copy = copy.video_quality_mode(mode);
    }
    if let Some(duration) = old.default_auto_archive_duration {
        copy = copy.default_auto_archive_duration(duration);
    }
    if old.kind == Forum {
        copy = copy.available_tags(old.available_tags.clone());
        if let Some(emoji) = old.default_reaction_emoji.clone() {
            copy = copy.default_reaction_emoji(emoji);
        }
        if let Some(order) = old.default_sort_order {
            copy = copy.default_sort_order(order);
        }
    }

    let new = old.guild_id.create_channel(ctx, copy).await?;
    // Creating with a position doesn't always stick, so set it again
    let _ = new.id.edit(ctx, serenity::EditChannel::new().position(old.position)).await;
    old.delete(ctx).await?;

    let done = format!("🧹 <#{}> has been reset.", new.id);
    if old.id != ctx.channel_id() {
        reply.edit(ctx, poise::CreateReply::default().content(done).components(buttons(true))).await?;
    }
    if matches!(new.kind, Text | News) {
        let note = serenity::CreateEmbed::new().colour(0x3498db).description(format!("🧹 This channel was reset by <@{author}>."));
        new.id.send_message(ctx, serenity::CreateMessage::new().embed(note)).await?;
    }
    Ok(())
}
