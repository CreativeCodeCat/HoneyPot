//! RaidProtection cog: watches how fast members join. When too many join too quickly, the
//! server goes into raid mode: it alerts staff and, as set up with `/raid setup`, locks the
//! text channels (with the Lockdown cog's locks, so `/unlock` works on them too), raises the
//! verification level, and times out, kicks or bans everyone who joins until it's over.
//! Raid mode ends once joins slow down for a while, or with `/raid end`.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use ::serenity::all as serenity;
use chrono::Utc;

use crate::core::framework::{command, Choice, CreateReply};
use crate::features::{logging, Context, Data, Error};
use crate::log;

use super::lockdown;

pub use crate::database::repositories::raid_protection::*;

pub const COG_NAME: &str = "raidprotection";

const RED: u32 = 0xed4245;
const GREEN: u32 = 0x57f287;
const BLUE: u32 = 0x3498db;
/// Raid locks lift themselves after this long even if raid mode never ends
const LOCK_SAFETY_SECS: i64 = 24 * 60 * 60;
const AUDIT_REASON: &str = "Raid protection";

/// Recent joins per server: when, and who.
static JOINS: LazyLock<Mutex<HashMap<u64, VecDeque<(Instant, serenity::UserId)>>>> = LazyLock::new(Default::default);

/// What to do with accounts that join during a raid, as a command option.
#[derive(Clone, Copy, Debug, Choice)]
pub enum JoinerChoice {
    #[name = "Nothing"]
    Nothing,
    #[name = "Time out"]
    Timeout,
    #[name = "Kick"]
    Kick,
    #[name = "Ban"]
    Ban,
    #[name = "Quarantine"]
    Quarantine,
}

impl From<JoinerChoice> for JoinerAction {
    fn from(choice: JoinerChoice) -> Self {
        match choice {
            JoinerChoice::Nothing => Self::Nothing,
            JoinerChoice::Timeout => Self::Timeout,
            JoinerChoice::Kick => Self::Kick,
            JoinerChoice::Ban => Self::Ban,
            JoinerChoice::Quarantine => Self::Quarantine,
        }
    }
}

fn describe_action(settings: &Settings) -> String {
    match settings.joiner_action {
        JoinerAction::Nothing => "nothing".to_string(),
        JoinerAction::Timeout => format!("time out for {} minutes", settings.timeout_minutes),
        JoinerAction::Kick => "kick".to_string(),
        JoinerAction::Ban => "ban".to_string(),
        JoinerAction::Quarantine => "quarantine".to_string(),
    }
}

/// Where alerts go: the chosen channel, else the join & leave log, else the system channel.
fn alert_channel(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, settings: &Settings) -> Option<serenity::ChannelId> {
    settings
        .alert_channel
        .map(serenity::ChannelId::new)
        .or_else(|| logging::log_channel(data, guild_id, "joinleave"))
        .or_else(|| ctx.cache.guild(guild_id).and_then(|g| g.system_channel_id))
}

async fn alert(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, settings: &Settings, embed: serenity::CreateEmbed) {
    let Some(channel) = alert_channel(ctx, data, guild_id, settings) else { return };
    let message = serenity::CreateMessage::new().embed(embed.timestamp(serenity::Timestamp::now()));
    if let Err(e) = channel.send_message(ctx, message).await {
        log::warning(format!("Couldn't post a raid alert in guild {guild_id}: {e}"));
    }
}

/// Times out, kicks or bans an account that joined during a raid.
async fn act_on_joiner(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user_id: serenity::UserId, settings: &Settings) {
    let reason = format!("{AUDIT_REASON}: joined during a raid");
    let result = match settings.joiner_action {
        JoinerAction::Nothing => return,
        JoinerAction::Timeout => {
            let until = Utc::now() + chrono::Duration::minutes(settings.timeout_minutes as i64);
            let edit = serenity::EditMember::new().disable_communication_until_datetime(until.into()).audit_log_reason(&reason);
            guild_id.edit_member(ctx, user_id, edit).await.map(|_| ())
        }
        JoinerAction::Kick => guild_id.kick_with_reason(ctx, user_id, &reason).await,
        JoinerAction::Ban => guild_id.ban_with_reason(ctx, user_id, 0, &reason).await,
        JoinerAction::Quarantine => {
            if let Err(e) = super::quarantine::isolate_or_timeout(ctx, data, guild_id, user_id, "Joined during a raid", "Raid protection").await {
                log::warning(format!("Raid protection couldn't quarantine {user_id} in guild {guild_id}: {e}"));
            }
            return;
        }
    };
    if let Err(e) = result {
        log::warning(format!("Raid protection couldn't {} {user_id} in guild {guild_id}: {e}", settings.joiner_action.key()));
    }
}

