//! Event router: hands each gateway event to the features that care about it, and runs the
//! startup work (stale data cleanup, command sync, background loops) once the cache is ready.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use ::serenity::all as serenity;

use super::framework::{Command, Framework};

use crate::features::developer;
use crate::features::{
    afk, applications, cog_manager, games, honeypot, incidents, leveling, live_status, lockdown, logging, offlimit, pets, quarantine, raid_protection, reaction_roles, server_setup, staff_policy, stats, sticky, suspicious_accounts, tickets, Data, Error,
};
use crate::database::repositories::analytics;
use crate::log;
use crate::features::{presence, private_mode};

/// Background loops are spawned once, on the first CacheReady (it can fire again after reconnects).
static TASKS_STARTED: AtomicBool = AtomicBool::new(false);

fn cleanup_stale_data(ctx: &serenity::Context, data: &Data) {
    let guild_ids = ctx.cache.guilds();
    let current: HashMap<u64, HashSet<u64>> = guild_ids
        .iter()
        .filter_map(|id| {
            let guild = ctx.cache.guild(*id)?;
            Some((id.get(), guild.channels.keys().map(|c| c.get()).collect()))
        })
        .collect();

    data.cog_config.lock().unwrap().cleanup_stale(&guild_ids);
    data.cooldowns.lock().unwrap().cleanup_stale(&guild_ids);
    if let Err(e) = data.applications.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale applications: {e}"));
    }
    {
        let bot_messages = data.bot_messages.lock().unwrap();
        if let Err(e) = bot_messages.cleanup_stale(&guild_ids).and_then(|_| bot_messages.prune(chrono::Utc::now().timestamp())) {
            log::error(format!("Error cleaning up stale bot messages: {e}"));
        }
    }
    if let Err(e) = data.live_status.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale live status settings: {e}"));
    }
    if let Err(e) = data.suspicious_accounts.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale suspicious account data: {e}"));
    }
    if let Err(e) = data.quarantine.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale quarantine data: {e}"));
    }
    if let Err(e) = data.raid_protection.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale raid protection settings: {e}"));
    }
    if let Err(e) = data.incidents.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale incidents: {e}"));
    }
    if let Err(e) = data.afk.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale AFKs: {e}"));
    }
    match data.tickets.lock().unwrap().cleanup_stale(&current) {
        Ok(0) => {}
        Ok(count) => log::info(format!("[TICKETS] Removed {count} ticket(s) whose channel is gone")),
        Err(e) => log::error(format!("Error cleaning up stale tickets: {e}")),
    }
    if let Err(e) = data.honeypot.lock().unwrap().cleanup_stale(&current) {
        log::error(format!("Error cleaning up stale data: {e}"));
    }
    if let Err(e) = data.suggestions.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale suggestions: {e}"));
    }
    if let Err(e) = data.counting.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale counting games: {e}"));
    }
    if let Err(e) = data.guessing.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale guessing games: {e}"));
    }
    if let Err(e) = data.staff_policy.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale staff policy records: {e}"));
    }
    if let Err(e) = data.dashboard_log.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale dashboard actions: {e}"));
    }
    if let Err(e) = data.reaction_roles.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale reaction roles: {e}"));
    }
    if let Err(e) = data.sticky.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale sticky messages: {e}"));
    }
    if let Err(e) = data.analytics.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale analytics: {e}"));
    }
    if let Err(e) = data.offlimit.lock().unwrap().cleanup_stale(&current) {
        log::error(format!("Error cleaning up stale off-limits channels: {e}"));
    }
}

/// Once a minute: notes each server's member count and writes the day's counts.
async fn analytics_loop(ctx: serenity::Context, data: Data) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tick.tick().await;
        let counts: Vec<(u64, u64)> = ctx.cache.guilds().iter().filter_map(|id| ctx.cache.guild(*id).map(|g| (id.get(), g.member_count))).collect();
        let mut store = data.analytics.lock().unwrap();
        for (guild, members) in counts {
            store.snapshot(guild, analytics::MEMBERS, members as i64);
        }
        if let Err(e) = store.flush() {
            log::error(format!("Error saving analytics: {e}"));
        }
    }
}

