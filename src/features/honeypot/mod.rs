//! Honeypot cog: detects and punishes messages sent in designated trap channels.

pub mod commands;

pub use crate::database::repositories::honeypot as store;

use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use ::serenity::all as serenity;
use regex::Regex;
use serde_json::{json, Map, Value};

use super::{parse_iso, truncate, Data};
use crate::log;
use store::ChannelConfig;

pub const COG_NAME: &str = "honeypot";

const PUNISH_REASON: &str = "Sent message in honeypot channel";
const DEFAULT_MESSAGE: &str = "**DO NOT SEND MESSAGES IN THIS CHANNEL**\n\nThis channel is used to catch spam bots. Any messages sent here will result in a softban.";

pub fn honeypot_embed(custom_message: Option<&str>) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .description(custom_message.unwrap_or(DEFAULT_MESSAGE))
        .colour(0x2b2d31)
}

/// Checks message content for suspicious patterns. All are case-insensitive except
/// `spam_caps`, which is about capitals.
fn check_suspicious_patterns(content: &str) -> Map<String, Value> {
    static PATTERNS: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            ("urls", r"https?://[^\s]+"),
            ("mentions", r"<@!?[0-9]+>"),
            (
                "invite_links",
                r"(discord\.gg/[^\s]+|discord\.com/invite/[^\s]+)",
            ),
            ("spam_caps", r"[A-Z]{5,}"),
        ]
        .into_iter()
        .map(|(name, pattern)| {
            let flags = if name == "spam_caps" { "" } else { "(?i)" };
            (name, Regex::new(&format!("{flags}{pattern}")).unwrap())
        })
        .collect()
    });

    let mut detected = Map::new();
    for (name, regex) in patterns {
        if regex.is_match(content) {
            detected.insert(name.to_string(), Value::Bool(true));
        }
    }

    // The regex crate has no backreferences, so `(.)\1{4,}` is checked by hand.
    let lower: Vec<char> = content.to_lowercase().chars().collect();
    if lower.windows(5).any(|w| w.iter().all(|c| *c == w[0])) {
        detected.insert("repeated_chars".to_string(), Value::Bool(true));
    }
    detected
}

fn is_whitelisted(user_id: u64, role_ids: &[serenity::RoleId], config: &ChannelConfig) -> bool {
    config.whitelisted_users.contains(&user_id)
        || role_ids
            .iter()
            .any(|r| config.whitelisted_roles.contains(&r.get()))
}

fn passes_account_age_check(user_id: serenity::UserId, config: &ChannelConfig) -> bool {
    if config.min_account_age_days == 0 {
        return true;
    }
    let age_days = (Utc::now().timestamp() - user_id.created_at().unix_timestamp()) / 86400;
    age_days >= config.min_account_age_days
}

async fn notify_admins(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    msg: &serenity::Message,
) {
    let Some(guild_data) = data
        .honeypot
        .lock()
        .unwrap()
        .guilds
        .get(&guild_id.get())
        .cloned()
    else {
        return;
    };
    let guild_name = ctx
        .cache
        .guild(guild_id)
        .map(|g| g.name.clone())
        .unwrap_or_default();
    let offender = format!("{} ({})", msg.author.tag(), msg.author.id);

    if guild_data.notification_dm {
        let text = format!(
            "🚨 **Honeypot Triggered in {guild_name}**\nUser: {offender}\nMessage: {}",
            truncate(&msg.content, 100)
        );
        for admin_id in &guild_data.admin_user_ids {
            let dm = serenity::CreateMessage::new().content(&text);
            if let Err(e) = serenity::UserId::new(*admin_id)
                .direct_message(ctx, dm)
                .await
            {
                log::error(format!("Failed to DM admin {admin_id}: {e}"));
            }
        }
    }

    if let Some(channel_id) = guild_data.notification_channel_id {
        let embed = serenity::CreateEmbed::new()
            .title("🚨 Honeypot Triggered")
            .colour(0xe74c3c)
            .field("User", offender, false)
            .field("Channel", format!("<#{}>", msg.channel_id), false)
            .field("Message", truncate(&msg.content, 200), false);
        let message = serenity::CreateMessage::new().embed(embed);
        if let Err(e) = serenity::ChannelId::new(channel_id)
            .send_message(&ctx.http, message)
            .await
        {
            log::error(format!("Failed to send notification to channel: {e}"));
        }
    }
}

