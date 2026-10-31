//! Moderation cog: `/ban`, `/unban`, `/kick`, `/timeout` and `/untimeout`.

pub mod automod;
pub mod lockdown;
pub mod quarantine;
pub mod raid_protection;
pub mod staff_policy;
pub mod suspicious_accounts;

use ::serenity::all as serenity;

use crate::core::framework::{command, Choice, CreateReply};
use super::{truncate, Context, Error};

const COLOUR: u32 = 0xed4245;
/// Discord's limit on audit log reasons
const MAX_REASON: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Choice)]
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
    /// The lengths `/timeout` suggests
    const ALL: [Self; 7] = [Self::Minute, Self::FiveMinutes, Self::TenMinutes, Self::Hour, Self::Day, Self::Week, Self::Max];
}

/// Discord's longest timeout: 28 days
pub const MAX_TIMEOUT_SECS: i64 = 28 * 24 * 60 * 60;

/// Parses a length like "90m", "2h30m", "1d 12h" or "3 days" into seconds.
fn parse_duration(text: &str) -> Option<i64> {
    let text = text.to_lowercase();
    let mut total = 0i64;
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let number: i64 = rest[..digits].parse().ok()?;
        rest = rest[digits..].trim_start();
        let letters = rest.find(|c: char| !c.is_alphabetic()).unwrap_or(rest.len());
        let unit = match &rest[..letters] {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
            "d" | "day" | "days" => 24 * 60 * 60,
            "w" | "week" | "weeks" => 7 * 24 * 60 * 60,
            _ => return None,
        };
        total = total.checked_add(number.checked_mul(unit)?)?;
        rest = rest[letters..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }
    Some(total)
}

/// Suggests the usual lengths, plus whatever's been typed if it's a valid length.
async fn autocomplete_length(_ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let mut choices = vec![];
    if parse_duration(partial).is_some_and(|s| (1..=MAX_TIMEOUT_SECS).contains(&s)) {
        choices.push(serenity::AutocompleteChoice::new(partial.trim().to_string(), partial.trim().to_string()));
    }
    let partial = partial.to_lowercase();
    for length in TimeoutLength::ALL {
        if length.name().to_lowercase().contains(partial.trim()) || choices.is_empty() {
            choices.push(serenity::AutocompleteChoice::new(length.name(), length.name()));
        }
    }
    choices
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Choice)]
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
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
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
#[command(
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
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Unban a user
#[command(
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
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Kick a member
#[command(
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
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Time a member out, so they can't chat, react or join voice
#[command(
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
    #[description = "How long for: pick one or type your own, like 90m, 2h30m or 3 days (max 28 days)"]
    #[autocomplete = "autocomplete_length"]
    #[max_length = 50]
    length: String,
    #[description = "Why they're being timed out"]
    #[max_length = 400]
    reason: Option<String>,
) -> Result<(), Error> {
    if let Some(problem) = hierarchy_problem(ctx, member.user.id).await {
        return say_private(ctx, problem).await;
    }
    let Some(seconds) = parse_duration(&length).filter(|s| (1..=MAX_TIMEOUT_SECS).contains(s)) else {
        return say_private(ctx, "❌ That isn't a length I understand. Use something like `10m`, `2h30m` or `3 days`, up to 28 days.").await;
    };
    let reason = reason.as_deref();
    let until = time_out(ctx, member.user.id, seconds, &audit_reason(ctx, reason)).await?;
    notify(ctx, &member.user, &format!("timed out until <t:{until}:F>"), reason).await;
    let text = format!("<@{}> has been timed out until <t:{until}:F> (<t:{until}:R>).", member.user.id);
    ctx.send(CreateReply::default().embed(result_embed("⏳ Member Timed Out", text, reason))).await?;
    Ok(())
}

/// Times the member out, returning when it ends as a Unix timestamp.
async fn time_out(ctx: Context<'_>, user_id: serenity::UserId, seconds: i64, reason: &str) -> Result<i64, Error> {
    let until = serenity::Timestamp::now().unix_timestamp() + seconds;
    let edit = serenity::EditMember::new()
        .disable_communication_until_datetime(serenity::Timestamp::from_unix_timestamp(until)?)
        .audit_log_reason(reason);
    ctx.guild_id().unwrap().edit_member(ctx, user_id, edit).await?;
    Ok(until)
}

/// End a member's timeout
#[command(
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
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_parsed() {
        assert_eq!(parse_duration("90m"), Some(90 * 60));
        assert_eq!(parse_duration("2h30m"), Some(2 * 3600 + 30 * 60));
        assert_eq!(parse_duration("1d 12h"), Some(36 * 3600));
        assert_eq!(parse_duration("3 Days"), Some(3 * 86400));
        assert_eq!(parse_duration("1 week, 2 days"), Some(9 * 86400));
        assert_eq!(parse_duration("60 Seconds"), Some(60));
        // Every suggested length can be read back, and is allowed
        for length in TimeoutLength::ALL {
            assert!(parse_duration(length.name()).is_some_and(|s| (1..=MAX_TIMEOUT_SECS).contains(&s)));
        }
        assert_eq!(parse_duration("28 Days"), Some(MAX_TIMEOUT_SECS));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("soon"), None);
    }
}
