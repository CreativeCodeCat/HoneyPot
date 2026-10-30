//! Command router: every command the bot registers, and how command errors are reported.

use crate::features::developer;
use crate::features::{
    afk, announce, applications, automod, cog_manager, cooldowns, economy, games, general, honeypot, info, leveling, lockdown, moderation, pets, profile,
    rules, server_setup, staff_policy, stats, sticky, suggestions, tickets, Context, Data, Error,
};
use crate::log;
use crate::features::presence;

pub fn commands() -> Vec<poise::Command<Data, Error>> {
    let mut commands = vec![
        general::ping(),
        general::cleanup(),
        general::resetchannel(),
        info::info(),
        afk::afk(),
        applications::application(),
        applications::apply(),
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
        leveling::rank_setup(),
        leveling::rank(),
        leveling::leaderboard(),
        leveling::reputation(),
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

async fn reply_ephemeral(ctx: Context<'_>, text: impl Into<String>) {
    let reply = poise::CreateReply::default().content(text).ephemeral(true);
    if let Err(e) = ctx.send(reply).await {
        log::error(format!("Failed to send error reply: {e}"));
    }
}

pub async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
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
            let cog = crate::features::command_category(ctx).unwrap_or_default().to_string();
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