/// Text channels @everyone can see, which raid mode locks. The alert channel stays open so
/// the alert can be posted, and honeypot channels so the trap keeps catching raiders.
fn channels_to_lock(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, settings: &Settings) -> Vec<serenity::ChannelId> {
    let mut keep_open: Vec<serenity::ChannelId> = alert_channel(ctx, data, guild_id, settings).into_iter().collect();
    if let Some(honeypot) = data.honeypot.lock().unwrap().guilds.get(&guild_id.get()) {
        keep_open.extend(honeypot.channels.iter().map(|c| serenity::ChannelId::new(c.channel_id)));
    }
    let Some(guild) = ctx.cache.guild(guild_id) else { return vec![] };
    let everyone = serenity::PermissionOverwriteType::Role(guild_id.everyone_role());
    guild
        .channels
        .values()
        .filter(|c| matches!(c.kind, serenity::ChannelType::Text | serenity::ChannelType::News))
        .filter(|c| !keep_open.contains(&c.id))
        .filter(|c| !c.permission_overwrites.iter().any(|o| o.kind == everyone && o.deny.view_channel()))
        .map(|c| c.id)
        .collect()
}

/// Puts the server in raid mode and acts on the accounts that set it off. `member` is the
/// join that tipped it over.
async fn start_raid(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    settings: &Settings,
    joiners: Vec<serenity::UserId>,
    member: serenity::UserId,
) {
    let now = Utc::now().timestamp();
    let started = data.raid_protection.lock().unwrap().start(guild_id.get(), now);
    match started {
        Ok(true) => {}
        // A join at the same moment started it: this joiner is just part of the raid
        Ok(false) => {
            if let Err(e) = data.raid_protection.lock().unwrap().joined(guild_id.get(), 1, now, true) {
                log::error(format!("Error counting a raid join in guild {guild_id}: {e}"));
            }
            return act_on_joiner(ctx, data, guild_id, member, settings).await;
        }
        Err(e) => return log::error(format!("Error starting raid mode in guild {guild_id}: {e}")),
    }
    log::warning(format!("Raid detected in guild {guild_id}: {} joins in {} seconds", joiners.len(), settings.seconds));

    let mut previous_verification = None;
    if settings.raise_verification {
        let current = ctx.cache.guild(guild_id).map(|g| g.verification_level);
        if let Some(current) = current.filter(|level| u8::from(*level) < u8::from(serenity::VerificationLevel::High)) {
            let edit = serenity::EditGuild::new().verification_level(serenity::VerificationLevel::High).audit_log_reason(AUDIT_REASON);
            match guild_id.edit(ctx, edit).await {
                Ok(_) => previous_verification = Some(u8::from(current)),
                Err(e) => log::warning(format!("Raid protection couldn't raise the verification level in guild {guild_id}: {e}")),
            }
        }
    }
    let mut locked = vec![];
    if settings.lock_channels {
        for channel_id in channels_to_lock(ctx, data, guild_id, settings) {
            match lockdown::lock_channel(ctx, data, guild_id, channel_id, now + LOCK_SAFETY_SECS, "raid protection").await {
                Ok(true) => locked.push(channel_id.get()),
                Ok(false) => {}
                Err(e) => log::warning(format!("Raid protection couldn't lock channel {channel_id}: {e}")),
            }
        }
    }
    if let Err(e) = data.raid_protection.lock().unwrap().set_changes(guild_id.get(), previous_verification, &locked) {
        log::error(format!("Error saving raid mode changes in guild {guild_id}: {e}"));
    }

    let mut done = vec![];
    if !locked.is_empty() {
        done.push(format!("🔒 Locked {} channels", locked.len()));
    }
    if previous_verification.is_some() {
        done.push("🛡️ Raised the verification level to High".to_string());
    }
    if settings.joiner_action != JoinerAction::Nothing {
        done.push(format!("👢 Every account that joins until it's over: {}", describe_action(settings)));
    }
    let text = format!(
        "**{} members joined in {} seconds.** The server is in raid mode.\n\n{}\n\nRaid mode ends after {} minutes without raid-speed joins, or with `/raid end`.",
        joiners.len(),
        settings.seconds,
        if done.is_empty() { "No automatic actions are set up.".to_string() } else { done.join("\n") },
        settings.quiet_minutes
    );
    alert(ctx, data, guild_id, settings, serenity::CreateEmbed::new().colour(RED).title("🚨 Raid detected").description(text)).await;

    if let Err(e) = data.raid_protection.lock().unwrap().joined(guild_id.get(), joiners.len() as u32, now, true) {
        log::error(format!("Error counting raid joins in guild {guild_id}: {e}"));
    }
    for user_id in joiners {
        act_on_joiner(ctx, data, guild_id, user_id, settings).await;
    }
}

