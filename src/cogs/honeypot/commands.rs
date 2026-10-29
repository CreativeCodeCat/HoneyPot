use poise::serenity_prelude as serenity;
use serde_json::{json, Value};

use super::honeypot_embed;
use super::store::ChannelConfig;
use crate::pagination::{field_pages, paginate};
use crate::cogs::{format_time, title_case, truncate, Context, Error};
use crate::log;

const NO_HONEYPOT: &str = "❌ No honeypot is configured for this server.";
const PUNISHMENTS: &[&str] = &["ban", "kick", "timeout"];

struct Template {
    name: &'static str,
    auto_ban: bool,
    punishment_type: &'static str,
    punishment_duration: i64,
    min_account_age_days: i64,
    check_patterns: bool,
    custom_message: &'static str,
}

const TEMPLATES: &[Template] = &[
    Template {
        name: "strict",
        auto_ban: true,
        punishment_type: "ban",
        punishment_duration: 0,
        min_account_age_days: 7,
        check_patterns: true,
        custom_message: "**⚠️ HIGH SECURITY ZONE**\n\nThis channel is a strict honeypot. Any message will result in an immediate ban.",
    },
    Template {
        name: "lenient",
        auto_ban: false,
        punishment_type: "timeout",
        punishment_duration: 60,
        min_account_age_days: 0,
        check_patterns: true,
        custom_message: "**🍯 Honeypot Channel**\n\nPlease do not send messages here. Messages will be logged.",
    },
    Template {
        name: "stealth",
        auto_ban: false,
        punishment_type: "timeout",
        punishment_duration: 10,
        min_account_age_days: 0,
        check_patterns: false,
        custom_message: "**Welcome!**\n\nFeel free to introduce yourself here.",
    },
    Template {
        name: "moderate",
        auto_ban: true,
        punishment_type: "kick",
        punishment_duration: 0,
        min_account_age_days: 3,
        check_patterns: true,
        custom_message: "**⚠️ Warning**\n\nThis is a monitored channel. Suspicious activity will result in a kick.",
    },
    Template {
        name: "newbie",
        auto_ban: true,
        punishment_type: "timeout",
        punishment_duration: 30,
        min_account_age_days: 1,
        check_patterns: true,
        custom_message: "**New User Check**\n\nThis channel helps us verify new accounts. Please wait for verification.",
    },
    Template {
        name: "investigation",
        auto_ban: false,
        punishment_type: "timeout",
        punishment_duration: 5,
        min_account_age_days: 0,
        check_patterns: true,
        custom_message: "**Rules & Guidelines**\n\nPlease read our rules before posting. Messages are monitored.",
    },
];

impl Template {
    fn apply(&self, config: &mut ChannelConfig) {
        config.auto_ban = self.auto_ban;
        config.punishment_type = self.punishment_type.to_string();
        config.punishment_duration = self.punishment_duration;
        config.min_account_age_days = self.min_account_age_days;
        config.check_patterns = self.check_patterns;
        config.custom_message = Some(self.custom_message.to_string());
    }
}

fn guild_id(ctx: Context<'_>) -> u64 {
    ctx.guild_id()
        .expect("honeypot commands are guild_only")
        .get()
}

fn target_channel(
    ctx: Context<'_>,
    channel: Option<serenity::GuildChannel>,
) -> serenity::ChannelId {
    channel.map(|c| c.id).unwrap_or(ctx.channel_id())
}

fn not_a_honeypot(channel_id: serenity::ChannelId) -> String {
    format!("❌ <#{channel_id}> is not a honeypot channel.")
}

fn audit(ctx: Context<'_>, action: &str, details: Value) {
    let honeypot = ctx.data().honeypot.lock().unwrap();
    if let Err(e) = honeypot.add_audit_log(guild_id(ctx), ctx.author().id.get(), action, details) {
        log::error(format!("Error adding audit log: {e}"));
    }
}

