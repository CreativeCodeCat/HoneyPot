//! Permissions: the check every command passes before it runs — blacklist, maintenance
//! mode, whether its feature is enabled in this guild, off-limits channels, the commands
//! channel and cooldowns.

use crate::features::developer;
use crate::features::{command_category, root_command, cooldowns, offlimit, normalize_cog_name, server_setup, Context, Error, COG_MANAGER};

/// Blocks commands whose cog is disabled in the current guild, then applies the command's cooldown.
pub async fn command_check(ctx: Context<'_>) -> Result<bool, Error> {
    // Owner-only commands belong to no cog and work in any channel
    if command_category(ctx) == Some(developer::CATEGORY) {
        return Ok(true);
    }
    if developer::is_user_blacklisted(ctx.data(), ctx.author().id) {
        return Err("🚫 You've been blacklisted from using this bot.".into());
    }
    if developer::maintenance() && !ctx.framework().owners.contains(&ctx.author().id) {
        return Err("🛠️ The bot is under maintenance right now. Please try again later.".into());
    }
    offlimit::command_check(ctx).await?;
    if let (Some(guild_id), Some(category)) = (ctx.guild_id(), command_category(ctx)) {
        let cog_name = normalize_cog_name(category);
        if cog_name != COG_MANAGER && !ctx.data().is_cog_enabled(guild_id.get(), &cog_name) {
            return Ok(false);
        }
        if cog_name == "games" && !ctx.data().cog_config.lock().unwrap().is_game_enabled(guild_id.get(), &root_command(ctx).name) {
            return Ok(false);
        }
    }
    server_setup::commands_channel_check(ctx).await?;
    cooldowns::check(ctx)?;
    if let Some(guild_id) = ctx.guild_id() {
        use crate::database::repositories::analytics::{COMMANDS, COMMAND_PREFIX};
        let mut analytics = ctx.data().analytics.lock().unwrap();
        analytics.bump(guild_id.get(), COMMANDS, 1);
        analytics.bump(guild_id.get(), &format!("{COMMAND_PREFIX}{}", root_command(ctx).name), 1);
    }
    Ok(true)
}
