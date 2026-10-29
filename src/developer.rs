//! Commands only the bot's owner can use: Discord tells the bot who owns it (or who's on the
//! team that does) when it starts. They belong to no cog, so they can't be disabled and work
//! in any channel. Also maintenance mode, when only the owner can use any command.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use poise::serenity_prelude as serenity;

use crate::cogs::{truncate, Context, Error};

/// The category these commands are filed under
pub const CATEGORY: &str = "Developer";
/// Exists while the bot is in maintenance mode, so it stays on across restarts
const MAINTENANCE_FILE: &str = "database/maintenance";

static MAINTENANCE: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(std::path::Path::new(MAINTENANCE_FILE).exists()));

/// Whether the bot is in maintenance mode, when only its owner can use commands.
pub fn maintenance() -> bool {
    MAINTENANCE.load(Ordering::Relaxed)
}

/// How long the confirm buttons wait
const CONFIRM_TIME: std::time::Duration = std::time::Duration::from_secs(60);

/// Suggests the servers the bot is in, by name, with their ID as the value.
async fn autocomplete_server(ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    let mut servers: Vec<(String, serenity::GuildId)> = ctx
        .cache()
        .guilds()
        .into_iter()
        .filter_map(|id| ctx.cache().guild(id).map(|g| (g.name.clone(), id)))
        .filter(|(name, id)| name.to_lowercase().contains(&partial) || id.to_string().contains(&partial))
        .collect();
    servers.sort();
    servers
        .into_iter()
        .take(25)
        .map(|(name, id)| serenity::AutocompleteChoice::new(truncate(&format!("{name} ({id})"), 100), id.to_string()))
        .collect()
}

/// Make the bot leave a server (bot owner only)
#[poise::command(
    slash_command,
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "The server to leave"]
    #[autocomplete = "autocomplete_server"]
    server: String,
) -> Result<(), Error> {
    let name = server
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .map(serenity::GuildId::new)
        .and_then(|id| ctx.cache().guild(id).map(|g| (id, g.name.clone())));
    let Some((guild_id, name)) = name else {
        ctx.say("❌ I'm not in that server. Pick one from the list.").await?;
        return Ok(());
    };

    let (yes_id, no_id) = (format!("{}remove_yes", ctx.id()), format!("{}remove_no", ctx.id()));
    let buttons = |enabled: bool| {
        vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&yes_id).label("Leave").style(serenity::ButtonStyle::Danger).disabled(!enabled),
            serenity::CreateButton::new(&no_id).label("Cancel").style(serenity::ButtonStyle::Secondary).disabled(!enabled),
        ])]
    };
    let embed = |colour: u32, text: String| serenity::CreateEmbed::new().colour(colour).description(text);
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed(0xed4245, format!("Are you sure you want me to leave **{name}** (`{guild_id}`)?")))
                .components(buttons(true)),
        )
        .await?;

    let (yes, no) = (yes_id.clone(), no_id.clone());
    let author = ctx.author().id;
    let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .filter(move |i| (i.data.custom_id == yes || i.data.custom_id == no) && i.user.id == author)
        .timeout(CONFIRM_TIME)
        .await;
    if let Some(press) = &press {
        press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
    }

    let result = match press {
        Some(press) if press.data.custom_id == yes_id => match guild_id.leave(ctx).await {
            Ok(()) => embed(0x2ecc71, format!("✅ Left **{name}** (`{guild_id}`).")),
            Err(e) => embed(0xed4245, format!("❌ Couldn't leave **{name}**: {e}")),
        },
        _ => embed(0x95a5a6, format!("Cancelled. I'm still in **{name}** (`{guild_id}`).")),
    };
    // Already gone if it was in the server being left
    let _ = reply.edit(ctx, poise::CreateReply::default().embed(result).components(buttons(false))).await;
    Ok(())
}

/// Turn maintenance mode on or off: while it's on, only the bot owner can use commands (bot owner only)
#[poise::command(
    slash_command,
    rename = "maintenance",
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn maintenance_mode(ctx: Context<'_>) -> Result<(), Error> {
    let on = !maintenance();
    if on {
        std::fs::write(MAINTENANCE_FILE, "")?;
    } else if let Err(e) = std::fs::remove_file(MAINTENANCE_FILE) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    MAINTENANCE.store(on, Ordering::Relaxed);
    crate::presence::apply(ctx.serenity_context());

    let (title, text) = if on {
        ("Maintenance mode **enabled**", "👷 The bot is in maintenance mode. Only you can use its commands until you run `/maintenance` again.")
    } else {
        ("Maintenance mode **disabled**", "👷 The bot is out of maintenance mode, and everyone can use its commands again.")
    };
    let embed = serenity::CreateEmbed::new().colour(0x2ecc71).title(title).description(text).timestamp(serenity::Timestamp::now());
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