/// Applies `edit` to a honeypot channel's config and saves it. If the guild or channel
/// isn't configured, replies with the matching error and returns `None`.
async fn update_channel<T>(
    ctx: Context<'_>,
    channel_id: serenity::ChannelId,
    edit: impl FnOnce(&mut ChannelConfig) -> T,
) -> Result<Option<T>, Error> {
    let gid = guild_id(ctx);
    let result = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        match honeypot.guilds.get_mut(&gid) {
            None => Err(NO_HONEYPOT.to_string()),
            Some(guild) => match guild.channel_mut(channel_id.get()) {
                None => Err(not_a_honeypot(channel_id)),
                Some(config) => {
                    let value = edit(config);
                    honeypot.save_guild(gid)?;
                    Ok(value)
                }
            },
        }
    };

    match result {
        Ok(value) => Ok(Some(value)),
        Err(message) => {
            ctx.say(message).await?;
            Ok(None)
        }
    }
}

/// Edits the bot's honeypot warning among the channel's last 10 messages.
async fn update_honeypot_message(
    ctx: Context<'_>,
    channel_id: serenity::ChannelId,
    message: Option<&str>,
) -> Result<(), Error> {
    let bot_id = ctx.cache().current_user().id;
    let messages = channel_id
        .messages(ctx, serenity::GetMessages::new().limit(10))
        .await?;
    if let Some(mut warning) = messages.into_iter().find(|m| m.author.id == bot_id) {
        warning
            .edit(
                ctx,
                serenity::EditMessage::new().embed(honeypot_embed(message)),
            )
            .await?;
    }
    Ok(())
}

async fn send_honeypot_message(
    ctx: Context<'_>,
    channel_id: serenity::ChannelId,
    message: Option<&str>,
) -> Result<(), Error> {
    channel_id
        .send_message(
            ctx,
            serenity::CreateMessage::new().embed(honeypot_embed(message)),
        )
        .await?;
    Ok(())
}

/// Current display name of a guild member, if they're still in the server.
async fn member_display_name(ctx: Context<'_>, user_id: u64) -> Option<String> {
    if user_id == 0 {
        return None;
    }
    let member = ctx
        .guild_id()?
        .member(ctx, serenity::UserId::new(user_id))
        .await
        .ok()?;
    Some(member.display_name().to_string())
}

/// Name of a cached channel in any guild the bot is in.
#[allow(deprecated)] // Cache::channel is the only lookup that doesn't need the guild ID
fn cached_channel_name(ctx: Context<'_>, channel_id: u64) -> Option<String> {
    if channel_id == 0 {
        return None;
    }
    ctx.cache()
        .channel(serenity::ChannelId::new(channel_id))
        .map(|c| c.name.clone())
}

fn channel_name(ctx: Context<'_>, channel_id: Option<u64>) -> String {
    channel_id
        .and_then(|id| cached_channel_name(ctx, id))
        .unwrap_or_else(|| {
            format!(
                "ID: {}",
                channel_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "None".to_string())
            )
        })
}

fn user_name(ctx: Context<'_>, user_id: Option<u64>) -> String {
    user_id
        .filter(|id| *id != 0)
        .and_then(|id| {
            ctx.cache()
                .user(serenity::UserId::new(id))
                .map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone()))
        })
        .unwrap_or_else(|| {
            format!(
                "ID: {}",
                user_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "None".to_string())
            )
        })
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn format_audit_details(ctx: Context<'_>, action: &str, details: &Value) -> String {
    if details.as_object().is_none_or(|d| d.is_empty()) {
        return "No details".to_string();
    }
    let channel = || channel_name(ctx, details["channel_id"].as_u64());
    let user = || user_name(ctx, details["user_id"].as_u64());
    let status = || {
        if details["new_status"].as_bool().unwrap_or(false) {
            "Enabled"
        } else {
            "Disabled"
        }
    };

    match action {
        "set_honeypot" => format!(
            "Channel: #{}\nAuto-ban: {}",
            channel(),
            details["auto_ban"].as_bool().unwrap_or(false)
        ),
        "remove_honeypot" | "set_notification_channel" | "set_message" => {
            format!("Channel: #{}", channel())
        }
        "apply_template" => format!(
            "Template: {}\nChannel: #{}",
            value_text(&details["template"]),
            channel()
        ),
        "set_punishment" => {
            let duration = details["duration"].as_i64().unwrap_or(0);
            let duration = if duration > 0 {
                format!("{duration} min")
            } else {
                "N/A".to_string()
            };
            format!(
                "Channel: #{}\nPunishment: {}\nDuration: {duration}",
                channel(),
                value_text(&details["punishment"])
            )
        }
        "whitelist_user" | "unwhitelist_user" => {
            format!("User: {}\nChannel: #{}", user(), channel())
        }
        "set_account_age" => format!(
            "Channel: #{}\nMin Age: {} days",
            channel(),
            details["days"].as_i64().unwrap_or(0)
        ),
        "toggle_dm_notifications" => format!("New Status: {}", status()),
        "add_admin" | "remove_admin" => format!("User: {}", user()),
        "toggle_auto_ban" => format!("Channel: #{}\nNew Status: {}", channel(), status()),
        "clear_logs" => "Logs cleared".to_string(),
        "reset_ban_count" => "Ban counter reset".to_string(),
        "export_logs" => format!(
            "Logs exported ({} entries)",
            details["log_count"].as_i64().unwrap_or(0)
        ),
        _ => truncate(&details.to_string(), 200),
    }
}

