//! `/info server|channel|user|staff`, plus the "User Info" user context menu.

use poise::serenity_prelude as serenity;

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

/// Show information about the server, a channel, a user or the staff
#[poise::command(
    slash_command,
    guild_only,
    category = "Info",
    subcommands("server", "channel", "user", "staff"),
    subcommand_required
)]
pub async fn info(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Show information about this server
#[poise::command(slash_command, guild_only)]
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
#[poise::command(slash_command, guild_only, ephemeral)]
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
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
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
    ctx.send(poise::CreateReply::default().embed(embed.timestamp(serenity::Timestamp::now()))).await?;
    Ok(())
}

/// Show information about a user
#[poise::command(slash_command, guild_only)]
async fn user(
    ctx: Context<'_>,
    #[description = "The user (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    show_user(ctx, user.unwrap_or_else(|| ctx.author().clone())).await
}

/// Show information about this user
#[poise::command(context_menu_command = "User Info", guild_only, category = "Info")]
pub async fn user_info_menu(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    show_user(ctx, user).await
}

/// List the server's staff: the owner, admins and moderators
#[poise::command(slash_command, guild_only, ephemeral)]
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
