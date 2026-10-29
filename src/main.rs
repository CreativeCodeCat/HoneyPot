mod cogs;
mod developer;
mod log;
mod pagination;
mod presence;
mod render;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use poise::serenity_prelude as serenity;

use cogs::{
    announce, automod, cog_manager, cooldowns, economy, games, general, honeypot, info, levels, lockdown, logs, moderation, pets, profile, rules, normalize_cog_name, server_setup, staff_policy, stats, sticky, suggestions, tickets, Context, Data,
    Error, State, COG_MANAGER,
};

/// Background loops are spawned once, on the first CacheReady (it can fire again after reconnects).
static TASKS_STARTED: AtomicBool = AtomicBool::new(false);

fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut commands = vec![
        general::ping(),
        general::cleanup(),
        general::resetchannel(),
        info::info(),
        info::user_info_menu(),
        developer::leave(),
        developer::blacklist(),
        developer::maintenance_mode(),
        presence::presence(),
        automod::automod(),
        cog_manager::enablecog(),
        cog_manager::disablecog(),
        cog_manager::cogstatus(),
        cooldowns::cooldown(),
        server_setup::server_setup(),
        stats::updatestats(),
        lockdown::lock(),
        lockdown::unlock(),
        moderation::ban(),
        moderation::unban(),
        moderation::kick(),
        moderation::timeout(),
        moderation::untimeout(),
        staff_policy::staff_policy(),
        economy::eco(),
        rules::rules(),
        pets::pet(),
        games::hack(),
        games::heist(),
        games::defuse(),
        games::fight(),
        games::counting(),
        games::guessing(),
        games::enablegame(),
        games::disablegame(),
        games::gamestatus(),
        levels::rank_setup(),
        levels::rank(),
        levels::leaderboard(),
        levels::reputation(),
    ];
    commands.extend(honeypot::commands::all());
    commands.push(tickets::commands::ticket_setup());
    commands.push(tickets::commands::ticket());
    commands.push(suggestions::suggest());
    commands.push(suggestions::suggestion());
    commands.push(sticky::sticky());
    commands.push(announce::announce());
    commands.push(profile::profile());
    commands.push(profile::profile_menu());
    commands
}

/// Blocks commands whose cog is disabled in the current guild, then applies the command's cooldown.
async fn command_check(ctx: Context<'_>) -> Result<bool, Error> {
    // Owner-only commands belong to no cog and work in any channel
    if cogs::command_category(ctx) == Some(developer::CATEGORY) {
        return Ok(true);
    }
    if developer::is_user_blacklisted(ctx.data(), ctx.author().id) {
        return Err("🚫 You've been blacklisted from using this bot.".into());
    }
    if developer::maintenance() && !ctx.framework().options().owners.contains(&ctx.author().id) {
        return Err("🛠️ The bot is under maintenance right now. Please try again later.".into());
    }
    if let (Some(guild_id), Some(category)) = (ctx.guild_id(), cogs::command_category(ctx)) {
        let cog_name = normalize_cog_name(category);
        if cog_name != COG_MANAGER && !ctx.data().is_cog_enabled(guild_id.get(), &cog_name) {
            return Ok(false);
        }
        if cog_name == "games" && !ctx.data().cog_config.lock().unwrap().is_game_enabled(guild_id.get(), &cogs::root_command(ctx).name) {
            return Ok(false);
        }
    }
    server_setup::commands_channel_check(ctx).await?;
    cooldowns::check(ctx)?;
    Ok(true)
}