/// Takes the server out of raid mode, unlocking what it locked and restoring the verification
/// level. `why` finishes "Raid mode ended ...". Returns false if it wasn't in raid mode.
pub async fn end_raid(http: &serenity::Context, data: &Data, guild_id: serenity::GuildId, why: &str) -> Result<bool, Error> {
    let Some(raid) = data.raid_protection.lock().unwrap().end(guild_id.get())? else { return Ok(false) };
    JOINS.lock().unwrap().remove(&guild_id.get());
    for channel_id in &raid.locked_channels {
        // Already unlocked with /unlock, or deleted, is fine
        if let Err(e) = lockdown::unlock_channel(&http.http, data, guild_id, serenity::ChannelId::new(*channel_id)).await {
            log::warning(format!("Raid protection couldn't unlock channel {channel_id}: {e}"));
        }
    }
    if let Some(level) = raid.previous_verification {
        let edit = serenity::EditGuild::new().verification_level(serenity::VerificationLevel::from(level)).audit_log_reason(AUDIT_REASON);
        if let Err(e) = guild_id.edit(http, edit).await {
            log::warning(format!("Raid protection couldn't restore the verification level in guild {guild_id}: {e}"));
        }
    }
    log::info(format!("Raid mode ended in guild {guild_id} ({why})"));
    let settings = data.raid_protection.lock().unwrap().settings(guild_id.get())?;
    let text = format!(
        "Raid mode ended {why}. {} accounts joined during it.{}{}",
        raid.joiners,
        if raid.locked_channels.is_empty() { "" } else { "\n🔓 Channels unlocked." },
        if raid.previous_verification.is_some() { "\n🛡️ Verification level put back." } else { "" },
    );
    alert(http, data, guild_id, &settings, serenity::CreateEmbed::new().colour(GREEN).title("✅ Raid over").description(text)).await;
    Ok(true)
}

/// Counts the join, and starts raid mode, or acts on the joiner during one.
pub async fn on_member_join(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    let guild_id = member.guild_id;
    if member.user.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let (settings, in_raid) = {
        let store = data.raid_protection.lock().unwrap();
        match store.settings(guild_id.get()).and_then(|s| Ok((s, store.raid(guild_id.get())?.is_some()))) {
            Ok(found) => found,
            Err(e) => return log::error(format!("Error reading raid protection for guild {guild_id}: {e}")),
        }
    };
    // Joins within the window, this one included
    let recent: Vec<serenity::UserId> = {
        let mut joins = JOINS.lock().unwrap();
        let window = joins.entry(guild_id.get()).or_default();
        let now = Instant::now();
        window.push_back((now, member.user.id));
        while window.front().is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(settings.seconds as u64)) {
            window.pop_front();
        }
        window.iter().map(|(_, user)| *user).collect()
    };
    let raid_speed = recent.len() >= settings.joins as usize;

    if in_raid {
        if let Err(e) = data.raid_protection.lock().unwrap().joined(guild_id.get(), 1, Utc::now().timestamp(), raid_speed) {
            log::error(format!("Error counting a raid join in guild {guild_id}: {e}"));
        }
        act_on_joiner(ctx, data, guild_id, member.user.id, &settings).await;
    } else if raid_speed {
        start_raid(ctx, data, guild_id, &settings, recent, member.user.id).await;
    }
}

/// Ends raid mode once joins have been slow for the quiet time. Runs whether or not the cog
/// is enabled, so disabling it never leaves a server locked down.
pub async fn quiet_loop(ctx: serenity::Context, data: Data) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let raids = match data.raid_protection.lock().unwrap().raids() {
            Ok(raids) => raids,
            Err(e) => {
                log::error(format!("Error checking raids: {e}"));
                continue;
            }
        };
        let now = Utc::now().timestamp();
        for raid in raids {
            let guild_id = serenity::GuildId::new(raid.guild_id);
            let quiet = match data.raid_protection.lock().unwrap().settings(raid.guild_id) {
                Ok(settings) => settings.quiet_minutes as i64 * 60,
                Err(_) => continue,
            };
            if now >= raid.last_activity + quiet {
                if let Err(e) = end_raid(&ctx, &data, guild_id, "after things went quiet").await {
                    log::error(format!("Error ending raid mode in guild {guild_id}: {e}"));
                }
            }
        }
    }
}

