//! `/info server|channel|user|staff|bot`, plus the "User Info" user context menu.

use ::serenity::all as serenity;

use crate::core::framework::{command, Command, CreateReply};
use crate::pagination::{chunk_lines, field_pages, paginate};
use super::{Context, Error};

const COLOUR: u32 = 0xf1c40f;
/// Members fetched per request when counting members or finding staff
const MEMBER_PAGE: u64 = 1000;

/// A Discord timestamp showing the full date and how long ago it was.
fn when(timestamp: serenity::Timestamp) -> String {
    let secs = timestamp.unix_timestamp();
    format!("<t:{secs}:F> (<t:{secs}:R>)")
}

/// Every member of the guild, fetched from Discord as the cache may only hold some of them.
async fn all_members(ctx: Context<'_>, guild_id: serenity::GuildId) -> Result<Vec<serenity::Member>, Error> {
    let mut members = Vec::new();
    let mut after = None;
    loop {
        let page = guild_id.members(ctx.http(), Some(MEMBER_PAGE), after).await?;
        let done = (page.len() as u64) < MEMBER_PAGE;
        after = page.last().map(|m| m.user.id);
        members.extend(page);
        if done {
            return Ok(members);
        }
    }
}

fn channel_kind(kind: serenity::ChannelType) -> &'static str {
    use serenity::ChannelType as T;
    match kind {
        T::Text => "Text",
        T::Voice => "Voice",
        T::Category => "Category",
        T::News => "Announcement",
        T::NewsThread => "Announcement Thread",
        T::PublicThread => "Public Thread",
        T::PrivateThread => "Private Thread",
        T::Stage => "Stage",
        T::Directory => "Directory",
        T::Forum => "Forum",
        _ => "Other",
    }
}

