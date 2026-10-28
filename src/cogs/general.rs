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
