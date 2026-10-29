//! Moderation cog: `/ban`, `/unban`, `/kick`, `/timeout` and `/untimeout`, plus the
//! "Moderate" message and user context menus, which delete the message or time out, kick
//! or ban the member with buttons.

use std::time::Duration;

use poise::serenity_prelude as serenity;
use poise::ChoiceParameter;

use super::{truncate, Context, Error};

const COLOUR: u32 = 0xed4245;
/// How long the context menu's buttons keep working after the last press
const MENU_TIMEOUT: Duration = Duration::from_secs(120);
/// Discord's limit on audit log reasons
const MAX_REASON: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum TimeoutLength {
    #[name = "60 Seconds"]
    Minute,
    #[name = "5 Minutes"]
    FiveMinutes,
    #[name = "10 Minutes"]
    TenMinutes,
    #[name = "1 Hour"]
    Hour,
    #[name = "1 Day"]
    Day,
    #[name = "1 Week"]
    Week,
    #[name = "28 Days"]
    Max,
}

impl TimeoutLength {
    const ALL: [Self; 7] = [Self::Minute, Self::FiveMinutes, Self::TenMinutes, Self::Hour, Self::Day, Self::Week, Self::Max];

    fn seconds(self) -> i64 {
        match self {
            Self::Minute => 60,
            Self::FiveMinutes => 5 * 60,
            Self::TenMinutes => 10 * 60,
            Self::Hour => 60 * 60,
            Self::Day => 24 * 60 * 60,
            Self::Week => 7 * 24 * 60 * 60,
            Self::Max => 28 * 24 * 60 * 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum DeleteMessages {
    #[name = "Don't delete any"]
    None,
    #[name = "Previous 24 hours"]
    Day,
    #[name = "Previous 7 days"]
    Week,
}

impl DeleteMessages {
    fn days(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Day => 1,
            Self::Week => 7,
        }
    }
}

/// The audit log reason: who did it and why.
fn audit_reason(ctx: Context<'_>, reason: Option<&str>) -> String {
    truncate(&format!("{}: {}", ctx.author().name, reason.unwrap_or("No reason given")), MAX_REASON)
}

fn result_embed(title: &str, text: String, reason: Option<&str>) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .colour(COLOUR)
        .title(title)
        .description(text)
        .field("Reason", reason.unwrap_or("No reason given"), false)
        .timestamp(serenity::Timestamp::now())
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Tells the member what happened before it's done, as they can't be messaged once they've left.
/// They may have DMs closed, which is fine.
async fn notify(ctx: Context<'_>, user: &serenity::User, what: &str, reason: Option<&str>) {
    let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_default();
    let text = format!("You have been {what} in **{guild_name}**.\n**Reason:** {}", reason.unwrap_or("No reason given"));
    let _ = user.direct_message(ctx, serenity::CreateMessage::new().content(text)).await;
}

/// Why the author (and the bot) can't act on `target`, if they can't: themselves, the bot,
/// the owner, or someone whose highest role isn't below theirs.
async fn hierarchy_problem(ctx: Context<'_>, target: serenity::UserId) -> Option<&'static str> {
    let bot_id = ctx.cache().current_user().id;
    if target == ctx.author().id {
        return Some("❌ You can't moderate yourself.");
    }
    if target == bot_id {
        return Some("❌ I can't moderate myself.");
    }
    let guild_id = ctx.guild_id()?;
    // Not a member: nothing to compare, as only bans and unbans work on them
    let target_member = guild_id.member(ctx, target).await.ok()?;
    let author_member = ctx.author_member().await?.into_owned();
    let bot_member = guild_id.member(ctx, bot_id).await.ok()?;

    let guild = ctx.guild()?;
    if target == guild.owner_id {
        return Some("❌ The server owner can't be moderated.");
    }
    let top = |member: &serenity::Member| guild.member_highest_role(member).map_or(0, |r| r.position);
    if author_member.user.id != guild.owner_id && top(&target_member) >= top(&author_member) {
        return Some("❌ That member's highest role is the same as or above yours.");
    }
    if top(&target_member) >= top(&bot_member) {
        return Some("❌ That member's highest role is the same as or above mine, so I can't moderate them.");
    }
    None
}

/// Ban a member, or anyone by user ID
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "BAN_MEMBERS",
    default_member_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
pub async fn ban(
    ctx: Context<'_>,
    #[description = "The user to ban (you can paste a user ID)"] user: serenity::User,
    #[description = "Why they're being banned"]
    #[max_length = 400]
    reason: Option<String>,
    #[description = "Delete their recent messages (default: don't)"] delete_messages: Option<DeleteMessages>,
) -> Result<(), Error> {
    if let Some(problem) = hierarchy_problem(ctx, user.id).await {
        return say_private(ctx, problem).await;
    }
    let reason = reason.as_deref();
    notify(ctx, &user, "banned", reason).await;
    let days = delete_messages.unwrap_or(DeleteMessages::None).days();
    ctx.guild_id().unwrap().ban_with_reason(ctx, user.id, days, audit_reason(ctx, reason)).await?;
    let embed = result_embed("🔨 Member Banned", format!("<@{}> (`{}`) has been banned.", user.id, user.name), reason);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Unban a user
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "BAN_MEMBERS",
    default_member_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
pub async fn unban(
    ctx: Context<'_>,
    #[description = "The user to unban (paste their user ID)"] user: serenity::User,
    #[description = "Why they're being unbanned"]
    #[max_length = 400]
    reason: Option<String>,
) -> Result<(), Error> {
    let reason = reason.as_deref();
    let guild_id = ctx.guild_id().unwrap();
    if ctx.http().get_ban(guild_id, user.id).await?.is_none() {
        return say_private(ctx, format!("❌ <@{}> isn't banned.", user.id)).await;
    }
    ctx.http().remove_ban(guild_id, user.id, Some(&audit_reason(ctx, reason))).await?;
    let embed = result_embed("✅ User Unbanned", format!("<@{}> (`{}`) has been unbanned.", user.id, user.name), reason);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Kick a member
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "KICK_MEMBERS",
    default_member_permissions = "KICK_MEMBERS",
    required_bot_permissions = "KICK_MEMBERS"
)]
pub async fn kick(
    ctx: Context<'_>,
    #[description = "The member to kick"] member: serenity::Member,
    #[description = "Why they're being kicked"]
    #[max_length = 400]
    reason: Option<String>,
) -> Result<(), Error> {
    if let Some(problem) = hierarchy_problem(ctx, member.user.id).await {
        return say_private(ctx, problem).await;
    }
    let reason = reason.as_deref();
    notify(ctx, &member.user, "kicked", reason).await;
    ctx.guild_id().unwrap().kick_with_reason(ctx, member.user.id, &audit_reason(ctx, reason)).await?;
    let embed = result_embed("👢 Member Kicked", format!("<@{}> (`{}`) has been kicked.", member.user.id, member.user.name), reason);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Time a member out, so they can't chat, react or join voice
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MODERATE_MEMBERS",
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
pub async fn timeout(
    ctx: Context<'_>,
    #[description = "The member to time out"] member: serenity::Member,
    #[description = "How long for"] length: TimeoutLength,
    #[description = "Why they're being timed out"]
    #[max_length = 400]
    reason: Option<String>,
) -> Result<(), Error> {
    if let Some(problem) = hierarchy_problem(ctx, member.user.id).await {
        return say_private(ctx, problem).await;
    }
    let reason = reason.as_deref();
    let until = time_out(ctx, member.user.id, length, &audit_reason(ctx, reason)).await?;
    notify(ctx, &member.user, &format!("timed out until <t:{until}:F>"), reason).await;
    let text = format!("<@{}> has been timed out until <t:{until}:F> (<t:{until}:R>).", member.user.id);
    ctx.send(poise::CreateReply::default().embed(result_embed("⏳ Member Timed Out", text, reason))).await?;
    Ok(())
}

/// Times the member out, returning when it ends as a Unix timestamp.
async fn time_out(ctx: Context<'_>, user_id: serenity::UserId, length: TimeoutLength, reason: &str) -> Result<i64, Error> {
    let until = serenity::Timestamp::now().unix_timestamp() + length.seconds();
    let edit = serenity::EditMember::new()
        .disable_communication_until_datetime(serenity::Timestamp::from_unix_timestamp(until)?)
        .audit_log_reason(reason);
    ctx.guild_id().unwrap().edit_member(ctx, user_id, edit).await?;
    Ok(until)
}

/// End a member's timeout
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MODERATE_MEMBERS",
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
pub async fn untimeout(
    ctx: Context<'_>,
    #[description = "The member whose timeout to end"] member: serenity::Member,
    #[description = "Why their timeout is ending"]
    #[max_length = 400]
    reason: Option<String>,
) -> Result<(), Error> {
    let timed_out = member.communication_disabled_until.is_some_and(|until| until > serenity::Timestamp::now());
    if !timed_out {
        return say_private(ctx, format!("❌ <@{}> isn't timed out.", member.user.id)).await;
    }
    let reason = reason.as_deref();
    let reason_text = audit_reason(ctx, reason);
    let edit = serenity::EditMember::new().enable_communication().audit_log_reason(&reason_text);
    ctx.guild_id().unwrap().edit_member(ctx, member.user.id, edit).await?;
    let embed = result_embed("✅ Timeout Removed", format!("<@{}>'s timeout has ended.", member.user.id), reason);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// The moderator's permissions in the channel the command was used in.
fn permissions(ctx: Context<'_>) -> serenity::Permissions {
    match ctx {
        poise::Context::Application(app) => app.interaction.member.as_ref().and_then(|m| m.permissions).unwrap_or_default(),
        poise::Context::Prefix(_) => serenity::Permissions::empty(),
    }
}

/// Delete a message, or time out, kick or ban its author
#[poise::command(
    context_menu_command = "Moderate",
    guild_only,
    ephemeral,
    category = "Moderation",
    // Discord shows the menu only to members with every listed permission, so only the
    // basic one is required; each button is disabled without its own permission
    default_member_permissions = "MANAGE_MESSAGES"
)]
pub async fn moderate_message(ctx: Context<'_>, message: serenity::Message) -> Result<(), Error> {
    moderation_menu(ctx, message.author.clone(), Some(message)).await
}

/// Time out, kick or ban this member
#[poise::command(
    context_menu_command = "Moderate",
    guild_only,
    ephemeral,
    category = "Moderation",
    // As above, each button is disabled without its own permission
    default_member_permissions = "MODERATE_MEMBERS"
)]
pub async fn moderate_user(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    moderation_menu(ctx, user, None).await
}

/// Buttons to moderate `user`, and to delete `message` when it came from a message.
async fn moderation_menu(ctx: Context<'_>, user: serenity::User, message: Option<serenity::Message>) -> Result<(), Error> {
    let perms = permissions(ctx);
    let prefix = ctx.id();
    let (delete_id, kick_id, ban_id, timeout_id, untimeout_id) = (
        format!("{prefix}mod_delete"),
        format!("{prefix}mod_kick"),
        format!("{prefix}mod_ban"),
        format!("{prefix}mod_timeout"),
        format!("{prefix}mod_untimeout"),
    );
    let components = |deleted: bool| {
        let mut buttons = vec![];
        if message.is_some() {
            buttons.push(
                serenity::CreateButton::new(&delete_id)
                    .label(if deleted { "Message Deleted" } else { "Delete Message" })
                    .emoji('🗑')
                    .style(serenity::ButtonStyle::Secondary)
                    .disabled(deleted || !perms.manage_messages()),
            );
        }
        buttons.extend([
            serenity::CreateButton::new(&untimeout_id).label("End Timeout").emoji('✅').style(serenity::ButtonStyle::Secondary).disabled(!perms.moderate_members()),
            serenity::CreateButton::new(&kick_id).label("Kick").emoji('👢').style(serenity::ButtonStyle::Danger).disabled(!perms.kick_members()),
            serenity::CreateButton::new(&ban_id).label("Ban").emoji('🔨').style(serenity::ButtonStyle::Danger).disabled(!perms.ban_members()),
        ]);
        let options = TimeoutLength::ALL
            .iter()
            .enumerate()
            .map(|(i, length)| serenity::CreateSelectMenuOption::new(length.name(), i.to_string()))
            .collect();
        let select = serenity::CreateSelectMenu::new(&timeout_id, serenity::CreateSelectMenuKind::String { options })
            .placeholder("⏳ Time out for...")
            .disabled(!perms.moderate_members());
        vec![serenity::CreateActionRow::Buttons(buttons), serenity::CreateActionRow::SelectMenu(select)]
    };

    let mut embed = serenity::CreateEmbed::new()
        .colour(COLOUR)
        .author(serenity::CreateEmbedAuthor::new(&user.name).icon_url(user.face()))
        .field("Member", format!("<@{}>", user.id), true);
    // What the audit log and the member's DM say it was about
    let (reason, why) = match &message {
        Some(message) => {
            let preview = if message.content.is_empty() { "*No text*".to_string() } else { truncate(&message.content, 1000) };
            embed = embed.title("Moderate Message").description(preview).field("Message", message.link(), true);
            (format!("{}: From the message {}", ctx.author().name, message.link()), Some(format!("Your message {}", message.link())))
        }
        None => {
            embed = embed.title("Moderate Member").thumbnail(user.face());
            (format!("{}: From the Moderate menu", ctx.author().name), None)
        }
    };
    let reason = truncate(&reason, MAX_REASON);
    let reply = ctx.send(poise::CreateReply::default().embed(embed).components(components(false))).await?;

    loop {
        let ids = [delete_id.clone(), kick_id.clone(), ban_id.clone(), timeout_id.clone(), untimeout_id.clone()];
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .filter(move |i| ids.contains(&i.data.custom_id))
            .timeout(MENU_TIMEOUT)
            .await;
        let Some(press) = press else { break };
        press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
        let id = press.data.custom_id.as_str();
        let guild_id = ctx.guild_id().unwrap();

        if let (true, Some(message)) = (id == delete_id, &message) {
            message.delete(ctx).await?;
            reply.edit(ctx, poise::CreateReply::default().components(components(true))).await?;
            say_private(ctx, "🗑️ The message has been deleted.").await?;
            continue;
        }

        if let Some(problem) = hierarchy_problem(ctx, user.id).await {
            say_private(ctx, problem).await?;
            continue;
        }
        if id == timeout_id {
            let serenity::ComponentInteractionDataKind::StringSelect { values } = &press.data.kind else { continue };
            let Some(length) = values.first().and_then(|v| v.parse::<usize>().ok()).and_then(|i| TimeoutLength::ALL.get(i)) else { continue };
            let until = time_out(ctx, user.id, *length, &reason).await?;
            notify(ctx, &user, &format!("timed out until <t:{until}:F>"), why.as_deref()).await;
            say_private(ctx, format!("⏳ <@{}> has been timed out until <t:{until}:F>.", user.id)).await?;
            continue;
        }
        if id == untimeout_id {
            let edit = serenity::EditMember::new().enable_communication().audit_log_reason(&reason);
            guild_id.edit_member(ctx, user.id, edit).await?;
            say_private(ctx, format!("✅ <@{}>'s timeout has ended.", user.id)).await?;
            continue;
        }

        // Kicking or banning ends the menu
        if id == kick_id {
            notify(ctx, &user, "kicked", why.as_deref()).await;
            guild_id.kick_with_reason(ctx, user.id, &reason).await?;
            say_private(ctx, format!("👢 <@{}> has been kicked.", user.id)).await?;
        } else {
            notify(ctx, &user, "banned", why.as_deref()).await;
            // From a message, their messages from the last day go too, that one included
            let days = if message.is_some() { 1 } else { 0 };
            guild_id.ban_with_reason(ctx, user.id, days, &reason).await?;
            say_private(ctx, format!("🔨 <@{}> has been banned.", user.id)).await?;
        }
        break;
    }

    // Already gone if someone dismissed it
    let _ = reply.edit(ctx, poise::CreateReply::default().components(vec![])).await;
    Ok(())
}
