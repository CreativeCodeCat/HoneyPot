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
