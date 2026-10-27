use poise::serenity_prelude as serenity;

use super::store::{Setup, Ticket};
use super::{is_handler, panel_components, panel_embed, set_member_access, FOLLOWUP_SECS, GREEN, RED};
use crate::cogs::{Context, Error};

async fn reply_embed(ctx: Context<'_>, colour: u32, text: impl Into<String>) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Setup your ticketing system message
#[poise::command(
    slash_command,
    rename = "ticket-setup",
    guild_only,
    ephemeral,
    category = "Tickets",
    required_permissions = "ADMINISTRATOR",
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn ticket_setup(
    ctx: Context<'_>,
    #[description = "Category to create ticket channels in"]
    #[channel_types("Category")]
    category: serenity::GuildChannel,
    #[description = "Channel to post the ticket panel in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
    #[description = "Channel to save transcripts in"]
    #[channel_types("Text")]
    transcript: serenity::GuildChannel,
    #[description = "Role that handles tickets"] handlers: serenity::Role,
    #[description = "Message shown on the ticket panel"] description: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap();

    let setup = Setup {
        category_id: category.id.get(),
        panel_channel_id: channel.id.get(),
        transcript_channel_id: transcript.id.get(),
        handler_role_id: handlers.id.get(),
        description: description.replace("\\n", "\n"),
    };
    ctx.data().tickets.lock().unwrap().save_setup(guild_id.get(), &setup)?;

    let (guild_name, icon_url) = ctx.guild().map(|g| (g.name.clone(), g.icon_url())).unwrap_or_default();
    let panel = serenity::CreateMessage::new()
        .embed(panel_embed(&guild_name, icon_url, &setup.description))
        .components(panel_components());
    channel.id.send_message(ctx, panel).await?;

    ctx.say(format!("✅ Ticket system set up. The panel is posted in <#{}>.", channel.id)).await?;
    Ok(())
}

/// Manage the ticket in this channel
#[poise::command(
    slash_command,
    guild_only,
    category = "Tickets",
    subcommands("add", "remove", "followup"),
    subcommand_required
)]
pub async fn ticket(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// The setup and open ticket for this channel, if the caller is a ticket handler.
/// Otherwise replies with the reason and returns `None`.
async fn handler_ticket(ctx: Context<'_>) -> Result<Option<(Setup, Ticket)>, Error> {
    let (setup, ticket) = {
        let tickets = ctx.data().tickets.lock().unwrap();
        (tickets.setup(ctx.guild_id().unwrap().get())?, tickets.open_ticket(ctx.channel_id().get())?)
    };
    let Some(setup) = setup else {
        reply_embed(ctx, RED, "🟥 | The ticket system isn't set up on this server.").await?;
        return Ok(None);
    };
    let Some(ticket) = ticket else {
        reply_embed(ctx, RED, "🟥 | This is not a ticket channel.").await?;
        return Ok(None);
    };

    let Some(member) = ctx.author_member().await else { return Ok(None) };
    let permissions = member.permissions.or_else(|| ctx.guild().map(|g| g.member_permissions(&member)));
    if !is_handler(&setup, &member.roles, permissions) {
        reply_embed(ctx, RED, "🟥 | Only ticket handlers can use this command.").await?;
        return Ok(None);
    }
    Ok(Some((setup, ticket)))
}

/// Add a member to the ticket.
#[poise::command(slash_command, guild_only, ephemeral, category = "Tickets")]
pub async fn add(ctx: Context<'_>, #[description = "Select a member."] member: serenity::Member) -> Result<(), Error> {
    let Some((_, mut ticket)) = handler_ticket(ctx).await? else { return Ok(()) };
    let user_id = member.user.id;

    if ticket.members.contains(&user_id.get()) {
        return reply_embed(ctx, RED, "🟥 | This member is already part of this ticket.").await;
    }

    set_member_access(ctx.http(), ctx.channel_id(), user_id, !ticket.locked).await?;
    ticket.members.push(user_id.get());
    ctx.data().tickets.lock().unwrap().set_members(ticket.channel_id, &ticket.members)?;

    let embed = serenity::CreateEmbed::new().colour(GREEN).description(format!("✅ | <@{user_id}> has been added to the ticket."));
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(false)).await?;
    Ok(())
}

/// Remove a member from the ticket.
#[poise::command(slash_command, guild_only, ephemeral, category = "Tickets")]
pub async fn remove(ctx: Context<'_>, #[description = "Select a member."] member: serenity::Member) -> Result<(), Error> {
    let Some((_, mut ticket)) = handler_ticket(ctx).await? else { return Ok(()) };
    let user_id = member.user.id;

    if user_id.get() == ticket.creator_id {
        return reply_embed(ctx, RED, "🟥 | You can't remove the member who opened the ticket.").await;
    }
    if !ticket.members.contains(&user_id.get()) {
        return reply_embed(ctx, RED, "🟥 | This member is not part of this ticket.").await;
    }

    ctx.channel_id().delete_permission(ctx, serenity::PermissionOverwriteType::Member(user_id)).await?;
    ticket.members.retain(|id| *id != user_id.get());
    ctx.data().tickets.lock().unwrap().set_members(ticket.channel_id, &ticket.members)?;

    let embed = serenity::CreateEmbed::new().colour(GREEN).description(format!("✅ | <@{user_id}> has been removed from the ticket."));
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(false)).await?;
    Ok(())
}

/// Post a follow-up message; the ticket closes in 48 hours unless someone replies.
#[poise::command(slash_command, guild_only, ephemeral, category = "Tickets")]
pub async fn followup(ctx: Context<'_>) -> Result<(), Error> {
    let Some((_, ticket)) = handler_ticket(ctx).await? else { return Ok(()) };

    let expires_at = chrono::Utc::now().timestamp() + FOLLOWUP_SECS;
    ctx.data().tickets.lock().unwrap().set_expiry(ticket.channel_id, expires_at)?;

    let name = ctx.author_member().await.map(|m| m.display_name().to_string()).unwrap_or_else(|| ctx.author().name.clone());
    let embed = serenity::CreateEmbed::new()
        .colour(GREEN)
        .description(format!(
            "We believe this issue is resolved and haven't heard otherwise. If you still need help, just reply here. \
             Otherwise this ticket will be closed automatically <t:{expires_at}:R>.\n\n\
             If you need more help later, feel free to open another ticket. We're always happy to help!\n\n\
             Kind regards,\n**{name}**"
        ));
    ctx.channel_id().send_message(ctx, serenity::CreateMessage::new().embed(embed)).await?;

    ctx.say("✅ Follow-up posted.").await?;
    Ok(())
}