async fn reply_ephemeral(ctx: Context<'_>, text: impl Into<String>) {
    let reply = poise::CreateReply::default().content(text).ephemeral(true);
    if let Err(e) = ctx.send(reply).await {
        log::error(format!("Failed to send error reply: {e}"));
    }
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::MissingUserPermissions { ctx, .. } => {
            reply_ephemeral(
                ctx,
                "❌ You need administrator permissions to use this command.",
            )
            .await;
        }
        poise::FrameworkError::CooldownHit {
            remaining_cooldown,
            ctx,
            ..
        } => {
            let secs = remaining_cooldown.as_secs();
            reply_ephemeral(
                ctx,
                format!("⏱️ This command is on cooldown. Try again in {secs} seconds."),
            )
            .await;
        }
        poise::FrameworkError::CommandCheckFailed {
            error: None, ctx, ..
        } => {
            let cog = cogs::command_category(ctx).unwrap_or_default().to_string();
            reply_ephemeral(ctx, format!("❌ The {cog} cog is disabled on this server.")).await;
        }
        poise::FrameworkError::CommandCheckFailed {
            error: Some(error), ctx, ..
        } => {
            reply_ephemeral(ctx, error.to_string()).await;
        }
        poise::FrameworkError::Command { error, ctx, .. } => {
            log::error(format!(
                "Error in command '{}': {error}",
                ctx.command().name
            ));
            reply_ephemeral(ctx, format!("An error occurred: {error}")).await;
        }
        other => {
            if let Err(e) = poise::builtins::on_error(other).await {
                log::error(format!("Error while handling error: {e}"));
            }
        }
    }
}

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
    if let Err(e) = data.sticky.lock().unwrap().cleanup_stale(&guild_ids) {
        log::error(format!("Error cleaning up stale sticky messages: {e}"));
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
async fn startup_report(ctx: &serenity::Context, data: &Data, commands: &[poise::Command<Data, Error>]) {
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

async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    logs::on_event(ctx, data, event).await;
    match event {
        serenity::FullEvent::GuildAuditLogEntryCreate { entry, guild_id } => {
            staff_policy::on_audit_entry(ctx, data, *guild_id, entry).await;
        }
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            // Also runs for every server on startup
            developer::enforce_blacklist(ctx, data, guild).await;
            // Guilds the bot was already in are synced on startup by `startup_report`
            if *is_new == Some(true) {
                if let Err(e) = cog_manager::sync_guild_commands(&ctx.http, &framework.options().commands, data, guild.id).await {
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
        serenity::FullEvent::CacheReady { .. } => {
            cleanup_stale_data(ctx, data);
            presence::apply(ctx);
            if !TASKS_STARTED.swap(true, Ordering::SeqCst) {
                startup_report(ctx, data, &framework.options().commands).await;
                tokio::spawn(stats::update_stats_loop(ctx.clone(), data.clone()));
                tokio::spawn(honeypot::weekly_summary_loop(ctx.clone(), data.clone()));
                tokio::spawn(tickets::auto_close_loop(ctx.clone(), data.clone()));
                tokio::spawn(lockdown::expiry_loop(ctx.clone(), data.clone()));
                tokio::spawn(pets::needs_loop(ctx.clone(), data.clone()));
            }
        }
        serenity::FullEvent::Message { new_message } => {
            if server_setup::on_message(ctx, data, new_message, &framework.options().commands).await {
                return Ok(());
            }
            honeypot::on_message(ctx, data, new_message).await;
            tickets::on_message(data, new_message);
            levels::on_message(ctx, data, new_message).await;
            sticky::on_message(ctx, data, new_message).await;
            games::counting::on_message(ctx, data, new_message).await;
            games::guessing::on_message(ctx, data, new_message).await;
        }
        serenity::FullEvent::GuildMemberRemoval { guild_id, user, .. } => {
            tickets::on_member_leave(ctx, data, *guild_id, user).await;
        }
        serenity::FullEvent::InteractionCreate { interaction } => {
            tickets::on_interaction(ctx, data, interaction).await;
        }
        _ => {}
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let token = std::env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN is not set (add it to .env)");

    std::fs::create_dir_all("database").expect("failed to create database directory");
    let data: Data = Arc::new(State {
        start_time: Instant::now(),
        cog_config: Mutex::new(
            cog_manager::CogConfig::open().expect("failed to open cog config database"),
        ),
        blacklist: Mutex::new(developer::Blacklist::open().expect("failed to open blacklist database")),
        cooldowns: Mutex::new(cooldowns::Cooldowns::open().expect("failed to open cooldowns database")),
        counting: Mutex::new(games::counting::Store::open().expect("failed to open counting database")),
        guessing: Mutex::new(games::guessing::Store::open().expect("failed to open guessing database")),
        honeypot: Mutex::new(
            honeypot::store::Store::open().expect("failed to open honeypot database"),
        ),
        economy: Mutex::new(economy::store::Store::open().expect("failed to open economy database")),
        levels: Mutex::new(levels::store::Store::open().expect("failed to open levels database")),
        pets: Mutex::new(pets::store::Store::open().expect("failed to open pets database")),
        profiles: Mutex::new(profile::Store::open().expect("failed to open profiles database")),
        rules: Mutex::new(rules::Store::open().expect("failed to open rules database")),
        server_setup: Mutex::new(server_setup::Store::open().expect("failed to open server setup database")),
        staff_policy: Mutex::new(staff_policy::Store::open().expect("failed to open staff policy database")),
        sticky: Mutex::new(sticky::Store::open().expect("failed to open sticky database")),
        suggestions: Mutex::new(suggestions::Store::open().expect("failed to open suggestions database")),
        lockdown: Mutex::new(lockdown::Store::open().expect("failed to open lockdown database")),
        tickets: Mutex::new(
            tickets::store::Store::open().expect("failed to open tickets database"),
        ),
    });

    match data.economy.lock().unwrap().refund_held(None) {
        Ok(0) => {}
        Ok(refunds) => log::info(format!("Refunded {refunds} bet(s) from games interrupted by a restart")),
        Err(e) => log::error(format!("Error refunding bets from interrupted games: {e}")),
    }

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands(),
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some("!".into()),
                ..Default::default()
            },
            command_check: Some(|ctx| Box::pin(command_check(ctx))),
            on_error: |error| Box::pin(on_error(error)),
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(|_ctx, ready, _framework| {
            Box::pin(async move {
                log::info(format!("{} has connected to Discord!", ready.user.name));
                log::blue(format!("Connected to {} guilds", ready.guilds.len()));
                Ok(data)
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MEMBERS;

    // Recent messages are cached so the Logs cog can show what deleted and edited messages said
    let mut cache_settings = serenity::cache::Settings::default();
    cache_settings.max_messages = 200;
    let mut client = serenity::ClientBuilder::new(token, intents)
        .cache_settings(cache_settings)
        .framework(framework)
        .await
        .expect("failed to create client");

    if let Err(e) = client.start().await {
        log::error(format!("Client error: {e}"));
    }
}
