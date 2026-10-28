//! Admin commands for turning single games on and off. They're part of the Games
//! cog, so they're only available while it's enabled. The settings are kept in the
//! cog manager's config as "game:<name>".

use poise::serenity_prelude as serenity;

use super::super::cog_manager::{game_key, resync};
use super::super::{Context, Error};
use super::GAMES;

async fn autocomplete_game(_ctx: Context<'_>, partial: &str) -> Vec<String> {
    let partial = partial.to_lowercase();
    GAMES.iter().filter(|game| game.contains(&partial)).map(|game| game.to_string()).collect()
}

/// Turns one game on or off and replies with the result.
async fn set_game(ctx: Context<'_>, game: String, enabled: bool) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();
    let game = game.trim().trim_start_matches('/').to_lowercase();
    if !GAMES.contains(&game.as_str()) {
        ctx.say(format!("❌ Unknown game '{game}'. Games: {}.", GAMES.join(", "))).await?;
        return Ok(());
    }

    ctx.data().cog_config.lock().unwrap().set_enabled(guild_id, &game_key(&game), enabled)?;
    let warning = resync(ctx).await;
    let state = if enabled { "enabled" } else { "disabled" };
    ctx.say(format!("✅ Game '{game}' has been {state} for this server.{warning}")).await?;
    Ok(())
}

/// Enable a game for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "Games",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn enablegame(
    ctx: Context<'_>,
    #[description = "Game to enable"]
    #[autocomplete = "autocomplete_game"]
    game: String,
) -> Result<(), Error> {
    set_game(ctx, game, true).await
}

/// Disable a game for this server, leaving the other games on
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "Games",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn disablegame(
    ctx: Context<'_>,
    #[description = "Game to disable"]
    #[autocomplete = "autocomplete_game"]
    game: String,
) -> Result<(), Error> {
    set_game(ctx, game, false).await
}

/// Check which games are enabled for this server
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "Games",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn gamestatus(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let status_msg = {
        let config = ctx.data().cog_config.lock().unwrap();
        GAMES
            .iter()
            .map(|game| {
                let status = if config.is_game_enabled(guild_id, game) { "✅ Enabled" } else { "❌ Disabled" };
                format!("{status}: {game}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = serenity::CreateEmbed::new()
        .title("Game Status")
        .description(status_msg)
        .colour(0x3498db);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