pub async fn on_message(ctx: &serenity::Context, data: &Data, msg: &serenity::Message) {
    let Some(guild_id) = msg.guild_id else { return };
    if msg.author.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }

    let config = {
        let honeypot = data.honeypot.lock().unwrap();
        let Some(guild) = honeypot.guilds.get(&guild_id.get()) else {
            return;
        };
        let Some(config) = guild
            .channels
            .iter()
            .find(|c| c.enabled && c.channel_id == msg.channel_id.get())
        else {
            return;
        };
        config.clone()
    };

    let author = &msg.author;
    let username = author.tag();
    let role_ids = msg
        .member
        .as_ref()
        .map(|m| m.roles.clone())
        .unwrap_or_default();

    if is_whitelisted(author.id.get(), &role_ids, &config) {
        log::warning(format!("[HONEYPOT] {username} is whitelisted, skipping"));
        return;
    }

    if !passes_account_age_check(author.id, &config) {
        // Still proceeds with punishment
        log::warning(format!(
            "[HONEYPOT] {username} account too young, taking action"
        ));
    }

    let patterns_detected = if config.check_patterns {
        check_suspicious_patterns(&msg.content)
    } else {
        Map::new()
    };

    let previous_bans = {
        let honeypot = data.honeypot.lock().unwrap();
        let details = json!({
            "content": msg.content,
            "channel_id": msg.channel_id.get(),
            "patterns_detected": patterns_detected,
        });
        if let Err(e) = honeypot.log_activity(
            guild_id.get(),
            author.id.get(),
            &username,
            "message_in_honeypot",
            details,
        ) {
            log::error(format!("Error logging activity: {e}"));
        }
        if let Err(e) = honeypot.add_offender(author.id.get(), &username, guild_id.get()) {
            log::error(format!("Error adding to offender list: {e}"));
        }
        honeypot
            .previous_ban_guilds(author.id.get())
            .unwrap_or_else(|e| {
                log::warning(format!("Error checking known malicious: {e}"));
                vec![]
            })
    };

    if let Err(e) = msg.delete(ctx).await {
        log::warning(format!("[HONEYPOT] Failed to delete message: {e}"));
    }

    if !previous_bans.is_empty() {
        let sources: Vec<String> = previous_bans
            .iter()
            .map(|g| format!("Previously banned in guild {g}"))
            .collect();
        log::error(format!(
            "[HONEYPOT] {username} found in malicious databases: {}",
            sources.join(", ")
        ));
        let honeypot = data.honeypot.lock().unwrap();
        if let Err(e) = honeypot.log_activity(
            guild_id.get(),
            author.id.get(),
            &username,
            "known_malicious",
            json!({ "sources": sources }),
        ) {
            log::error(format!("Error logging activity: {e}"));
        }
    }

    notify_admins(ctx, data, guild_id, msg).await;

    if config.auto_ban {
        punish(ctx, data, guild_id, msg, &config).await;
    }

    log::debug(format!(
        "[HONEYPOT] {username} sent message in honeypot channel: {}...",
        truncate(&msg.content, 50)
    ));
}

async fn punish(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    msg: &serenity::Message,
    config: &ChannelConfig,
) {
    let author = &msg.author;
    let username = author.tag();

    let (author_is_admin, bot_permissions) = {
        let Some(guild) = ctx.cache.guild(guild_id) else {
            return;
        };
        let author_is_admin = msg.member.as_ref().is_some_and(|m| {
            guild
                .partial_member_permissions(author.id, m)
                .administrator()
        });
        let bot_id = ctx.cache.current_user().id;
        let bot_permissions = guild
            .members
            .get(&bot_id)
            .map(|m| guild.member_permissions(m))
            .unwrap_or_default();
        (author_is_admin, bot_permissions)
    };

    if author_is_admin {
        log::warning(format!(
            "[HONEYPOT] Skipping punishment for {username} - user is administrator"
        ));
        return;
    }

    let log_punishment = |action: &str, mut details: Value| {
        details["reason"] = json!(PUNISH_REASON);
        details["content"] = json!(msg.content);
        if let Err(e) = data.honeypot.lock().unwrap().log_activity(
            guild_id.get(),
            author.id.get(),
            &username,
            action,
            details,
        ) {
            log::error(format!("Error logging activity: {e}"));
        }
    };

    let result = match config.punishment_type.as_str() {
        "ban" if bot_permissions.ban_members() => guild_id
            .ban_with_reason(&ctx.http, author.id, 7, PUNISH_REASON)
            .await
            .map(|_| {
                {
                    let mut honeypot = data.honeypot.lock().unwrap();
                    if let Some(guild) = honeypot.guilds.get_mut(&guild_id.get()) {
                        guild.ban_count += 1;
                    }
                    if let Err(e) = honeypot.save_guild(guild_id.get()) {
                        log::error(format!("Error saving honeypot data: {e}"));
                    }
                }
                log_punishment("user_banned", json!({}));
                log::info(format!(
                    "[HONEYPOT] Banned {username} for sending message in honeypot channel"
                ));
            }),
        "kick" if bot_permissions.kick_members() => guild_id
            .kick_with_reason(&ctx.http, author.id, PUNISH_REASON)
            .await
            .map(|_| {
                log_punishment("user_kicked", json!({}));
                log::info(format!(
                    "[HONEYPOT] Kicked {username} for sending message in honeypot channel"
                ));
            }),
        "timeout" if bot_permissions.moderate_members() => {
            let minutes = if config.punishment_duration > 0 {
                config.punishment_duration
            } else {
                60
            };
            let until =
                serenity::Timestamp::from_unix_timestamp(Utc::now().timestamp() + minutes * 60)
                    .unwrap();
            let edit = serenity::EditMember::new()
                .disable_communication_until_datetime(until)
                .audit_log_reason(PUNISH_REASON);
            guild_id
                .edit_member(&ctx.http, author.id, edit)
                .await
                .map(|_| {
                    log_punishment("user_timed_out", json!({ "duration_minutes": minutes }));
                    log::info(format!(
                        "[HONEYPOT] Timed out {username} for sending message in honeypot channel"
                    ));
                })
        }
        "quarantine" if bot_permissions.manage_roles() => {
            match crate::features::quarantine::isolate_or_timeout(ctx, data, guild_id, author.id, PUNISH_REASON, "Honeypot").await {
                Ok(()) => {
                    log_punishment("user_quarantined", json!({}));
                    log::info(format!("[HONEYPOT] Quarantined {username} for sending message in honeypot channel"));
                }
                Err(e) => log::error(format!("[HONEYPOT] Error quarantining {username}: {e}")),
            }
            Ok(())
        }
        "ban" | "kick" | "timeout" | "quarantine" => {
            log::warning(format!(
                "[HONEYPOT] Bot lacks {} permissions",
                config.punishment_type
            ));
            Ok(())
        }
        other => {
            log::warning(format!("[HONEYPOT] Unknown punishment type '{other}'"));
            Ok(())
        }
    };

    if let Err(e) = result {
        log::error(format!("[HONEYPOT] Error applying punishment: {e}"));
    }
}

