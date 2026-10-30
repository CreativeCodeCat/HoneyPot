//! Gateway: opens the databases, builds the framework and connects to Discord.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use poise::serenity_prelude as serenity;

use super::{command_router, event_router, permissions};
use crate::database::guild_config;
use crate::features::developer;
use crate::features::{
    afk, applications, cooldowns, economy, games, honeypot, leveling, lockdown, pets, profile, rules, server_setup, staff_policy, sticky, suggestions, tickets,
    Data, State,
};
use crate::log;

pub async fn run() {
    dotenvy::dotenv().ok();
    let token = std::env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN is not set (add it to .env)");

    std::fs::create_dir_all(crate::database::connection::DIR).expect("failed to create database directory");
    let data: Data = Arc::new(State {
        start_time: Instant::now(),
        cog_config: Mutex::new(
            guild_config::CogConfig::open().expect("failed to open cog config database"),
        ),
        afk: Mutex::new(afk::Store::open().expect("failed to open AFK database")),
        applications: Mutex::new(applications::Store::open().expect("failed to open applications database")),
        blacklist: Mutex::new(developer::Blacklist::open().expect("failed to open blacklist database")),
        cooldowns: Mutex::new(cooldowns::Cooldowns::open().expect("failed to open cooldowns database")),
        counting: Mutex::new(games::counting::Store::open().expect("failed to open counting database")),
        guessing: Mutex::new(games::guessing::Store::open().expect("failed to open guessing database")),
        honeypot: Mutex::new(
            honeypot::store::Store::open().expect("failed to open honeypot database"),
        ),
        economy: Mutex::new(economy::store::Store::open().expect("failed to open economy database")),
        levels: Mutex::new(leveling::store::Store::open().expect("failed to open levels database")),
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
            commands: command_router::commands(),
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some("!".into()),
                ..Default::default()
            },
            command_check: Some(|ctx| Box::pin(permissions::command_check(ctx))),
            on_error: |error| Box::pin(command_router::on_error(error)),
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_router::event_handler(ctx, event, framework, data))
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