/// Posts an embed in every server's ready log channel (`/server-setup logs Ready`).
async fn post_ready_log(ctx: &serenity::Context, data: &Data, description: String, author: Option<&str>) {
    let channels = match data.server_setup.lock().unwrap().everywhere("logs.ready") {
        Ok(channels) => channels,
        Err(e) => return log::error(format!("Error reading ready log channels: {e}")),
    };
    let bot = ctx.cache.current_user().clone();
    let year = chrono::Utc::now().format("%Y");
    let mut embed = serenity::CreateEmbed::new()
        .colour(0x2b2d31)
        .description(description)
        .footer(serenity::CreateEmbedFooter::new(format!("© {} {year}", bot.name)).icon_url(bot.face()))
        .timestamp(serenity::Timestamp::now());
    if let Some(author) = author {
        embed = embed.author(serenity::CreateEmbedAuthor::new(author).icon_url(bot.face()));
    }
    for (guild_id, channel_id) in channels {
        let message = serenity::CreateMessage::new().embed(embed.clone());
        if let Err(e) = serenity::ChannelId::new(channel_id).send_message(&ctx.http, message).await {
            log::warning(format!("Could not post the ready log in guild {guild_id}: {e}"));
        }
    }
}

/// On startup: announces the bot in ready log channels, then syncs the global commands and
/// every server's enabled-cog commands, reporting how many were registered.
async fn startup_report(ctx: &serenity::Context, data: &Data, commands: &[Command]) {
    let guild_ids = ctx.cache.guilds();
    let (members, channels) = guild_ids
        .iter()
        .filter_map(|id| ctx.cache.guild(*id).map(|g| (g.member_count, g.channels.len() as u64)))
        .fold((0, 0), |(m, c), (gm, gc)| (m + gm, c + gc));
    let stats = format!(
        "> Shard Count `{}`\n> Total Members `{members}`\n> Total Servers `{}`\n> Total Channels `{channels}`",
        ctx.cache.shard_count(),
        guild_ids.len()
    );
    let name = ctx.cache.current_user().name.clone();
    post_ready_log(ctx, data, stats, Some(&format!("{name} Has Started"))).await;
    post_ready_log(ctx, data, "Started refreshing application (/) commands.".to_string(), None).await;

    let global = match cog_manager::sync_global_commands(&ctx.http, commands).await {
        Ok(count) => count,
        Err(e) => {
            log::error(format!("Error syncing global commands: {e}"));
            0
        }
    };
    let mut guild = 0;
    for guild_id in &guild_ids {
        match cog_manager::sync_guild_commands(&ctx.http, commands, data, *guild_id).await {
            Ok(count) => guild += count,
            Err(e) => log::error(format!("Error syncing commands for guild {guild_id}: {e}")),
        }
    }
    log::info(format!("Synced {global} global and {guild} guild command(s)"));
    post_ready_log(ctx, data, format!("Successfully reloaded {global} application and {guild} guild (/) commands."), None).await;
}

