//! Suggestions cog: members post suggestions with `/suggest`, which go to the server's
//! suggestions channel for others to vote on with reactions. Admins then accept,
//! decline, complete or reopen them with `/suggestion`, which updates the post,
//! announces the result and, if the member asked, DMs them.

use ::serenity::all as serenity;

use crate::core::framework::{command, Choice, CreateReply};
use super::{Context, Error};

pub use crate::database::repositories::suggestions::*;

const GREEN: u32 = 0x57f287;
const PURPLE: u32 = 0x9b59b6;
const RED: u32 = 0xed4245;

#[derive(Clone, Copy, Debug, Choice)]
enum Kind {
    Command,
    Event,
    System,
    Other,
}

async fn reply(ctx: Context<'_>, colour: u32, text: impl Into<String>) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// The setup for this server, or a reply saying it hasn't been done.
async fn setup(ctx: Context<'_>) -> Result<Option<Setup>, Error> {
    let setup = ctx.data().suggestions.lock().unwrap().setup(ctx.guild_id().unwrap().get())?;
    if setup.is_none() {
        reply(ctx, RED, "❌ This server hasn't set up suggestions yet. An admin can run `/suggestion setup`.").await?;
    }
    Ok(setup)
}

/// The suggestion's post, with its status (and a reason or date) filled in.
fn post(author: &serenity::User, suggestion: &Suggestion, colour: u32, status: &str, extra: Option<(&str, &str)>) -> serenity::CreateEmbed {
    let mut embed = serenity::CreateEmbed::new()
        .colour(colour)
        .author(serenity::CreateEmbedAuthor::new(author.tag()).icon_url(author.face()))
        .description(format!("**Suggestion:**\n{}", suggestion.text))
        .field("Type", &suggestion.kind, true)
        .field("Status", status, true);
    if let Some((name, value)) = extra {
        embed = embed.field(name, value, true);
    }
    embed
}

/// Submit a suggestion
#[command(slash_command, guild_only, ephemeral, category = "Suggestions")]
pub async fn suggest(
    ctx: Context<'_>,
    #[description = "What kind of suggestion it is"]
    #[rename = "type"]
    kind: Kind,
    #[description = "Describe your suggestion"]
    #[max_length = 4000]
    suggestion: String,
    #[description = "DM you once your suggestion has been accepted or declined"] dm: bool,
) -> Result<(), Error> {
    let Some(setup) = setup(ctx).await? else { return Ok(()) };
    let guild_id = ctx.guild_id().unwrap().get();

    let mut suggestion = Suggestion {
        message_id: 0,
        member_id: ctx.author().id.get(),
        kind: Choice::name(&kind).to_string(),
        text: suggestion,
        dm,
    };
    let message = serenity::CreateMessage::new().embed(post(ctx.author(), &suggestion, PURPLE, "🕐 Pending", None));
    let message = match serenity::ChannelId::new(setup.channel_id).send_message(ctx, message).await {
        Ok(message) => message,
        Err(e) => {
            crate::log::error(format!("Error posting suggestion in guild {guild_id}: {e}"));
            return reply(ctx, RED, "❌ Couldn't post in the suggestions channel. Ask an admin to check its permissions.").await;
        }
    };
    for emoji in ['✅', '❌'] {
        message.react(ctx, emoji).await?;
    }
    suggestion.message_id = message.id.get();
    ctx.data().suggestions.lock().unwrap().add(guild_id, &suggestion)?;
    reply(ctx, PURPLE, format!("✅ Your [suggestion]({}) was sent to <#{}>.", message.link(), setup.channel_id)).await
}

/// Manage suggestions
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "Suggestions",
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR",
    subcommands("setup_cmd", "accept", "decline", "pending", "completed"),
    subcommand_required
)]
pub async fn suggestion(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Choose where suggestions go and where results are announced
#[command(slash_command, guild_only, ephemeral, rename = "setup", required_permissions = "ADMINISTRATOR")]
async fn setup_cmd(
    ctx: Context<'_>,
    #[description = "Channel suggestions are posted in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
    #[description = "Channel accepted suggestions are announced in"]
    #[channel_types("Text")]
    accepted: serenity::GuildChannel,
    #[description = "Channel declined suggestions are announced in"]
    #[channel_types("Text")]
    declined: serenity::GuildChannel,
) -> Result<(), Error> {
    let setup = Setup { channel_id: channel.id.get(), accepted_channel_id: accepted.id.get(), declined_channel_id: declined.id.get() };
    ctx.data().suggestions.lock().unwrap().save_setup(ctx.guild_id().unwrap().get(), &setup)?;
    reply(
        ctx,
        GREEN,
        format!("✅ Suggestions go to <#{}>; accepted ones are announced in <#{}> and declined ones in <#{}>.", channel.id, accepted.id, declined.id),
    )
    .await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Accepted,
    Declined,
    Pending,
    Completed,
}

/// Updates a suggestion's post, then announces it and DMs the member as needed.
async fn decide(ctx: Context<'_>, message_id: String, verdict: Verdict, reason: Option<String>) -> Result<(), Error> {
    let Some(setup) = setup(ctx).await? else { return Ok(()) };
    let guild_id = ctx.guild_id().unwrap();
    let Ok(message_id) = message_id.trim().parse::<u64>() else {
        return reply(ctx, RED, "❌ That isn't a message ID. Right-click the suggestion and choose Copy Message ID.").await;
    };
    let Some(suggestion) = ctx.data().suggestions.lock().unwrap().get(guild_id.get(), message_id)? else {
        return reply(ctx, RED, "❌ That suggestion wasn't found.").await;
    };
    let channel = serenity::ChannelId::new(setup.channel_id);
    let Ok(mut message) = channel.message(ctx, message_id).await else {
        return reply(ctx, RED, "❌ That suggestion's message wasn't found in the suggestions channel.").await;
    };
    let author = serenity::UserId::new(suggestion.member_id).to_user(ctx).await?;
    let reason = reason.unwrap_or_else(|| "No reason given".to_string());
    let link = message.link();

    let (colour, status, word, emoji) = match verdict {
        Verdict::Accepted => (GREEN, "Accepted", "accepted", "✅"),
        Verdict::Declined => (RED, "Declined", "declined", "❎"),
        Verdict::Pending => (PURPLE, "🕐 Pending", "set back to pending", "🕐"),
        Verdict::Completed => (GREEN, "Completed", "completed", "✅"),
    };
    let date = chrono::Utc::now().format("%a %b %d %Y").to_string();
    let extra = if verdict == Verdict::Completed { ("Date", date.as_str()) } else { ("Reason", reason.as_str()) };
    message.edit(ctx, serenity::EditMessage::new().embed(post(&author, &suggestion, colour, status, Some(extra)))).await?;

    let result = |whose: &str| {
        let mut embed = serenity::CreateEmbed::new()
            .colour(colour)
            .title("Suggestion 💡")
            .description(format!("{whose} suggestion was {word} {emoji}"))
            .field("Suggestion", format!("[link]({link})"), true);
        if whose == "Your" {
            let guild_name = ctx.guild().map(|g| g.name.clone()).unwrap_or_default();
            embed = embed.field("Server", guild_name, true);
        }
        if verdict != Verdict::Completed {
            embed = embed.field("Reason", &reason, true);
        }
        embed
    };

    let announce = match verdict {
        Verdict::Accepted => Some(setup.accepted_channel_id),
        Verdict::Declined => Some(setup.declined_channel_id),
        _ => None,
    };
    let mut warning = String::new();
    if let Some(id) = announce {
        if serenity::ChannelId::new(id).send_message(ctx, serenity::CreateMessage::new().embed(result("The"))).await.is_err() {
            warning = format!("\n⚠️ Couldn't announce it in <#{id}>.");
        }
    }
    if verdict == Verdict::Completed {
        message.delete_reactions(ctx).await?;
        ctx.data().suggestions.lock().unwrap().remove(guild_id.get(), message_id)?;
    } else if suggestion.dm {
        // Members with DMs closed just don't hear about it
        let _ = author.direct_message(ctx, serenity::CreateMessage::new().embed(result("Your"))).await;
    }
    reply(ctx, colour, format!("[Suggestion]({link}) was {word} {emoji}{warning}")).await
}

/// Accept a suggestion
#[command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn accept(
    ctx: Context<'_>,
    #[description = "Message ID of the suggestion"]
    #[rename = "message-id"]
    message_id: String,
    #[description = "Why it was accepted"] reason: Option<String>,
) -> Result<(), Error> {
    decide(ctx, message_id, Verdict::Accepted, reason).await
}

