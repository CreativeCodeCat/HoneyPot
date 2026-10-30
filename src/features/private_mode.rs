//! Private mode: one switch for the whole bot, not per server. While it's on, the bot only
//! stays in servers owned by its owners: it leaves the others when it starts (and when the
//! mode is turned on), and leaves straight away if someone adds it to one, logging it.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use crate::features::{Context, Error};
use crate::log;

/// Exists while private mode is on, so it stays on across restarts
const PRIVATE_FILE: &str = "database/private_mode";
/// How long the confirm buttons wait
const CONFIRM_TIME: std::time::Duration = std::time::Duration::from_secs(60);
/// Servers named in the confirmation before "and N more"
const LISTED: usize = 10;

static PRIVATE: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(std::path::Path::new(PRIVATE_FILE).exists()));

/// Whether the bot only stays in its owners' servers.
pub fn enabled() -> bool {
    PRIVATE.load(Ordering::Relaxed)
}

/// Turns private mode on or off, keeping it across restarts. Turning it on doesn't leave any
/// servers by itself: see `others` and `leave`.
pub fn set(on: bool) -> std::io::Result<()> {
    if on {
        std::fs::write(PRIVATE_FILE, "")?;
    } else if let Err(e) = std::fs::remove_file(PRIVATE_FILE) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e);
        }
    }
    PRIVATE.store(on, Ordering::Relaxed);
    Ok(())
}

/// The servers the bot is in that aren't owned by a bot owner, by name.
pub fn others(cache: &serenity::Cache, owners: &HashSet<serenity::UserId>) -> Vec<(serenity::GuildId, String)> {
    let mut others: Vec<(serenity::GuildId, String)> = cache
        .guilds()
        .into_iter()
        .filter_map(|id| cache.guild(id).filter(|g| !owners.contains(&g.owner_id)).map(|g| (id, g.name.clone())))
        .collect();
    others.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    others
}

/// Leaves the servers, logging each. Returns the ones it couldn't leave, with why.
pub async fn leave(http: &serenity::Http, servers: &[(serenity::GuildId, String)]) -> Vec<(String, String)> {
    let mut failed = Vec::new();
    for (id, name) in servers {
        log::warning(format!("[PRIVATE] Leaving {name} (ID: {id}): private mode is on"));
        if let Err(e) = id.leave(http).await {
            log::error(format!("[PRIVATE] Error leaving {id}: {e}"));
            failed.push((name.clone(), e.to_string()));
        }
    }
    failed
}

/// Leaves a server not owned by a bot owner while private mode is on. Run whenever the bot
/// joins or reconnects to a server, so it covers startup and new servers. `is_new` is true
/// when someone has just added the bot. Returns true if it left.
pub async fn enforce(ctx: &serenity::Context, owners: &HashSet<serenity::UserId>, guild: &serenity::Guild, is_new: bool) -> bool {
    // Without knowing who the owners are, every server would look like someone else's
    if !enabled() || owners.is_empty() || owners.contains(&guild.owner_id) {
        return false;
    }
    let owner = guild.members.get(&guild.owner_id).map_or_else(|| guild.owner_id.to_string(), |m| format!("{} ({})", m.user.tag(), guild.owner_id));
    if is_new {
        log::warning(format!(
            "[PRIVATE] Blocked being added to {} (ID: {}, owner {owner}, {} members): private mode is on",
            guild.name, guild.id, guild.member_count
        ));
    } else {
        log::warning(format!("[PRIVATE] Leaving {} (ID: {}, owner {owner}): private mode is on", guild.name, guild.id));
    }
    if let Err(e) = guild.id.leave(ctx).await {
        log::error(format!("[PRIVATE] Error leaving {}: {e}", guild.id));
    }
    true
}

/// Turn private mode on or off: while it's on, the bot only stays in servers you own (bot owner only)
#[command(
    slash_command,
    rename = "private",
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn private_mode(ctx: Context<'_>) -> Result<(), Error> {
    let embed = |colour: u32, title: &str, text: String| {
        serenity::CreateEmbed::new().colour(colour).title(title).description(text).timestamp(serenity::Timestamp::now())
    };
    if enabled() {
        set(false)?;
        log::info(format!("[PRIVATE] {} turned private mode off", ctx.author().tag()));
        let text = "🔓 Anyone can add the bot to their server again.".to_string();
        ctx.send(CreateReply::default().embed(embed(0x2ecc71, "Private mode **disabled**", text))).await?;
        return Ok(());
    }

    // The servers it would leave
    let others = others(ctx.cache(), &ctx.framework().owners);

    if others.is_empty() {
        set(true)?;
        log::info(format!("[PRIVATE] {} turned private mode on", ctx.author().tag()));
        let text = "🔒 The bot is only in servers you own, and will leave any other server it's added to.".to_string();
        ctx.send(CreateReply::default().embed(embed(0x2ecc71, "Private mode **enabled**", text))).await?;
        return Ok(());
    }

    let mut list: Vec<String> = others.iter().take(LISTED).map(|(id, name)| format!("• **{name}** (`{id}`)")).collect();
    if others.len() > LISTED {
        list.push(format!("…and {} more", others.len() - LISTED));
    }
    let (yes_id, no_id) = (format!("{}private_yes", ctx.id()), format!("{}private_no", ctx.id()));
    let buttons = |enabled: bool| {
        vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&yes_id).label("Turn on and leave them").style(serenity::ButtonStyle::Danger).disabled(!enabled),
            serenity::CreateButton::new(&no_id).label("Cancel").style(serenity::ButtonStyle::Secondary).disabled(!enabled),
        ])]
    };
    let count = others.len();
    let text = format!(
        "Turning private mode on makes me leave the **{count}** server{} not owned by a bot owner:\n{}\n\nI'll also leave any other server I'm added to while it's on.",
        if count == 1 { "" } else { "s" },
        list.join("\n")
    );
    let reply = ctx.send(CreateReply::default().embed(embed(0xed4245, "Turn on private mode?", text)).components(buttons(true))).await?;

    let (yes, no) = (yes_id.clone(), no_id.clone());
    let author = ctx.author().id;
    let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .filter(move |i| (i.data.custom_id == yes || i.data.custom_id == no) && i.user.id == author)
        .timeout(CONFIRM_TIME)
        .await;
    if let Some(press) = &press {
        press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
    }
    if !press.is_some_and(|press| press.data.custom_id == yes_id) {
        let result = embed(0x95a5a6, "Private mode is still off", "Cancelled. I haven't left any servers.".to_string());
        reply.edit(ctx, CreateReply::default().embed(result).components(buttons(false))).await?;
        return Ok(());
    }

    set(true)?;
    log::info(format!("[PRIVATE] {} turned private mode on, leaving {count} server(s)", ctx.author().tag()));
    let failed: Vec<String> = leave(ctx.http(), &others).await.into_iter().map(|(name, e)| format!("**{name}**: {e}")).collect();
    let left = count - failed.len();
    let mut text = format!("🔒 Left **{left}** server{}. I'll leave any other server I'm added to.", if left == 1 { "" } else { "s" });
    if !failed.is_empty() {
        text.push_str(&format!("\n\n❌ Couldn't leave:\n{}", failed.join("\n")));
    }
    // Already gone if it was run in one of the servers left
    let _ = reply.edit(ctx, CreateReply::default().embed(embed(0x2ecc71, "Private mode **enabled**", text)).components(buttons(false))).await;
    Ok(())
}
