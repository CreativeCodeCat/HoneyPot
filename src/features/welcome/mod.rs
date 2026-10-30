//! Welcome: greets new members with a welcome image in the welcome log channel, and thanks
//! boosters with a boost image in the boosts log channel. Both channels are set with
//! `/server-setup logs`, and both run as part of the Logs cog.

pub mod card;

use ::serenity::all as serenity;

use super::logging::{embed, log_channel, with_user};
use super::Data;
use crate::log;
use crate::shared::models::CardUser;

const GREEN: u32 = 0x57f287;
const PINK: u32 = 0xff73fa;
const GREY: u32 = 0x95a5a6;

/// The member's names and avatar, for the welcome and boost images.
async fn card_member(member: &serenity::Member) -> CardUser {
    CardUser::fetch(vec![member.display_name().to_string(), member.user.name.clone()], &member.user).await
}

/// Posts in the boosts log when a member starts or stops boosting: a thank-you image for a
/// new boost (an embed if it can't be drawn), and an embed when they stop.
pub(crate) async fn boost_change(ctx: &serenity::Context, data: &Data, old: &serenity::Member, new: &serenity::Member) {
    let started = old.premium_since.is_none() && new.premium_since.is_some();
    let stopped = old.premium_since.is_some() && new.premium_since.is_none();
    if !started && !stopped {
        return;
    }
    let Some(channel) = log_channel(data, new.guild_id, "boosts") else { return };
    // Fetched, as the cached server may not have the new boost count yet
    let (server, boosts, level) = match new.guild_id.to_partial_guild(ctx).await {
        Ok(guild) => (guild.name, guild.premium_subscription_count.unwrap_or(0), u8::from(guild.premium_tier)),
        Err(e) => return log::error(format!("Error reading boosts for guild {}: {e}", new.guild_id)),
    };
    let user = &new.user;
    let level_text = if level == 0 { "no level yet".to_string() } else { format!("level {level}") };
    let mut message = serenity::CreateMessage::new().allowed_mentions(serenity::CreateAllowedMentions::new());
    if started {
        let card = card_member(new).await;
        let name = server.clone();
        let image = tokio::task::spawn_blocking(move || card::boost(&card, &name, boosts, level)).await.ok().flatten();
        message = match image {
            Some(png) => message.add_file(serenity::CreateAttachment::bytes(png, "boost.png")),
            None => {
                log::warning("Could not draw the boost image, sending an embed instead");
                let text = format!("<@{}> boosted **{server}**! 💎 The server now has {boosts} boosts ({level_text}).", user.id);
                message.embed(with_user(embed(PINK, "Server Boosted", text), user))
            }
        };
    } else {
        let text = format!("<@{}> stopped boosting **{server}**. The server now has {boosts} boosts ({level_text}).", user.id);
        message = message.embed(with_user(embed(GREY, "Boost Ended", text), user));
    }
    if let Err(e) = channel.send_message(ctx, message).await {
        log::error(format!("Error posting boost log in guild {}: {e}", new.guild_id));
    }
}

/// Greets a new member in the welcome channel with a welcome image. Falls back to an embed
/// if the image can't be drawn.
pub(crate) async fn welcome(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    let Some(channel) = log_channel(data, member.guild_id, "welcome") else { return };
    let (server, count, icon) = match ctx.cache.guild(member.guild_id) {
        Some(guild) => (guild.name.clone(), guild.member_count, guild.icon_url()),
        None => return,
    };
    let user = &member.user;
    let card = card_member(member).await;
    let name = server.clone();
    let image = tokio::task::spawn_blocking(move || card::welcome(&card, &name, count)).await.ok().flatten();

    // Just the image, with nobody pinged
    let mut message = serenity::CreateMessage::new().allowed_mentions(serenity::CreateAllowedMentions::new());
    match image {
        Some(png) => message = message.add_file(serenity::CreateAttachment::bytes(png, "welcome.png")),
        None => {
            log::warning("Could not draw the welcome image, sending an embed instead");
            let mut embed = serenity::CreateEmbed::new()
                .colour(GREEN)
                .title(format!("Welcome to {server}!"))
                .description(format!("Hey <@{}>, welcome to **{server}**! 🎉\nYou're member **#{count}**. We're glad to have you here.", user.id))
                .thumbnail(member.face())
                .timestamp(serenity::Timestamp::now());
            if let Some(icon) = icon {
                embed = embed.footer(serenity::CreateEmbedFooter::new(&server).icon_url(icon));
            }
            message = message.embed(embed);
        }
    }
    if let Err(e) = channel.send_message(ctx, message).await {
        log::error(format!("Error posting welcome message in guild {}: {e}", member.guild_id));
    }
}
