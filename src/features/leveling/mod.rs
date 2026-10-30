//! Levels cog: members earn XP for chatting, level up, and can give each other reputation.

mod card;

pub use crate::database::repositories::leveling as store;


use chrono::Utc;
use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::{Context, Data, Error};
use crate::log;
use crate::shared::models::CardUser;
use store::{level_for, xp_for, Settings, COOLDOWN_SECS_RANGE, RATE_RANGE, WINDOW_MINS_RANGE, XP_RANGE};

pub const COG_NAME: &str = "levels";

/// Reputation given per /reputation
const REP_AMOUNT: i64 = 3;
/// Time between /reputation uses for each member
const REP_COOLDOWN_SECS: i64 = 2 * 60 * 60;

/// 2015-01-01 in Unix milliseconds
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

const GREEN: u32 = 0x57f287;
const PURPLE: u32 = 0x9b59b6;

fn guild_id(ctx: Context<'_>) -> u64 {
    ctx.guild_id().expect("levels commands are guild_only").get()
}

fn number(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A text progress bar, e.g. "▰▰▰▱▱▱▱▱▱▱".
fn progress_bar(current: i64, total: i64) -> String {
    const WIDTH: i64 = 12;
    let filled = if total > 0 { (current * WIDTH / total).clamp(0, WIDTH) } else { 0 };
    "▰".repeat(filled as usize) + &"▱".repeat((WIDTH - filled) as usize)
}

/// Only people earn XP or count as the latest speaker: bots (the bot itself included),
/// webhooks and system messages are ignored entirely.
fn is_human(msg: &serenity::Message) -> bool {
    !msg.author.bot && !msg.author.system && msg.webhook_id.is_none()
}

/// Awards XP for a message and announces level-ups.
pub async fn on_message(ctx: &serenity::Context, data: &Data, msg: &serenity::Message) {
    let Some(guild_id) = msg.guild_id else { return };
    if !is_human(msg) || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }

    // One lock covers reading the channel's latest message, awarding and updating it,
    // so messages handled at the same time can't both claim the same reply bonus
    let result = {
        let mut levels = data.levels.lock().unwrap();
        levels.settings(guild_id.get()).and_then(|settings| {
            // Discord IDs encode the send time in milliseconds since the Discord epoch
            let sent_ms = (msg.id.get() >> 22) as i64 + DISCORD_EPOCH_MS;
            let award = levels.award_message_xp(guild_id.get(), msg.channel_id.get(), msg.author.id.get(), sent_ms, &settings)?;
            Ok(award.map(|a| (level_for(a.old_xp, settings.rate), level_for(a.new_xp, settings.rate), settings.levelup_channel_id)))
        })
    };

    let (old_level, new_level, levelup_channel) = match result {
        Ok(Some(levels)) => levels,
        Ok(None) => return,
        Err(e) => {
            log::error(format!("[LEVELS] Error awarding XP: {e}"));
            return;
        }
    };
    if new_level <= old_level {
        return;
    }

    let embed = serenity::CreateEmbed::new()
        .colour(GREEN)
        .description("⬆ Level Playground")
        .field("New Level Unlocked 🎉", format!("Congratulations <@{}>! You have leveled up to **{new_level}**", msg.author.id), false)
        .footer(serenity::CreateEmbedFooter::new(format!("⬆ {} Leveled Up", msg.author.tag())))
        .timestamp(serenity::Timestamp::now());

    // Announce in the level-up channel, else the system channel, else where they're chatting
    let channel = levelup_channel
        .map(serenity::ChannelId::new)
        .or_else(|| ctx.cache.guild(guild_id).and_then(|g| g.system_channel_id))
        .unwrap_or(msg.channel_id);
    if let Err(e) = channel.send_message(&ctx.http, serenity::CreateMessage::new().embed(embed)).await {
        log::warning(format!("[LEVELS] Could not announce level-up: {e}"));
    }
}