async fn autocomplete_template(
    _ctx: Context<'_>,
    partial: &str,
) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    TEMPLATES
        .iter()
        .filter(|t| t.name.contains(&partial))
        .map(|t| serenity::AutocompleteChoice::new(title_case(t.name), t.name))
        .collect()
}

async fn autocomplete_punishment(
    _ctx: Context<'_>,
    partial: &str,
) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    PUNISHMENTS
        .iter()
        .filter(|p| p.contains(&partial))
        .map(|p| serenity::AutocompleteChoice::new(title_case(p), *p))
        .collect()
}

/// Set a channel as a honeypot for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn sethoneypot(
    ctx: Context<'_>,
    #[description = "Channel to turn into a honeypot (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
    #[description = "Punish users who post here"] auto_ban: Option<bool>,
    #[description = "Custom warning message (use \\n for new lines)"] message: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let channel_id = target_channel(ctx, channel);
    let auto_ban = auto_ban.unwrap_or(false);
    let message = message.map(|m| m.replace("\\n", "\n"));

    let added = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        let guild = honeypot.guilds.entry(gid).or_default();
        if guild.channel_mut(channel_id.get()).is_some() {
            false
        } else {
            let mut config = ChannelConfig::new(channel_id.get());
            config.auto_ban = auto_ban;
            config.custom_message = message.clone();
            guild.channels.push(config);
            honeypot.save_guild(gid)?;
            true
        }
    };

    if !added {
        ctx.say(format!("❌ <#{channel_id}> is already a honeypot channel."))
            .await?;
        return Ok(());
    }

    audit(
        ctx,
        "set_honeypot",
        json!({ "channel_id": channel_id.get(), "auto_ban": auto_ban }),
    );
    send_honeypot_message(ctx, channel_id, message.as_deref()).await?;

    let ban_status = if auto_ban {
        "with auto-ban enabled"
    } else {
        "without auto-ban"
    };
    ctx.say(format!("✅ Honeypot channel set to <#{channel_id}> {ban_status}. All messages in this channel will be logged."))
        .await?;
    Ok(())
}

/// Remove a honeypot channel for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn removehoneypot(
    ctx: Context<'_>,
    #[description = "Honeypot channel to remove (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let channel_id = target_channel(ctx, channel);

    let removed = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        match honeypot.guilds.get_mut(&gid) {
            None => None,
            Some(guild) => {
                let before = guild.channels.len();
                guild.channels.retain(|c| c.channel_id != channel_id.get());
                let removed = guild.channels.len() < before;
                if removed {
                    honeypot.save_guild(gid)?;
                }
                Some(removed)
            }
        }
    };

    match removed {
        None => {
            ctx.say("❌ No honeypot channels are set for this server.")
                .await?
        }
        Some(false) => ctx.say(not_a_honeypot(channel_id)).await?,
        Some(true) => {
            audit(
                ctx,
                "remove_honeypot",
                json!({ "channel_id": channel_id.get() }),
            );
            ctx.say(format!("✅ Honeypot channel <#{channel_id}> removed."))
                .await?
        }
    };
    Ok(())
}