async fn send_weekly_summary(ctx: &serenity::Context, data: &Data) {
    let guilds: Vec<(u64, store::GuildData)> = data
        .honeypot
        .lock()
        .unwrap()
        .guilds
        .iter()
        .map(|(id, g)| (*id, g.clone()))
        .collect();
    let one_week_ago = Utc::now() - chrono::Duration::days(7);

    for (guild_id, guild_data) in guilds {
        if !data.is_cog_enabled(guild_id, COG_NAME) {
            continue;
        }
        let Some(guild_name) = ctx.cache.guild(guild_id).map(|g| g.name.clone()) else {
            continue;
        };

        let logs = match data
            .honeypot
            .lock()
            .unwrap()
            .activity_logs(guild_id, None, None)
        {
            Ok(logs) => logs,
            Err(e) => {
                log::error(format!(
                    "Error sending weekly summary for guild {guild_id}: {e}"
                ));
                continue;
            }
        };
        let recent: Vec<_> = logs
            .iter()
            .filter(|l| parse_iso(&l.timestamp).is_some_and(|t| t > one_week_ago))
            .collect();
        if recent.is_empty() {
            continue;
        }

        let count = |action: &str| recent.iter().filter(|l| l.action == action).count();
        let embed = serenity::CreateEmbed::new()
            .title("📊 Weekly Honeypot Summary")
            .colour(0x3498db)
            .field("Total Triggers", recent.len().to_string(), true)
            .field("Bans", count("user_banned").to_string(), true)
            .field("Kicks", count("user_kicked").to_string(), true)
            .field("Timeouts", count("user_timed_out").to_string(), true)
            .field(
                "Total Bans (All Time)",
                guild_data.ban_count.to_string(),
                true,
            )
            .field(
                "Active Honeypots",
                guild_data.channels.len().to_string(),
                true,
            );

        if let Some(channel_id) = guild_data.notification_channel_id {
            let message = serenity::CreateMessage::new().embed(embed.clone());
            if let Err(e) = serenity::ChannelId::new(channel_id)
                .send_message(&ctx.http, message)
                .await
            {
                log::error(format!(
                    "Error sending weekly summary for guild {guild_id}: {e}"
                ));
            }
        }

        if guild_data.notification_dm {
            for admin_id in &guild_data.admin_user_ids {
                let dm = serenity::CreateMessage::new()
                    .content(format!("📊 Weekly Honeypot Summary for {guild_name}"))
                    .embed(embed.clone());
                let _ = serenity::UserId::new(*admin_id)
                    .direct_message(ctx, dm)
                    .await;
            }
        }
    }
}

pub async fn weekly_summary_loop(ctx: serenity::Context, data: Data) {
    let mut interval = tokio::time::interval(Duration::from_secs(7 * 24 * 60 * 60));
    loop {
        interval.tick().await;
        send_weekly_summary(&ctx, &data).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_suspicious_patterns() {
        let found =
            check_suspicious_patterns("join discord.gg/abc at https://x.y <@123> HELLO aaaaa");
        for key in [
            "urls",
            "mentions",
            "invite_links",
            "spam_caps",
            "repeated_chars",
        ] {
            assert!(found.contains_key(key), "missing {key}");
        }
        // Only capitals count as shouting, not any long word
        assert!(!check_suspicious_patterns("hello there everyone").contains_key("spam_caps"));
        assert!(check_suspicious_patterns("QUIET please").contains_key("spam_caps"));
        assert!(check_suspicious_patterns("hi").is_empty());
        assert!(check_suspicious_patterns("AaAaA b").contains_key("repeated_chars"));
    }
}