fn settings_embed(settings: &Settings, raid: Option<&Raid>) -> serenity::CreateEmbed {
    let yes_no = |on: bool| if on { "Yes" } else { "No" };
    let mut embed = serenity::CreateEmbed::new()
        .colour(if raid.is_some() { RED } else { BLUE })
        .title("Raid Protection")
        .field("Raid", format!("{} joins in {} seconds", settings.joins, settings.seconds), true)
        .field("Ends after", format!("{} quiet minutes", settings.quiet_minutes), true)
        .field("Joiners", describe_action(settings), true)
        .field("Lock channels", yes_no(settings.lock_channels), true)
        .field("Raise verification", yes_no(settings.raise_verification), true)
        .field("Alerts", settings.alert_channel.map_or_else(|| "Join & leave log, or system channel".to_string(), |c| format!("<#{c}>")), true);
    embed = match raid {
        Some(raid) => embed.field(
            "🚨 Raid mode is on",
            format!("Since <t:{}:R>, {} accounts joined. Last raid-speed join <t:{}:R>.", raid.started_at, raid.joiners, raid.last_activity),
            false,
        ),
        None => embed.field("Status", "No raid right now.", false),
    };
    embed
}

/// Raid protection: spots join raids and locks the server down
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "RaidProtection",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("setup", "status", "end"),
    subcommand_required
)]
pub async fn raid(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Change what counts as a raid and what happens during one
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
#[allow(clippy::too_many_arguments)]
async fn setup(
    ctx: Context<'_>,
    #[description = "Joins that count as a raid (default 10)"]
    #[min = 2]
    #[max = 100]
    joins: Option<u32>,
    #[description = "Within this many seconds (default 30)"]
    #[min = 5]
    #[max = 600]
    seconds: Option<u32>,
    #[description = "End raid mode after this many minutes without raid-speed joins (default 10)"]
    #[min = 1]
    #[max = 1440]
    quiet_minutes: Option<u32>,
    #[description = "Lock text channels during a raid (default yes)"] lock_channels: Option<bool>,
    #[description = "Raise the verification level to High during a raid (default yes)"] raise_verification: Option<bool>,
    #[description = "What to do with accounts that join during a raid (default kick)"] joiners: Option<JoinerChoice>,
    #[description = "How long to time joiners out for, in minutes (default 60)"]
    #[min = 1]
    #[max = 40320]
    timeout_minutes: Option<u32>,
    #[description = "Where to post raid alerts (default: join & leave log, or system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let (settings, raid) = {
        let store = ctx.data().raid_protection.lock().unwrap();
        let mut settings = store.settings(guild_id)?;
        settings.joins = joins.unwrap_or(settings.joins);
        settings.seconds = seconds.unwrap_or(settings.seconds);
        settings.quiet_minutes = quiet_minutes.unwrap_or(settings.quiet_minutes);
        settings.lock_channels = lock_channels.unwrap_or(settings.lock_channels);
        settings.raise_verification = raise_verification.unwrap_or(settings.raise_verification);
        settings.joiner_action = joiners.map_or(settings.joiner_action, JoinerAction::from);
        settings.timeout_minutes = timeout_minutes.unwrap_or(settings.timeout_minutes);
        settings.alert_channel = alert_channel.map(|c| c.id.get()).or(settings.alert_channel);
        store.set_settings(guild_id, &settings)?;
        (settings.clone(), store.raid(guild_id)?)
    };
    ctx.send(CreateReply::default().content("✅ Raid protection updated.").embed(settings_embed(&settings, raid.as_ref()))).await?;
    Ok(())
}

/// See the raid protection settings, and whether a raid is going on
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let (settings, raid) = {
        let store = ctx.data().raid_protection.lock().unwrap();
        (store.settings(guild_id)?, store.raid(guild_id)?)
    };
    ctx.send(CreateReply::default().embed(settings_embed(&settings, raid.as_ref()))).await?;
    Ok(())
}

/// End raid mode now, unlocking channels and putting the verification level back
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn end(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let why = format!("by {}", ctx.author().name);
    let text = if end_raid(ctx.serenity_context(), ctx.data(), ctx.guild_id().unwrap(), &why).await? {
        "✅ Raid mode ended."
    } else {
        "There's no raid going on."
    };
    ctx.say(text).await?;
    Ok(())
}