/// Check the honeypot status for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot"
)]
pub async fn honeypotstatus(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild = ctx
        .data()
        .honeypot
        .lock()
        .unwrap()
        .guilds
        .get(&guild_id(ctx))
        .cloned();
    let Some(guild) = guild.filter(|g| !g.channels.is_empty()) else {
        ctx.say(NO_HONEYPOT).await?;
        return Ok(());
    };

    let mut embed = serenity::CreateEmbed::new()
        .title("🍯 Honeypot Status")
        .colour(0xe67e22);
    for (i, config) in guild.channels.iter().enumerate() {
        let name = match cached_channel_name(ctx, config.channel_id) {
            Some(channel) => format!("Channel {}: #{channel}", i + 1),
            None => format!("Channel {}: Unknown (ID: {})", i + 1, config.channel_id),
        };
        let status = if config.enabled {
            "✅ Enabled"
        } else {
            "❌ Disabled"
        };
        let auto_ban = if config.auto_ban { "✅" } else { "❌" };
        let punishment = title_case(&config.punishment_type);
        let duration = if config.punishment_duration > 0 && punishment == "Timeout" {
            format!(" ({} min)", config.punishment_duration)
        } else {
            String::new()
        };
        embed = embed.field(
            name,
            format!("Status: {status}\nAuto-ban: {auto_ban}\nPunishment: {punishment}{duration}"),
            false,
        );
    }
    embed = embed.field("Total Bans", guild.ban_count.to_string(), false);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// View recent honeypot activity logs
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn honeypotlogs(
    ctx: Context<'_>,
    #[description = "Number of entries to show, 5 per page (default 10, max 50)"]
    #[min = 1]
    #[max = 50]
    limit: Option<i64>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let limit = limit.unwrap_or(10).clamp(1, 50);
    let logs =
        ctx.data()
            .honeypot
            .lock()
            .unwrap()
            .activity_logs(guild_id(ctx), None, Some(limit))?;

    if logs.is_empty() {
        ctx.say("No recent activity to show.").await?;
        return Ok(());
    }

    let mut fields = vec![];
    for entry in &logs {
        let display_name = member_display_name(ctx, entry.user_id)
            .await
            .unwrap_or_else(|| entry.username.clone());

        let mut details_text = String::new();
        match entry.details.as_object() {
            Some(details) => {
                for (key, value) in details {
                    let line = match key.as_str() {
                        "content" => format!("**Content:** {}", value_text(value)),
                        "channel_id" => {
                            let name = value
                                .as_u64()
                                .and_then(|id| cached_channel_name(ctx, id))
                                .unwrap_or_else(|| format!("Unknown ({value})"));
                            format!("**Channel:** #{name}")
                        }
                        "channel_name" => format!("**Channel:** #{}", value_text(value)),
                        "reason" => format!("**Reason:** {}", value_text(value)),
                        _ => format!("**{}:** {}", title_case(key), value_text(value)),
                    };
                    details_text.push_str(&line);
                    details_text.push('\n');
                }
            }
            None => details_text = value_text(&entry.details),
        }

        let value = format!(
            "**Action:** {}\n{details_text}",
            title_case(&entry.action.replace('_', " "))
        );
        fields.push((
            truncate(&format!("{display_name} - {}", format_time(&entry.timestamp)), 256),
            truncate(&value, 1024),
        ));
    }

    let title = format!("Recent Honeypot Activity (Last {})", logs.len());
    let make_page = || serenity::CreateEmbed::new().title(&title).colour(0xe67e22);
    paginate(ctx, field_pages(make_page, fields, 5, true)).await
}

/// Clear honeypot activity logs for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn clearhoneypotlogs(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    ctx.data()
        .honeypot
        .lock()
        .unwrap()
        .clear_activity_logs(guild_id(ctx))?;
    audit(ctx, "clear_logs", json!({}));
    ctx.say("✅ Honeypot logs cleared for this server.").await?;
    Ok(())
}

/// Export honeypot logs to CSV
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn exportlogs(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let rows = ctx.data().honeypot.lock().unwrap().export_rows(gid)?;

    let mut writer = csv::Writer::from_writer(vec![]);
    writer.write_record(["timestamp", "user_id", "username", "action", "details"])?;
    for row in &rows {
        writer.write_record(row)?;
    }
    let bytes = writer.into_inner().map_err(|e| e.into_error())?;

    audit(ctx, "export_logs", json!({ "log_count": rows.len() }));

    let file = serenity::CreateAttachment::bytes(bytes, format!("honeypot_export_{gid}.csv"));
    ctx.send(poise::CreateReply::default().attachment(file))
        .await?;
    Ok(())
}