/// Show information about the server, a channel, a user, the staff or the bot
#[command(
    slash_command,
    guild_only,
    category = "Info",
    subcommands("server", "channel", "user", "staff", "bot"),
    subcommand_required
)]
pub async fn info(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Show information about this server
#[command(slash_command, guild_only)]
async fn server(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let guild_id = ctx.guild_id().unwrap();
    let members = all_members(ctx, guild_id).await?;
    // The cached server can be missing its description, so it's fetched fresh
    let description = guild_id.to_partial_guild(ctx).await?.description.filter(|d| !d.is_empty());
    let bots = members.iter().filter(|m| m.user.bot).count();

    // Read everything needed from the cache before any more awaits
    let (general, stats, roles, icon) = {
        let Some(guild) = ctx.guild() else { return Err("server isn't cached".into()) };
        let count = |kind: serenity::ChannelType| guild.channels.values().filter(|c| c.kind == kind).count();
        let animated = guild.emojis.values().filter(|e| e.animated).count();
        let filter = match guild.explicit_content_filter {
            serenity::ExplicitContentFilter::None => "Off",
            serenity::ExplicitContentFilter::WithoutRole => "Members without roles",
            serenity::ExplicitContentFilter::All => "Everyone",
            _ => "Unknown",
        };
        let verification = match guild.verification_level {
            serenity::VerificationLevel::None => "None",
            serenity::VerificationLevel::Low => "Low",
            serenity::VerificationLevel::Medium => "Medium",
            serenity::VerificationLevel::High => "High",
            serenity::VerificationLevel::Higher => "Very High",
            _ => "Unknown",
        };
        let tier = match u8::from(guild.premium_tier) {
            0 => "None".to_string(),
            tier => format!("Tier {tier}"),
        };
        let general = format!(
            "**❯ Name:** `{}`\n**❯ Description:** {}\n**❯ ID:** `{}`\n**❯ Owner:** <@{}>\n**❯ Boost Tier:** `{tier}`\n\
             **❯ Explicit Filter:** `{filter}`\n**❯ Verification Level:** `{verification}`\n**❯ Created:** {}",
            guild.name,
            description.as_deref().map_or("None".to_string(), |d| format!("`{d}`")),
            guild.id,
            guild.owner_id,
            when(guild.id.created_at()),
        );
        let stats = format!(
            "**❯ Members:** `{}`\n**❯ Humans:** `{}`\n**❯ Bots:** `{bots}`\n**❯ Roles:** `{}`\n\
             **❯ Emojis:** `{}` (`{}` regular, `{animated}` animated)\n**❯ Text Channels:** `{}`\n\
             **❯ Voice Channels:** `{}`\n**❯ Categories:** `{}`\n**❯ Boosts:** `{}`",
            members.len(),
            members.len() - bots,
            // Without @everyone
            guild.roles.len().saturating_sub(1),
            guild.emojis.len(),
            guild.emojis.len() - animated,
            count(serenity::ChannelType::Text) + count(serenity::ChannelType::News),
            count(serenity::ChannelType::Voice) + count(serenity::ChannelType::Stage),
            count(serenity::ChannelType::Category),
            guild.premium_subscription_count.unwrap_or(0),
        );
        // Highest first, leaving out @everyone and roles bots and integrations manage (but not the booster role)
        let mut roles: Vec<&serenity::Role> = guild
            .roles
            .values()
            .filter(|r| r.id.get() != guild.id.get() && (!r.managed || r.tags.premium_subscriber))
            .collect();
        roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        let roles: Vec<String> = roles.iter().map(|r| format!("<@&{}>", r.id)).collect();
        (general, stats, roles, guild.icon_url())
    };

    let page = || {
        let mut embed = serenity::CreateEmbed::new().colour(COLOUR).timestamp(serenity::Timestamp::now());
        if let Some(icon) = &icon {
            embed = embed.thumbnail(icon);
        }
        embed
    };
    let title = format!("Server Information for {}", ctx.guild().map(|g| g.name.clone()).unwrap_or_default());
    let mut pages = vec![page().title(&title).field("__General__", general, false).field("__Statistics__", stats, false)];
    if roles.is_empty() {
        pages.push(page().title(&title).field("Roles [0]", "None", false));
    }
    for text in chunk_lines(&roles, 40) {
        // One line of space-separated mentions per page
        pages.push(page().title(&title).field(format!("Roles [{}]", roles.len()), text.replace('\n', " "), false));
    }
    paginate(ctx, pages).await
}

/// Show information about a channel
#[command(slash_command, guild_only, ephemeral)]
async fn channel(
    ctx: Context<'_>,
    #[description = "The channel (default: this one)"] channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let channel = match channel {
        Some(channel) => channel,
        None => ctx.channel_id().to_channel(ctx).await?.guild().ok_or("not a server channel")?,
    };
    let parent = channel.parent_id.map_or("None".to_string(), |id| format!("<#{id}>"));
    let mut embed = serenity::CreateEmbed::new()
        .colour(COLOUR)
        .description(format!("Info about <#{}>:", channel.id))
        .field("Name", format!("`{}`", channel.name), true)
        .field("ID", format!("`{}`", channel.id), true)
        .field("Type", format!("`{}`", channel_kind(channel.kind)), true)
        .field("Category", parent, true)
        .field("Position", format!("`{}`", channel.position), true)
        .field("NSFW", if channel.nsfw { "`Yes`" } else { "`No`" }, true);
    if let Some(seconds) = channel.rate_limit_per_user.filter(|s| *s > 0) {
        embed = embed.field("Slowmode", format!("`{seconds}s`"), true);
    }
    if let Some(limit) = channel.user_limit.filter(|l| *l > 0) {
        embed = embed.field("User Limit", format!("`{limit}`"), true);
    }
    embed = embed.field("Created", when(channel.id.created_at()), false);
    if let Some(topic) = channel.topic.as_deref().filter(|t| !t.is_empty()) {
        embed = embed.field("Topic", super::truncate(topic, 1024), false);
    }
    if let Some(guild) = ctx.guild() {
        embed = embed.author(serenity::CreateEmbedAuthor::new(&guild.name).icon_url(guild.icon_url().unwrap_or_default()));
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Shows `user`, with their server details if they're a member.
async fn show_user(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let member = guild_id.member(ctx, user.id).await.ok();

    let mut embed = serenity::CreateEmbed::new()
        .colour(COLOUR)
        .title("General Info")
        .author(serenity::CreateEmbedAuthor::new(&user.name).icon_url(user.face()))
        .thumbnail(member.as_ref().map_or_else(|| user.face(), |m| m.face()))
        .field("Username", format!("> {}", user.name), true)
        .field("Display Name", format!("> {}", user.display_name()), true)
        .field("Nickname", format!("> {}", member.as_ref().and_then(|m| m.nick.as_deref()).unwrap_or("None")), true)
        .field("User ID", format!("> {}", user.id), true)
        .field("Bot", if user.bot { "> Yes" } else { "> No" }, true)
        .field("Account Created", when(user.id.created_at()), false);

    match &member {
        Some(member) => {
            if let Some(joined) = member.joined_at {
                embed = embed.field("Joined Server", when(joined), false);
            }
            if let Some(since) = member.premium_since {
                embed = embed.field("Boosting Since", when(since), false);
            }
            let mut roles: Vec<(i16, serenity::RoleId)> = {
                let guild = ctx.guild();
                member
                    .roles
                    .iter()
                    .map(|id| (guild.as_ref().and_then(|g| g.roles.get(id)).map_or(0, |r| r.position as i16), *id))
                    .collect()
            };
            roles.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
            let shown: Vec<String> = roles.iter().take(30).map(|(_, id)| format!("<@&{id}>")).collect();
            let mut text = if shown.is_empty() { "None".to_string() } else { shown.join(" ") };
            if roles.len() > shown.len() {
                text += &format!(" and {} more", roles.len() - shown.len());
            }
            embed = embed.field(format!("Roles [{}]", roles.len()), text, false);
        }
        None => embed = embed.field("Server", "Not a member of this server", false),
    }

    if let Some(guild) = ctx.guild() {
        embed = embed.footer(serenity::CreateEmbedFooter::new(&guild.name).icon_url(guild.icon_url().unwrap_or_default()));
    }
    ctx.send(CreateReply::default().embed(embed.timestamp(serenity::Timestamp::now()))).await?;
    Ok(())
}

/// Show information about a user
#[command(slash_command, guild_only)]
async fn user(
    ctx: Context<'_>,
    #[description = "The user (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    show_user(ctx, user.unwrap_or_else(|| ctx.author().clone())).await
}

/// Show information about this user
#[command(context_menu_command = "User Info", guild_only, category = "Info")]
pub async fn user_info_menu(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    show_user(ctx, user).await
}

/// List the server's staff: the owner, admins and moderators
#[command(slash_command, guild_only, ephemeral)]
async fn staff(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap();
    let members = all_members(ctx, guild_id).await?;

    let (owner, mut admins, mut mods, title, icon) = {
        let Some(guild) = ctx.guild() else { return Err("server isn't cached".into()) };
        let (mut admins, mut mods) = (vec![], vec![]);
        for member in members.iter().filter(|m| !m.user.bot && m.user.id != guild.owner_id) {
            let perms = guild.member_permissions(member);
            if perms.administrator() || perms.manage_guild() {
                admins.push(format!("<@{}>", member.user.id));
            } else if perms.moderate_members() || perms.kick_members() || perms.ban_members() || perms.manage_messages() {
                mods.push(format!("<@{}>", member.user.id));
            }
        }
        (guild.owner_id, admins, mods, format!("Staff of {}", guild.name), guild.icon_url())
    };
    admins.sort();
    mods.sort();

    let page = || {
        let mut embed = serenity::CreateEmbed::new().colour(0xe67e22).title(&title).timestamp(serenity::Timestamp::now());
        if let Some(icon) = &icon {
            embed = embed.thumbnail(icon);
        }
        embed
    };
    // The owner, then each group 20 to a field, 3 fields to a page
    let mut fields = vec![("👑 Owner".to_string(), format!("<@{owner}>"))];
    for (name, group) in [("🛡️ Administrators", &admins), ("🔨 Moderators", &mods)] {
        for (i, text) in chunk_lines(group, 20).into_iter().enumerate() {
            let label = if i == 0 { format!("{name} [{}]", group.len()) } else { format!("{name} (continued)") };
            fields.push((label, text));
        }
    }
    let pages = field_pages(page, fields, 3, false);
    paginate(ctx, pages).await
}

/// Total and available memory in bytes, from /proc/meminfo (Linux only).
fn system_memory() -> Option<(u64, u64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        info.lines().find(|l| l.starts_with(name))?.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// The bot's own memory use in bytes, from /proc/self/status (Linux only).
fn bot_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = status.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// (busy, total) CPU time across all cores, from /proc/stat (Linux only).
fn cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let times: Vec<u64> = stat.lines().next()?.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
    let total: u64 = times.iter().sum();
    // idle and iowait
    let idle = times.get(3)? + times.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// System CPU use as a percentage, measured over a short moment.
async fn cpu_percent() -> Option<f64> {
    let (busy1, total1) = cpu_times()?;
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let (busy2, total2) = cpu_times()?;
    let total = total2.checked_sub(total1).filter(|t| *t > 0)?;
    Some((busy2 - busy1) as f64 * 100.0 / total as f64)
}

fn bytes(n: u64) -> String {
    let mb = n as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 { format!("{:.2} GB", mb / 1024.0) } else { format!("{mb:.1} MB") }
}

/// A 20-block bar like `██████░░░░░░░░░░░░░░` for a percentage.
fn bar(percent: f64) -> String {
    let filled = ((percent / 5.0).round() as usize).min(20);
    format!("`{}{}`", "█".repeat(filled), "░".repeat(20 - filled))
}

/// The SQLite files in `database/`: how many, and their total size.
fn database_files() -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir("database") else { return (0, 0) };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "db"))
        .filter_map(|e| e.metadata().ok())
        .fold((0, 0), |(count, size), meta| (count + 1, size + meta.len()))
}

