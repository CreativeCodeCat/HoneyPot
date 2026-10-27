//! Keeps voice channels named like "MEMBERS: 123" updated with live server statistics.

use std::time::Duration;

use poise::serenity_prelude as serenity;
use unicode_normalization::char::canonical_combining_class;
use unicode_normalization::UnicodeNormalization;

use super::Data;

const COG_NAME: &str = "serverstats";

const UPPERCASE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const SMALL_CAPS: &str = "ᴀʙᴄᴅᴇғɢʜɪᴊᴋʟᴍɴᴏᴘǫʀsᴛᴜᴠᴡxʏᴢ";

const STAT_KEYWORDS: &[&str] = &["MEMBERS", "BOTS", "ROLES", "BOOSTS", "CHANNELS"];

fn stat_value(guild: &serenity::Guild, keyword: &str) -> u64 {
    match keyword {
        "MEMBERS" => guild.members.values().filter(|m| !m.user.bot).count() as u64,
        "BOTS" => guild.members.values().filter(|m| m.user.bot).count() as u64,
        "ROLES" => guild.roles.len() as u64,
        "BOOSTS" => guild.premium_subscription_count.unwrap_or(0),
        "CHANNELS" => guild.channels.len() as u64,
        _ => 0,
    }
}

fn to_small_caps(text: &str) -> String {
    text.chars()
        .map(|c| {
            UPPERCASE
                .chars()
                .position(|u| u == c)
                .and_then(|i| SMALL_CAPS.chars().nth(i))
                .unwrap_or(c)
        })
        .collect()
}

/// Converts normal and small-cap Unicode text to uppercase ASCII.
fn normalize_text(text: &str) -> String {
    let ascii: String = text
        .chars()
        .map(|c| match SMALL_CAPS.chars().position(|s| s == c) {
            Some(i) => UPPERCASE.chars().nth(i).unwrap(),
            None => c,
        })
        .collect();

    ascii
        .nfkd()
        .filter(|c| canonical_combining_class(*c) == 0)
        .collect::<String>()
        .to_uppercase()
}

pub async fn update_guild_channels(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
) {
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }

    // Work out renames from the cache first; cache references can't be held across awaits.
    let renames: Vec<(serenity::ChannelId, String)> = {
        let Some(guild) = ctx.cache.guild(guild_id) else {
            return;
        };
        guild
            .channels
            .values()
            .filter(|channel| channel.kind == serenity::ChannelType::Voice)
            .filter_map(|channel| {
                let normalized = normalize_text(&channel.name);
                let keyword = STAT_KEYWORDS.iter().find(|k| normalized.contains(*k))?;
                let new_name = format!(
                    "【📺】{}: {}",
                    to_small_caps(keyword),
                    stat_value(&guild, keyword)
                );
                (channel.name != new_name).then_some((channel.id, new_name))
            })
            .collect()
    };

    for (channel_id, new_name) in renames {
        let edit = serenity::EditChannel::new()
            .name(new_name)
            .audit_log_reason("Automatic server statistics update");
        // Missing permissions or rate limits are expected here; skip quietly.
        let _ = channel_id.edit(&ctx.http, edit).await;
    }
}

pub async fn update_stats_loop(ctx: serenity::Context, data: Data) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        for guild_id in ctx.cache.guilds() {
            update_guild_channels(&ctx, &data, guild_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_stat_channel_names() {
        assert_eq!(normalize_text("【📺】ᴍᴇᴍʙᴇʀs: 12"), "【📺】MEMBERS: 12");
        assert_eq!(normalize_text("Bøts café"), "BØTS CAFE");
        assert_eq!(to_small_caps("BOOSTS"), "ʙᴏᴏsᴛs");
    }
}