/// Reset the ban counter for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn resetbancount(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);

    let found = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        match honeypot.guilds.get_mut(&gid) {
            Some(guild) => {
                guild.ban_count = 0;
                honeypot.save_guild(gid)?;
                true
            }
            None => false,
        }
    };

    if !found {
        ctx.say(NO_HONEYPOT).await?;
        return Ok(());
    }
    audit(ctx, "reset_ban_count", json!({}));
    ctx.say("✅ Ban counter reset to 0.").await?;
    Ok(())
}

/// Toggle auto-ban for a honeypot channel
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn toggleautoban(
    ctx: Context<'_>,
    #[description = "Honeypot channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel_id = target_channel(ctx, channel);

    let Some(new_status) = update_channel(ctx, channel_id, |c| {
        c.auto_ban = !c.auto_ban;
        c.auto_ban
    })
    .await?
    else {
        return Ok(());
    };

    audit(
        ctx,
        "toggle_auto_ban",
        json!({ "channel_id": channel_id.get(), "new_status": new_status }),
    );
    let status_text = if new_status { "enabled" } else { "disabled" };
    ctx.say(format!(
        "✅ Auto-ban has been {status_text} for <#{channel_id}>."
    ))
    .await?;
    Ok(())
}

/// Check the current ban count for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn bancount(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild = ctx
        .data()
        .honeypot
        .lock()
        .unwrap()
        .guilds
        .get(&guild_id(ctx))
        .cloned();
    match guild {
        Some(guild) => {
            let auto_ban = if guild.channels.iter().any(|c| c.auto_ban) {
                "enabled"
            } else {
                "disabled"
            };
            ctx.say(format!(
                "🍯 Total bans: {}\nAuto-ban: {auto_ban}",
                guild.ban_count
            ))
            .await?
        }
        None => ctx.say(NO_HONEYPOT).await?,
    };
    Ok(())
}

/// Set a custom message for a honeypot channel
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn setmessage(
    ctx: Context<'_>,
    #[description = "Honeypot channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
    #[description = "Warning message (use \\n for new lines)"] message: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel_id = target_channel(ctx, channel);

    let Some(message) = message.map(|m| m.replace("\\n", "\n")) else {
        ctx.say("❌ Please provide a message.").await?;
        return Ok(());
    };

    if update_channel(ctx, channel_id, |c| {
        c.custom_message = Some(message.clone())
    })
    .await?
    .is_none()
    {
        return Ok(());
    }
    audit(
        ctx,
        "set_message",
        json!({ "channel_id": channel_id.get() }),
    );

    if let Err(e) = update_honeypot_message(ctx, channel_id, Some(&message)).await {
        log::error(format!("Error updating honeypot message: {e}"));
    }
    ctx.say("✅ Custom message updated.").await?;
    Ok(())
}

/// Set punishment type for a honeypot channel
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn setpunishment(
    ctx: Context<'_>,
    #[description = "Honeypot channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
    #[description = "ban, kick or timeout"]
    #[autocomplete = "autocomplete_punishment"]
    punishment: Option<String>,
    #[description = "Timeout length in minutes"] duration: Option<i64>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel_id = target_channel(ctx, channel);
    let duration = duration.unwrap_or(0);

    let Some(punishment) = punishment.map(|p| p.to_lowercase()) else {
        ctx.say("❌ Please specify a punishment type: ban, kick, or timeout")
            .await?;
        return Ok(());
    };
    if !PUNISHMENTS.contains(&punishment.as_str()) {
        ctx.say("❌ Invalid punishment type. Use: ban, kick, or timeout")
            .await?;
        return Ok(());
    }

    let updated = update_channel(ctx, channel_id, |c| {
        c.punishment_type = punishment.clone();
        c.punishment_duration = duration;
    })
    .await?;
    if updated.is_none() {
        return Ok(());
    }

    audit(
        ctx,
        "set_punishment",
        json!({ "channel_id": channel_id.get(), "punishment": punishment, "duration": duration }),
    );
    let duration_text = if punishment == "timeout" && duration > 0 {
        format!(" for {duration} minutes")
    } else {
        String::new()
    };
    ctx.say(format!(
        "✅ Punishment set to {punishment}{duration_text} for <#{channel_id}>."
    ))
    .await?;
    Ok(())
}