/// Decline a suggestion
#[command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn decline(
    ctx: Context<'_>,
    #[description = "Message ID of the suggestion"]
    #[rename = "message-id"]
    message_id: String,
    #[description = "Why it was declined"] reason: Option<String>,
) -> Result<(), Error> {
    decide(ctx, message_id, Verdict::Declined, reason).await
}

/// Set a suggestion back to pending
#[command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn pending(
    ctx: Context<'_>,
    #[description = "Message ID of the suggestion"]
    #[rename = "message-id"]
    message_id: String,
    #[description = "Why it's pending again"] reason: Option<String>,
) -> Result<(), Error> {
    decide(ctx, message_id, Verdict::Pending, reason).await
}

/// Mark a suggestion as done, which closes its voting
#[command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn completed(
    ctx: Context<'_>,
    #[description = "Message ID of the suggestion"]
    #[rename = "message-id"]
    message_id: String,
) -> Result<(), Error> {
    decide(ctx, message_id, Verdict::Completed, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_are_saved_per_server() {
        let s = Store::open_path(":memory:").unwrap();
        let setup = Setup { channel_id: 1, accepted_channel_id: 2, declined_channel_id: 3 };
        s.save_setup(10, &setup).unwrap();
        assert_eq!(s.setup(10).unwrap(), Some(setup));
        assert_eq!(s.setup(11).unwrap(), None);

        let suggestion = Suggestion { message_id: 5, member_id: 6, kind: "Command".into(), text: "More games".into(), dm: true };
        s.add(10, &suggestion).unwrap();
        assert_eq!(s.get(10, 5).unwrap(), Some(suggestion));
        assert_eq!(s.get(11, 5).unwrap(), None);

        s.cleanup_stale(&[serenity::GuildId::new(11)]).unwrap();
        assert_eq!((s.setup(10).unwrap(), s.get(10, 5).unwrap()), (None, None));
    }
}