pub async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: &Framework,
    data: &Data,
) -> Result<(), Error> {
    // Nothing at all happens in off-limits channels
    if offlimit::is_offlimit_event(data, event) {
        return Ok(());
    }
    logging::on_event(ctx, data, event).await;
    match event {
        serenity::FullEvent::GuildAuditLogEntryCreate { entry, guild_id } => {
            staff_policy::on_audit_entry(ctx, data, *guild_id, entry).await;
            incidents::on_audit_entry(ctx, data, *guild_id, entry).await;
        }
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            // Also runs for every server on startup. In private mode, servers not owned by a
            // bot owner are left at once, and nothing else happens for them
            if private_mode::enforce(ctx, &framework.owners, guild, *is_new == Some(true)).await {
                return Ok(());
            }
            developer::enforce_blacklist(ctx, data, guild).await;
            // Guilds the bot was already in are synced on startup by `startup_report`
            if *is_new == Some(true) {
                if let Err(e) = cog_manager::sync_guild_commands(&ctx.http, &framework.commands, data, guild.id).await {
                    log::error(format!("Error syncing commands for guild {}: {e}", guild.id));
                }
            }
            // Request the full member list so member counts and permission checks work
            ctx.shard.chunk_guild(
                guild.id,
                None,
                false,
                serenity::ChunkGuildFilter::None,
                None,
            );
            if *is_new == Some(true) {
                log::info(format!(
                    "Joined new guild: {} (ID: {})",
                    guild.name, guild.id
                ));
            }
        }
        serenity::FullEvent::GuildDelete { incomplete, full } => {
            if !incomplete.unavailable {
                let name = full.as_ref().map(|g| g.name.clone()).unwrap_or_default();
                log::warning(format!("Left guild: {name} (ID: {})", incomplete.id));
            }
        }
        serenity::FullEvent::Ready { data_about_bot } => {
            log::info(format!("{} has connected to Discord!", data_about_bot.user.name));
            log::blue(format!("Connected to {} guilds", data_about_bot.guilds.len()));
        }
        serenity::FullEvent::CacheReady { .. } => {
            cleanup_stale_data(ctx, data);
            presence::apply(ctx);
            if !TASKS_STARTED.swap(true, Ordering::SeqCst) {
                startup_report(ctx, data, &framework.commands).await;
                tokio::spawn(super::dashboard::serve(ctx.clone(), data.clone()));
                tokio::spawn(stats::update_stats_loop(ctx.clone(), data.clone()));
                tokio::spawn(honeypot::weekly_summary_loop(ctx.clone(), data.clone()));
                tokio::spawn(tickets::auto_close_loop(ctx.clone(), data.clone()));
                tokio::spawn(lockdown::expiry_loop(ctx.clone(), data.clone()));
                tokio::spawn(raid_protection::quiet_loop(ctx.clone(), data.clone()));
                tokio::spawn(live_status::live_loop(ctx.clone(), data.clone()));
                tokio::spawn(pets::needs_loop(ctx.clone(), data.clone()));
                tokio::spawn(analytics_loop(ctx.clone(), data.clone()));
            }
        }
        serenity::FullEvent::Message { new_message } => {
            // First, so a message deleted by the commands, counting or guessing channel still
            // takes its author out of AFK
            afk::on_message(ctx, data, new_message).await;
            if server_setup::on_message(ctx, data, new_message).await {
                return Ok(());
            }
            if let (Some(guild_id), false) = (new_message.guild_id, new_message.author.bot) {
                data.analytics.lock().unwrap().message(guild_id.get(), new_message.author.id.get());
            }
            honeypot::on_message(ctx, data, new_message).await;
            tickets::on_message(data, new_message);
            leveling::on_message(ctx, data, new_message).await;
            sticky::on_message(ctx, data, new_message).await;
            suspicious_accounts::on_message(ctx, data, new_message).await;
            games::counting::on_message(ctx, data, new_message).await;
            games::guessing::on_message(ctx, data, new_message).await;
        }
        serenity::FullEvent::ReactionAdd { add_reaction } => {
            reaction_roles::on_reaction(ctx, data, add_reaction, true).await;
        }
        serenity::FullEvent::ReactionRemove { removed_reaction } => {
            reaction_roles::on_reaction(ctx, data, removed_reaction, false).await;
        }
        serenity::FullEvent::MessageDelete { deleted_message_id, .. } => {
            reaction_roles::on_message_delete(data, *deleted_message_id);
        }
        serenity::FullEvent::ChannelCreate { channel } => {
            quarantine::on_channel_create(ctx, data, channel).await;
        }
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            data.analytics.lock().unwrap().bump(new_member.guild_id.get(), analytics::JOINS, 1);
            // Raid protection first: it may already deal with the joiner
            raid_protection::on_member_join(ctx, data, new_member).await;
            quarantine::on_member_join(ctx, data, new_member).await;
            suspicious_accounts::on_member_join(ctx, data, new_member).await;
        }
        serenity::FullEvent::GuildMemberRemoval { guild_id, user, member_data_if_available } => {
            data.analytics.lock().unwrap().bump(guild_id.get(), analytics::LEAVES, 1);
            incidents::on_member_leave(*guild_id, member_data_if_available.as_ref());
            tickets::on_member_leave(ctx, data, *guild_id, user).await;
        }
        serenity::FullEvent::InteractionCreate { interaction } => {
            tickets::on_interaction(ctx, data, interaction).await;
            applications::on_interaction(ctx, data, interaction).await;
            quarantine::on_interaction(ctx, data, interaction).await;
        }
        _ => {}
    }
    Ok(())
}