/// Whitelist a user from honeypot
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn whitelistuser(
    ctx: Context<'_>,
    #[description = "User to whitelist"] user: serenity::Member,
    #[description = "Honeypot channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel_id = target_channel(ctx, channel);
    let user_id = user.user.id;

    let Some(added) = update_channel(ctx, channel_id, |c| {
        if c.whitelisted_users.contains(&user_id.get()) {
            return false;
        }
        c.whitelisted_users.push(user_id.get());
        true
    })
    .await?
    else {
        return Ok(());
    };

    if added {
        audit(
            ctx,
            "whitelist_user",
            json!({ "user_id": user_id.get(), "channel_id": channel_id.get() }),
        );
        ctx.say(format!(
            "✅ <@{user_id}> has been whitelisted from <#{channel_id}>."
        ))
        .await?;
    } else {
        ctx.say(format!("❌ <@{user_id}> is already whitelisted."))
            .await?;
    }
    Ok(())
}

/// Remove user from honeypot whitelist
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn unwhitelistuser(
    ctx: Context<'_>,
    #[description = "User to remove from the whitelist"] user: serenity::Member,
    #[description = "Honeypot channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel_id = target_channel(ctx, channel);
    let user_id = user.user.id;

    let Some(removed) = update_channel(ctx, channel_id, |c| {
        let before = c.whitelisted_users.len();
        c.whitelisted_users.retain(|id| *id != user_id.get());
        c.whitelisted_users.len() < before
    })
    .await?
    else {
        return Ok(());
    };

    if removed {
        audit(
            ctx,
            "unwhitelist_user",
            json!({ "user_id": user_id.get(), "channel_id": channel_id.get() }),
        );
        ctx.say(format!("✅ <@{user_id}> has been removed from whitelist."))
            .await?;
    } else {
        ctx.say(format!("❌ <@{user_id}> is not whitelisted."))
            .await?;
    }
    Ok(())
}

/// Set minimum account age (in days) for honeypot
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn setaccountage(
    ctx: Context<'_>,
    #[description = "Honeypot channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
    #[description = "Minimum account age in days (0 = no requirement)"]
    #[min = 0]
    days: Option<i64>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let channel_id = target_channel(ctx, channel);
    let days = days.unwrap_or(0);

    if update_channel(ctx, channel_id, |c| c.min_account_age_days = days)
        .await?
        .is_none()
    {
        return Ok(());
    }
    audit(
        ctx,
        "set_account_age",
        json!({ "channel_id": channel_id.get(), "days": days }),
    );

    if days == 0 {
        ctx.say(format!(
            "✅ Account age requirement removed for <#{channel_id}>."
        ))
        .await?;
    } else {
        ctx.say(format!(
            "✅ Account age requirement set to {days} days for <#{channel_id}>."
        ))
        .await?;
    }
    Ok(())
}

/// Set channel for honeypot notifications
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn setadminchannel(
    ctx: Context<'_>,
    #[description = "Channel to receive notifications"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        honeypot
            .guilds
            .entry(gid)
            .or_default()
            .notification_channel_id = Some(channel.id.get());
        honeypot.save_guild(gid)?;
    }
    audit(
        ctx,
        "set_notification_channel",
        json!({ "channel_id": channel.id.get() }),
    );
    ctx.say(format!("✅ Notification channel set to <#{}>.", channel.id))
        .await?;
    Ok(())
}

/// Toggle DM notifications for admins
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn toggleadminnotifications(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let new_status = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        let guild = honeypot.guilds.entry(gid).or_default();
        guild.notification_dm = !guild.notification_dm;
        let new_status = guild.notification_dm;
        honeypot.save_guild(gid)?;
        new_status
    };
    audit(
        ctx,
        "toggle_dm_notifications",
        json!({ "new_status": new_status }),
    );
    ctx.say(format!(
        "✅ DM notifications {}.",
        if new_status { "enabled" } else { "disabled" }
    ))
    .await?;
    Ok(())
}

/// Add an admin to receive notifications
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn addadmin(
    ctx: Context<'_>,
    #[description = "User to notify"] user: serenity::Member,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let user_id = user.user.id;

    let added = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        let guild = honeypot.guilds.entry(gid).or_default();
        if guild.admin_user_ids.contains(&user_id.get()) {
            false
        } else {
            guild.admin_user_ids.push(user_id.get());
            honeypot.save_guild(gid)?;
            true
        }
    };

    if added {
        audit(ctx, "add_admin", json!({ "user_id": user_id.get() }));
        ctx.say(format!("✅ <@{user_id}> added to admin list."))
            .await?;
    } else {
        ctx.say(format!("❌ <@{user_id}> is already an admin."))
            .await?;
    }
    Ok(())
}

