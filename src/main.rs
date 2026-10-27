mod cogs;
mod log;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use poise::serenity_prelude as serenity;

use cogs::{
    automod, cog_manager, general, honeypot, normalize_cog_name, stats, tickets, Context, Data,
    Error, State, COG_MANAGER,
};

/// Background loops are spawned once, on the first CacheReady (it can fire again after reconnects).
static TASKS_STARTED: AtomicBool = AtomicBool::new(false);

fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut commands = vec![
        general::ping(),
        automod::automod(),
        cog_manager::enablecog(),
        cog_manager::disablecog(),
        cog_manager::cogstatus(),
        stats::updatestats(),
    ];
    commands.extend(honeypot::commands::all());
    commands.push(tickets::commands::ticket_setup());
    commands.push(tickets::commands::ticket());
    commands
}

/// Blocks commands whose cog is disabled in the current guild.
async fn cog_enabled_check(ctx: Context<'_>) -> Result<bool, Error> {
    let (Some(guild_id), Some(category)) = (ctx.guild_id(), ctx.command().category.as_deref())
    else {
        return Ok(true);
    };
    let cog_name = normalize_cog_name(category);
    Ok(cog_name == COG_MANAGER || ctx.data().is_cog_enabled(guild_id.get(), &cog_name))
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
            let cog = ctx.command().category.clone().unwrap_or_default();
            reply_ephemeral(ctx, format!("❌ The {cog} cog is disabled on this server.")).await;
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
    if let Err(e) = data.honeypot.lock().unwrap().cleanup_stale(&current) {
        log::error(format!("Error cleaning up stale data: {e}"));
    }
}

async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::GuildCreate { guild, is_new } => {
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
            if !TASKS_STARTED.swap(true, Ordering::SeqCst) {
                tokio::spawn(stats::update_stats_loop(ctx.clone(), data.clone()));
                tokio::spawn(honeypot::weekly_summary_loop(ctx.clone(), data.clone()));
                tokio::spawn(tickets::auto_close_loop(ctx.clone(), data.clone()));
            }
        }
        serenity::FullEvent::Message { new_message } => {
            honeypot::on_message(ctx, data, new_message).await;
            tickets::on_message(data, new_message);
        }
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            stats::update_guild_channels(ctx, data, new_member.guild_id).await;
        }
        serenity::FullEvent::GuildMemberRemoval { guild_id, user, .. } => {
            stats::update_guild_channels(ctx, data, *guild_id).await;
            tickets::on_member_leave(ctx, data, *guild_id, user).await;
        }
        serenity::FullEvent::ChannelCreate { channel } => {
            stats::update_guild_channels(ctx, data, channel.guild_id).await;
        }
        serenity::FullEvent::ChannelDelete { channel, .. } => {
            stats::update_guild_channels(ctx, data, channel.guild_id).await;
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
        honeypot: Mutex::new(
            honeypot::store::Store::open().expect("failed to open honeypot database"),
        ),
        tickets: Mutex::new(
            tickets::store::Store::open().expect("failed to open tickets database"),
        ),
    });

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands(),
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some("!".into()),
                ..Default::default()
            },
            command_check: Some(|ctx| Box::pin(cog_enabled_check(ctx))),
            on_error: |error| Box::pin(on_error(error)),
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(|ctx, ready, framework| {
            Box::pin(async move {
                log::info(format!("{} has connected to Discord!", ready.user.name));
                log::blue(format!("Connected to {} guilds", ready.guilds.len()));

                // Clear leftover per-guild commands; everything is registered globally
                for guild in &ready.guilds {
                    match guild.id.set_commands(ctx, vec![]).await {
                        Ok(_) => log::warning(format!("Cleared commands for guild: {}", guild.id)),
                        Err(e) => log::error(format!(
                            "Error clearing commands for guild {}: {e}",
                            guild.id
                        )),
                    }
                }

                let commands = &framework.options().commands;
                match poise::builtins::register_globally(ctx, commands).await {
                    Ok(()) => log::info(format!("Synced {} global command(s)", commands.len())),
                    Err(e) => log::error(format!("Error syncing global commands: {e}")),
                }
                Ok(data)
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MEMBERS;

    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .expect("failed to create client");

    if let Err(e) = client.start().await {
        log::error(format!("Client error: {e}"));
    }
}
