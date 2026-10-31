//! Command router: every command the bot registers, and how command errors are reported.

use crate::features::developer;
use crate::features::{
    afk, announce, applications, automod, cog_manager, cooldowns, economy, games, general, honeypot, offlimit, incidents, info, leveling, live_status, lockdown, moderation, pets, profile,
    quarantine, raid_protection, reaction_roles, rules, server_setup, staff_policy, stats, sticky, suggestions, suspicious_accounts, tickets, Context,
};
use crate::core::framework::{Command, CreateReply, FrameworkError};
use crate::log;
use crate::features::presence;

pub fn commands() -> Vec<Command> {
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
        crate::features::private_mode::private_mode(),
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
        games::games(),
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
    commands.push(offlimit::offlimit());
    commands.push(reaction_roles::reaction_roles());
    commands.push(incidents::incident());
    commands.push(raid_protection::raid());
    commands.push(quarantine::quarantine());
    commands.push(suspicious_accounts::suspicious());
    commands.push(live_status::live());
    commands.push(announce::announce());
    commands.push(profile::profile());
    commands.push(profile::profile_menu());
    commands
}

async fn reply_ephemeral(ctx: Context<'_>, text: impl Into<String>) {
    let reply = CreateReply::default().content(text).ephemeral(true);
    if let Err(e) = ctx.send(reply).await {
        log::error(format!("Failed to send error reply: {e}"));
    }
}

pub async fn on_error(error: FrameworkError<'_>) {
    match error {
        FrameworkError::MissingUserPermissions { ctx, missing } => {
            reply_ephemeral(ctx, format!("❌ You need these permissions to use this command: {missing}")).await;
        }
        FrameworkError::MissingBotPermissions { ctx, missing } => {
            reply_ephemeral(ctx, format!("❌ I need these permissions for this command: {missing}")).await;
        }
        FrameworkError::NotAnOwner { ctx } => {
            reply_ephemeral(ctx, "❌ Only the bot's owner can use this command.").await;
        }
        FrameworkError::GuildOnly { ctx } => {
            reply_ephemeral(ctx, "❌ This command only works in a server.").await;
        }
        FrameworkError::CommandCheckFailed { error: None, ctx } => {
            let cog = crate::features::command_category(ctx).unwrap_or_default().to_string();
            reply_ephemeral(ctx, format!("❌ The {cog} cog is disabled on this server.")).await;
        }
        FrameworkError::CommandCheckFailed { error: Some(error), ctx } => {
            reply_ephemeral(ctx, error.to_string()).await;
        }
        FrameworkError::Command { error, ctx } => {
            log::error(format!("Error in command '{}': {error}", ctx.command().qualified_name));
            reply_ephemeral(ctx, format!("An error occurred: {error}")).await;
        }
        FrameworkError::Panic { ctx } => {
            log::error(format!("Command '{}' panicked", ctx.command().qualified_name));
            reply_ephemeral(ctx, "An unexpected internal error has occurred.").await;
        }
    }
}