/// Remove an admin from notifications
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn removeadmin(
    ctx: Context<'_>,
    #[description = "User to stop notifying"] user: serenity::Member,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let user_id = user.user.id;

    let removed = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        match honeypot.guilds.get_mut(&gid) {
            Some(guild) if guild.admin_user_ids.contains(&user_id.get()) => {
                guild.admin_user_ids.retain(|id| *id != user_id.get());
                honeypot.save_guild(gid)?;
                true
            }
            _ => false,
        }
    };

    if removed {
        audit(ctx, "remove_admin", json!({ "user_id": user_id.get() }));
        ctx.say(format!("✅ <@{user_id}> removed from admin list."))
            .await?;
    } else {
        ctx.say(format!("❌ <@{user_id}> is not an admin.")).await?;
    }
    Ok(())
}

/// View honeypot audit log
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn auditlog(
    ctx: Context<'_>,
    #[description = "Number of entries to show, 10 per page (default 25, max 100)"]
    #[min = 1]
    #[max = 100]
    limit: Option<i64>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let limit = limit.unwrap_or(25).clamp(1, 100);
    let entries = ctx
        .data()
        .honeypot
        .lock()
        .unwrap()
        .audit_log(guild_id(ctx), limit)?;

    if entries.is_empty() {
        ctx.say("No audit log entries.").await?;
        return Ok(());
    }

    let mut fields = vec![];
    // Oldest first, like the Python bot
    for entry in entries.iter().rev() {
        let admin_name = match member_display_name(ctx, entry.admin_id).await {
            Some(name) => name,
            None => ctx
                .cache()
                .user(serenity::UserId::new(entry.admin_id.max(1)))
                .map(|u| u.name.clone())
                .unwrap_or_else(|| format!("ID: {}", entry.admin_id)),
        };
        let details = format_audit_details(ctx, &entry.action, &entry.details);
        fields.push((
            truncate(&format!("{admin_name} - {}", format_time(&entry.timestamp)), 256),
            truncate(&format!("**Action:** {}\n{details}", entry.action), 1024),
        ));
    }

    let title = format!("Audit Log (Last {})", entries.len());
    let make_page = || serenity::CreateEmbed::new().title(&title).colour(0x3498db);
    paginate(ctx, field_pages(make_page, fields, 10, false)).await
}