/// Every command, counting each subcommand rather than the groups holding them.
fn count_commands(commands: &[Command]) -> usize {
    commands.iter().map(|c| if c.subcommands.is_empty() { 1 } else { count_commands(&c.subcommands) }).sum()
}

/// Show information about the bot: its stats, database, host, memory and CPU
#[command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
async fn bot(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let me = ctx.cache().current_user().clone();
    // A team-owned bot's owner is a placeholder user named after the team's ID, so the team's name is shown
    let owner = ctx.http().get_current_application_info().await.ok().and_then(|app| match app.team {
        Some(team) => Some(format!("{} (team)", team.name)),
        None => app.owner.map(|owner| owner.tag()),
    });
    let ping = ctx.ping().await.as_millis();
    let started = serenity::Timestamp::now().unix_timestamp() - ctx.data().start_time.elapsed().as_secs() as i64;
    let (servers, members) = {
        let guilds = ctx.cache().guilds();
        let members: u64 = guilds.iter().filter_map(|id| ctx.cache().guild(*id).map(|g| g.member_count)).sum();
        (guilds.len(), members)
    };
    let commands = count_commands(&ctx.framework().commands);
    let (db_count, db_size) = database_files();
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map_or("Unknown".to_string(), |r| r.trim().to_string());
    let cpu = cpu_percent().await;
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());

    let page = |title: &str| {
        serenity::CreateEmbed::new()
            .colour(0x9b59b6)
            .author(serenity::CreateEmbedAuthor::new(ctx.author().tag()).icon_url(ctx.author().face()))
            .title(title)
            .thumbnail(me.face())
            .timestamp(serenity::Timestamp::now())
    };
    let unknown = || "Unknown (only shown on Linux)".to_string();

    let mut pages = vec![
        page("🤖 Client")
            .field("Username", me.tag(), true)
            .field("Version", env!("CARGO_PKG_VERSION"), true)
            .field("Library", "[serenity](https://github.com/serenity-rs/serenity)", true)
            .field("Ping", format!("🟢 Online, {ping} ms"), true)
            .field("Owner", owner.unwrap_or_else(|| "Unknown".into()), true)
            .field("Started", format!("<t:{started}:F> (<t:{started}:R>)"), false),
        page("📊 Stats")
            .field("Commands", commands.to_string(), true)
            .field("Servers", servers.to_string(), true)
            .field("Members", members.to_string(), true),
        page("🗄️ Database")
            .field("Name", "SQLite", true)
            .field("Status", if db_count > 0 { "🟢 Connected" } else { "🔴 No database files found" }, true)
            .field("Files", format!("{db_count} ({})", bytes(db_size)), true),
        page("🖥️ Host")
            .field("Platform", std::env::consts::OS, true)
            .field("Architecture", std::env::consts::ARCH, true)
            .field("Release", kernel, true)
            .field("CPU Cores", cores.to_string(), true),
    ];

    let mut memory = page("🧠 Memory");
    match system_memory() {
        Some((total, available)) => {
            let used = total.saturating_sub(available);
            let percent = used as f64 * 100.0 / total as f64;
            memory = memory
                .field("Percentage", format!("{percent:.1}%"), true)
                .field("Used", format!("{} / {}", bytes(used), bytes(total)), true)
                .field("Bar", bar(percent), false);
        }
        None => memory = memory.field("System", unknown(), false),
    }
    memory = memory.field("Used by the Bot", bot_memory().map_or_else(unknown, bytes), false);
    pages.push(memory);

    let mut cpu_page = page("⚙️ CPU");
    match cpu {
        Some(percent) => cpu_page = cpu_page.field("Percentage", format!("{percent:.1}%"), true).field("Bar", bar(percent), false),
        None => cpu_page = cpu_page.field("Percentage", unknown(), false),
    }
    pages.push(cpu_page);

    paginate(ctx, pages).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_sizes_are_formatted() {
        assert_eq!(bar(0.0), format!("`{}`", "░".repeat(20)));
        assert_eq!(bar(50.0), format!("`{}{}`", "█".repeat(10), "░".repeat(10)));
        assert_eq!(bar(150.0), format!("`{}`", "█".repeat(20)));
        assert_eq!(bytes(512 * 1024 * 1024), "512.0 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