/// Set up how XP and levels work in this server
#[command(
    slash_command,
    rename = "rank-setup",
    guild_only,
    ephemeral,
    category = "Levels",
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn rank_setup(
    ctx: Context<'_>,
    #[description = "Extra XP each level needs over the last (default 2500)"]
    #[min = 250]
    #[max = 7500]
    rate: Option<i64>,
    #[description = "XP per message (default 25)"]
    #[min = 1]
    #[max = 500]
    base: Option<i64>,
    #[description = "XP for replying to someone else within the engagement window (default 300)"]
    #[min = 1]
    #[max = 500]
    max: Option<i64>,
    #[description = "Minutes after another member's message that a reply earns max XP (default 5)"]
    #[min = 1]
    #[max = 60]
    window: Option<i64>,
    #[description = "Seconds between XP awards for each member (default 300)"]
    #[min = 0]
    #[max = 3600]
    cooldown: Option<i64>,
    #[description = "Channel to announce level-ups in"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let gid = guild_id(ctx);
    let current = ctx.data().levels.lock().unwrap().settings(gid)?;
    let settings = Settings {
        rate: rate.unwrap_or(current.rate).clamp(RATE_RANGE.0, RATE_RANGE.1),
        base_xp: base.unwrap_or(current.base_xp).clamp(XP_RANGE.0, XP_RANGE.1),
        max_xp: max.unwrap_or(current.max_xp).clamp(XP_RANGE.0, XP_RANGE.1),
        engagement_window_mins: window.unwrap_or(current.engagement_window_mins).clamp(WINDOW_MINS_RANGE.0, WINDOW_MINS_RANGE.1),
        xp_cooldown_secs: cooldown.unwrap_or(current.xp_cooldown_secs).clamp(COOLDOWN_SECS_RANGE.0, COOLDOWN_SECS_RANGE.1),
        levelup_channel_id: channel.map(|c| c.id.get()).or(current.levelup_channel_id),
    };

    if let Err(problem) = settings.validate() {
        ctx.say(format!("❌ {problem}")).await?;
        return Ok(());
    }

    ctx.data().levels.lock().unwrap().save_settings(gid, settings)?;
    let embed = serenity::CreateEmbed::new().colour(PURPLE).description(format!(
        "✅ | Levels are set up.\n**Rate:** {} XP per level\n**XP per message:** {}, or {} for replying to someone else within {} min\n**XP cooldown:** {} seconds\n**Level-up channel:** {}",
        number(settings.rate),
        settings.base_xp,
        settings.max_xp,
        settings.engagement_window_mins,
        settings.xp_cooldown_secs,
        settings
            .levelup_channel_id
            .map(|id| format!("<#{id}>"))
            .unwrap_or_else(|| "System channel".to_string())
    ));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Gathers everything the rank card shows for a member.
async fn card_data(
    ctx: Context<'_>,
    user: &serenity::User,
    level: i64,
    rep: i64,
    position: Option<i64>,
    xp: i64,
    next_level_xp: i64,
) -> card::CardData {
    let avatar_url = CardUser::avatar_url(user);

    // Highest role (by position) and the member's display colour (highest coloured role)
    let member = ctx.guild_id().unwrap().member(ctx, user.id).await.ok();
    // Server nickname, else global display name, else username
    let display_name = member
        .as_ref()
        .map(|m| m.display_name().to_string())
        .unwrap_or_else(|| user.display_name().to_string());
    let (role_name, role_colour, role_icon_url) = member
        .and_then(|member| {
            let guild = ctx.guild()?;
            let mut roles: Vec<&serenity::Role> = member.roles.iter().filter_map(|id| guild.roles.get(id)).collect();
            roles.sort_by_key(|role| std::cmp::Reverse(role.position));
            let highest = roles.first()?;
            let colour = roles.iter().map(|r| r.colour.0).find(|c| *c != 0).unwrap_or(0xe5e9f0);
            let icon = highest.icon.as_ref().map(|hash| format!("https://cdn.discordapp.com/role-icons/{}/{hash}.png?size=64", highest.id));
            Some((Some(highest.name.clone()), colour, icon))
        })
        .unwrap_or((None, 0xe5e9f0, None));

    let role_icon = match role_icon_url {
        Some(url) => crate::render::download(&url).await,
        None => None,
    };

    card::CardData {
        names: vec![display_name, user.name.clone()],
        avatar: crate::render::download(&avatar_url).await,
        level,
        rep,
        position,
        role_name,
        role_colour,
        role_icon,
        xp,
        next_level_xp,
    }
}

/// Show a member's level, XP and reputation
#[command(slash_command, guild_only, category = "Levels")]
pub async fn rank(
    ctx: Context<'_>,
    #[description = "The member to check (defaults to you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let user = user.as_ref().unwrap_or(ctx.author());
    let (settings, member) = {
        let levels = ctx.data().levels.lock().unwrap();
        (levels.settings(guild_id(ctx))?, levels.member(guild_id(ctx), user.id.get())?)
    };
    let member = member.unwrap_or_default();

    let level = level_for(member.xp, settings.rate);
    let level_start = xp_for(level, settings.rate);
    let next_level = xp_for(level + 1, settings.rate);
    let progress = member.xp - level_start;
    let needed = next_level - level_start;

    ctx.defer().await?;
    let card = card_data(ctx, user, level, member.rep, (member.xp > 0).then_some(member.position), member.xp, next_level).await;
    match tokio::task::spawn_blocking(move || card::render(&card)).await {
        Ok(Some(png)) => {
            let file = serenity::CreateAttachment::bytes(png, "rank.png");
            ctx.send(CreateReply::default().attachment(file)).await?;
            return Ok(());
        }
        _ => log::warning("[LEVELS] Could not render rank card, sending an embed instead"),
    }

    let position = if member.xp > 0 { format!("#{}", member.position) } else { "Unranked".to_string() };
    let embed = serenity::CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .colour(GREEN)
        .field("Rank", position, true)
        .field("Level", level.to_string(), true)
        .field("Reputation", number(member.rep), true)
        .field(
            format!("Progress to level {}", level + 1),
            format!("{} `{} / {} XP`\nTotal XP: {}", progress_bar(progress, needed), number(progress), number(needed), number(member.xp)),
            false,
        );
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Members shown on `/leaderboard`, 10 per page
const LEADERBOARD_SIZE: i64 = 100;

/// Show the top members by XP
#[command(slash_command, guild_only, category = "Levels")]
pub async fn leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let (settings, top) = {
        let levels = ctx.data().levels.lock().unwrap();
        (levels.settings(guild_id(ctx))?, levels.leaderboard(guild_id(ctx), LEADERBOARD_SIZE)?)
    };

    if top.is_empty() {
        ctx.say("There aren't any members on the leaderboard yet.").await?;
        return Ok(());
    }

    let lines: Vec<String> = top
        .iter()
        .enumerate()
        .map(|(i, (user_id, xp))| {
            let medal = match i {
                0 => "🥇",
                1 => "🥈",
                2 => "🥉",
                _ => "▫️",
            };
            format!("{medal} **#{}** <@{user_id}> · Level **{}** · {} XP", i + 1, level_for(*xp, settings.rate), number(*xp))
        })
        .collect();

    let pages = crate::pagination::chunk_lines(&lines, 10)
        .into_iter()
        .map(|text| serenity::CreateEmbed::new().title("🏆 Leaderboard").description(text).colour(PURPLE))
        .collect();
    crate::pagination::paginate(ctx, pages).await
}

/// Give reputation to a member (once every 2 hours)
#[command(slash_command, guild_only, category = "Levels")]
pub async fn reputation(
    ctx: Context<'_>,
    #[description = "The member you want to give reputation to"] user: serenity::User,
) -> Result<(), Error> {
    let reply_private = |text: String| ctx.send(CreateReply::default().content(text).ephemeral(true));

    if user.id == ctx.author().id {
        reply_private("❌ You can't give reputation to yourself.".to_string()).await?;
        return Ok(());
    }
    if user.bot {
        reply_private("❌ You can't give reputation to bots.".to_string()).await?;
        return Ok(());
    }

    let now = Utc::now().timestamp();
    let result = ctx.data().levels.lock().unwrap().give_rep(
        guild_id(ctx),
        ctx.author().id.get(),
        user.id.get(),
        REP_AMOUNT,
        now,
        REP_COOLDOWN_SECS,
    )?;

    match result {
        Ok(total) => {
            let embed = serenity::CreateEmbed::new().title("Reputation Given").colour(PURPLE).description(format!(
                "<@{}> gave +{REP_AMOUNT} reputation to <@{}>! They now have {} reputation points.",
                ctx.author().id,
                user.id,
                number(total)
            ));
            ctx.send(CreateReply::default().embed(embed)).await?;
        }
        Err(next) => {
            reply_private(format!("❌ You can give reputation once every 2 hours. Try again <t:{next}:R>.")).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_and_bars() {
        assert_eq!(number(0), "0");
        assert_eq!(number(1234567), "1,234,567");
        assert_eq!(progress_bar(0, 100), "▱".repeat(12));
        assert_eq!(progress_bar(50, 100), "▰".repeat(6) + &"▱".repeat(6));
        assert_eq!(progress_bar(100, 100), "▰".repeat(12));
    }

    fn message(bot: bool, webhook: bool) -> serenity::Message {
        let mut msg = serenity::Message::default();
        msg.author.bot = bot;
        msg.webhook_id = webhook.then(|| serenity::WebhookId::new(1));
        msg
    }

    #[test]
    fn bots_and_webhooks_are_ignored() {
        assert!(is_human(&message(false, false)));
        assert!(!is_human(&message(true, false)));
        assert!(!is_human(&message(false, true)));
    }

    #[test]
    fn send_time_comes_from_the_message_id() {
        let id = serenity::MessageId::new(1_300_000_000_000_000_000);
        let from_id = (id.get() >> 22) as i64 + DISCORD_EPOCH_MS;
        assert_eq!(from_id / 1000, id.created_at().unix_timestamp());
    }
}