/// Check if a user is in the offender list
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn checkoffender(
    ctx: Context<'_>,
    #[description = "User to check"] user: serenity::Member,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let offender = ctx
        .data()
        .honeypot
        .lock()
        .unwrap()
        .offender(user.user.id.get())?;

    match offender {
        Some(offender) => {
            let embed = serenity::CreateEmbed::new()
                .title("⚠️ Offender Found")
                .colour(0xe74c3c)
                .field("Username", offender.username, false)
                .field("Offense Count", offender.offense_count.to_string(), false)
                .field("First Offense", offender.first_offense, false)
                .field("Last Offense", offender.last_offense, false)
                .field("Servers", offender.servers.len().to_string(), false);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        None => {
            ctx.say(format!(
                "✅ <@{}> is not in the offender list.",
                user.user.id
            ))
            .await?;
        }
    }
    Ok(())
}

/// Check a user's activity across all servers
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn crossservercheck(
    ctx: Context<'_>,
    #[description = "User to check"] user: serenity::Member,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let user_id = user.user.id;

    // (guild_id, logs newest first)
    let per_guild = {
        let honeypot = ctx.data().honeypot.lock().unwrap();
        let mut per_guild = vec![];
        for gid in honeypot.guilds_with_activity(user_id.get())? {
            let logs = honeypot.activity_logs(gid, Some(user_id.get()), None)?;
            if !logs.is_empty() {
                per_guild.push((gid, logs));
            }
        }
        per_guild
    };

    if per_guild.is_empty() {
        ctx.say(format!(
            "✅ No activity found for <@{user_id}> across all servers."
        ))
        .await?;
        return Ok(());
    }

    let mut fields = vec![];
    for (gid, logs) in &per_guild {
        let guild_name = ctx
            .cache()
            .guild(*gid)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| format!("ID: {gid}"));

        let mut actions: Vec<(String, usize)> = vec![];
        let mut channels: Vec<u64> = vec![];
        for entry in logs {
            match actions.iter_mut().find(|(a, _)| *a == entry.action) {
                Some((_, count)) => *count += 1,
                None => actions.push((entry.action.clone(), 1)),
            }
            if let Some(channel_id) = entry.details["channel_id"].as_u64() {
                if !channels.contains(&channel_id) {
                    channels.push(channel_id);
                }
            }
        }

        let action_summary: Vec<String> = actions
            .iter()
            .map(|(action, count)| format!("{}: {count}", title_case(&action.replace('_', " "))))
            .collect();
        let channel_names: Vec<String> = channels
            .iter()
            .map(|id| match cached_channel_name(ctx, *id) {
                Some(channel) => format!("#{channel}"),
                None => format!("ID: {id}"),
            })
            .collect();

        let mut value = format!(
            "Total Events: {}\nActions: {}\n",
            logs.len(),
            action_summary.join(", ")
        );
        if !channel_names.is_empty() {
            value += &format!(
                "Channels: {}",
                channel_names
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if channel_names.len() > 5 {
                value += &format!(" (+{} more)", channel_names.len() - 5);
            }
            value.push('\n');
        }
        value += &format!("Latest: {}", format_time(&logs[0].timestamp));

        fields.push((truncate(&format!("📊 {guild_name}"), 256), truncate(&value, 1024)));
    }

    // The summary covers every server, so it's on every page
    let total_events: usize = per_guild.iter().map(|(_, logs)| logs.len()).sum();
    let title = format!("🌍 Cross-Server Activity for {}", user.display_name());
    let summary = format!("📈 **Total Events:** {total_events} • **Servers:** {}", per_guild.len());
    let make_page = || serenity::CreateEmbed::new().title(&title).description(&summary).colour(0x3498db);
    paginate(ctx, field_pages(make_page, fields, 5, false)).await
}

/// Apply a pre-configured honeypot template
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "HoneyPot",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn applytemplate(
    ctx: Context<'_>,
    #[description = "strict, lenient, stealth, moderate, newbie or investigation"]
    #[autocomplete = "autocomplete_template"]
    template: String,
    #[description = "Channel (defaults to this one)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let channel_id = target_channel(ctx, channel);

    let template_name = template.to_lowercase();
    let Some(template) = TEMPLATES.iter().find(|t| t.name == template_name) else {
        ctx.say("❌ Invalid template. Use: strict, lenient, stealth, moderate, newbie, or investigation").await?;
        return Ok(());
    };

    let existed = {
        let mut honeypot = ctx.data().honeypot.lock().unwrap();
        let guild = honeypot.guilds.entry(gid).or_default();
        let existed = match guild.channel_mut(channel_id.get()) {
            Some(config) => {
                template.apply(config);
                true
            }
            None => {
                let mut config = ChannelConfig::new(channel_id.get());
                template.apply(&mut config);
                guild.channels.push(config);
                false
            }
        };
        honeypot.save_guild(gid)?;
        existed
    };

    audit(
        ctx,
        "apply_template",
        json!({ "template": template.name, "channel_id": channel_id.get() }),
    );

    if existed {
        let _ = update_honeypot_message(ctx, channel_id, Some(template.custom_message)).await;
    } else {
        send_honeypot_message(ctx, channel_id, Some(template.custom_message)).await?;
    }

    ctx.say(format!(
        "✅ Template '{}' applied to <#{channel_id}>.",
        template.name
    ))
    .await?;
    Ok(())
}

pub fn all() -> Vec<poise::Command<crate::cogs::Data, Error>> {
    vec![
        sethoneypot(),
        removehoneypot(),
        honeypotstatus(),
        honeypotlogs(),
        clearhoneypotlogs(),
        exportlogs(),
        resetbancount(),
        toggleautoban(),
        bancount(),
        setmessage(),
        setpunishment(),
        whitelistuser(),
        unwhitelistuser(),
        setaccountage(),
        setadminchannel(),
        toggleadminnotifications(),
        addadmin(),
        removeadmin(),
        auditlog(),
        checkoffender(),
        crossservercheck(),
        applytemplate(),
    ]
}
