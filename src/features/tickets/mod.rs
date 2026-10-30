//! Tickets cog: a dropdown panel opens private ticket channels that handlers can
//! claim, lock and close. Closed tickets are saved as HTML transcripts.

pub mod commands;

pub use crate::database::repositories::tickets as store;
mod transcript;

use std::time::Duration;

use chrono::Utc;
use ::serenity::all as serenity;
use serenity::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage,
};

use super::{Data, Error};
use crate::log;
use store::{Setup, Ticket};

pub const COG_NAME: &str = "tickets";

/// Open tickets with no messages for this long are closed automatically
pub const IDLE_SECS: i64 = 7 * 24 * 60 * 60;
/// After `/ticket followup`, the ticket closes this long after the last message
pub const FOLLOWUP_SECS: i64 = 48 * 60 * 60;
/// Delay between confirming a close and the channel being deleted
const CLOSE_DELAY: Duration = Duration::from_secs(10);

const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;
const PURPLE: u32 = 0x9b59b6;
const BLURPLE: u32 = 0x5865f2;

const PANEL_ID: &str = "TicketSystem";
const MODAL_PREFIX: &str = "ticket_modal:";
const REASON_ID: &str = "reason";

const CLOSE: &str = "close_report";
const CLOSE_YES: &str = "close_report_yes";
const CLOSE_NO: &str = "close_report_no";
const LOCK: &str = "lock_report";
const UNLOCK: &str = "unlock_report";
const CLAIM: &str = "claim_report";
const TRANSCRIPT: &str = "send_transcript";
const BUTTON_IDS: &[&str] = &[CLOSE, CLOSE_YES, CLOSE_NO, LOCK, UNLOCK, CLAIM, TRANSCRIPT];

/// (type, description, emoji)
const TICKET_TYPES: &[(&str, &str, &str)] = &[
    ("Support", "Create a support ticket.", "📱"),
    ("Purchase", "Create a purchase ticket.", "💵"),
    ("Other", "Anything else that doesn't fit the options above.", "🎟️"),
];

fn emoji(e: &str) -> serenity::ReactionType {
    serenity::ReactionType::Unicode(e.to_string())
}

fn button(id: &str, label: &str, style: ButtonStyle, e: &str) -> CreateButton {
    CreateButton::new(id).label(label).style(style).emoji(emoji(e))
}

pub fn panel_embed(guild_name: &str, icon_url: Option<String>, description: &str) -> CreateEmbed {
    let mut author = serenity::CreateEmbedAuthor::new(format!("{guild_name} | Ticket System"));
    if let Some(icon) = icon_url {
        author = author.icon_url(icon);
    }
    CreateEmbed::new()
        .author(author)
        .colour(BLURPLE)
        .description(description)
        .footer(serenity::CreateEmbedFooter::new("Click the dropdown to get started!"))
}

pub fn panel_components() -> Vec<CreateActionRow> {
    let options = TICKET_TYPES
        .iter()
        .map(|(value, description, e)| serenity::CreateSelectMenuOption::new(*value, *value).description(*description).emoji(emoji(e)))
        .collect();
    let menu = serenity::CreateSelectMenu::new(PANEL_ID, serenity::CreateSelectMenuKind::String { options })
        .placeholder("Make a selection!");
    vec![CreateActionRow::SelectMenu(menu)]
}

fn ticket_buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        button(CLOSE, "Save & Close", ButtonStyle::Danger, "💾"),
        button(LOCK, "Lock", ButtonStyle::Secondary, "🔒"),
        button(UNLOCK, "Unlock", ButtonStyle::Secondary, "🔓"),
        button(CLAIM, "Claim", ButtonStyle::Success, "🛄"),
        button(TRANSCRIPT, "Transcript", ButtonStyle::Primary, "📒"),
    ])]
}

fn confirm_buttons(disabled: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        button(CLOSE_YES, "Yes", ButtonStyle::Secondary, "✅").disabled(disabled),
        button(CLOSE_NO, "No", ButtonStyle::Secondary, "❌").disabled(disabled),
    ])]
}

/// Ticket handlers are members with the handler role, or administrators.
pub fn is_handler(setup: &Setup, roles: &[serenity::RoleId], permissions: Option<serenity::Permissions>) -> bool {
    roles.iter().any(|r| r.get() == setup.handler_role_id) || permissions.is_some_and(|p| p.administrator())
}

/// Gives a member access to a ticket channel, optionally read-only.
pub async fn set_member_access(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    user_id: serenity::UserId,
    can_send: bool,
) -> Result<(), Error> {
    use serenity::Permissions as P;
    let view = P::VIEW_CHANNEL | P::READ_MESSAGE_HISTORY;
    let overwrite = serenity::PermissionOverwrite {
        allow: if can_send { view | P::SEND_MESSAGES } else { view },
        deny: if can_send { P::empty() } else { P::SEND_MESSAGES },
        kind: serenity::PermissionOverwriteType::Member(user_id),
    };
    channel_id.create_permission(http, overwrite).await?;
    Ok(())
}

fn now() -> i64 {
    Utc::now().timestamp()
}

fn closed_embed(ticket: &Ticket, closed_by: serenity::UserId) -> CreateEmbed {
    let claimed_by = ticket.claimed_by.map(|id| format!("<@{id}>")).unwrap_or_else(|| "Not claimed.".to_string());
    CreateEmbed::new()
        .title("Ticket Closed!")
        .colour(RED)
        .field(
            "Ticket Information:",
            format!(
                "**Created By:** <@{}>\n**Ticket ID:** #{}\n**Ticket Type:** {}\n**Claimed By:** {claimed_by}\n**Closed By:** <@{closed_by}>",
                ticket.creator_id,
                ticket.number(),
                ticket.ticket_type,
            ),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new("The ticket was closed at"))
        .timestamp(serenity::Timestamp::now())
}

/// Saves the transcript to the transcript channel, then deletes the ticket channel and
/// forgets the ticket.
pub async fn close_ticket(ctx: &serenity::Context, data: &Data, ticket: &Ticket, closed_by: serenity::UserId) {
    let setup = {
        let tickets = data.tickets.lock().unwrap();
        match tickets.mark_closed(ticket.channel_id) {
            Ok(true) => {}
            Ok(false) => return, // already closed
            Err(e) => {
                log::error(format!("[TICKETS] Error closing ticket {}: {e}", ticket.ticket_id));
                return;
            }
        }
        tickets.setup(ticket.guild_id).ok().flatten()
    };

    let channel_id = serenity::ChannelId::new(ticket.channel_id);
    let filename = format!("{}_{}.html", ticket.ticket_type, ticket.ticket_id);
    let title = format!("{} ticket #{}", ticket.ticket_type, ticket.number());

    if let Some(setup) = setup {
        let mut message = serenity::CreateMessage::new().embed(closed_embed(ticket, closed_by));
        match transcript::build(&ctx.http, channel_id, &title, filename).await {
            Ok(file) => message = message.add_file(file),
            Err(e) => log::warning(format!("[TICKETS] Could not build transcript for {}: {e}", ticket.ticket_id)),
        }
        if let Err(e) = serenity::ChannelId::new(setup.transcript_channel_id).send_message(&ctx.http, message).await {
            log::error(format!("[TICKETS] Could not post transcript for {}: {e}", ticket.ticket_id));
        }
    }

    if let Err(e) = channel_id.delete(&ctx.http).await {
        log::warning(format!("[TICKETS] Could not delete ticket channel {}: {e}", ticket.channel_id));
    }
    // Only once the transcript is posted, as it's built from the channel. Until then it's
    // kept, marked closed, so another close can't run at the same time.
    if let Err(e) = data.tickets.lock().unwrap().delete(ticket.channel_id) {
        log::error(format!("[TICKETS] Error removing closed ticket {}: {e}", ticket.ticket_id));
    }
    log::info(format!("[TICKETS] Closed ticket {} in guild {}", ticket.ticket_id, ticket.guild_id));
}

async fn reply(ctx: &serenity::Context, c: &serenity::ComponentInteraction, message: CreateInteractionResponseMessage) -> Result<(), Error> {
    c.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await?;
    Ok(())
}

async fn reply_ephemeral(ctx: &serenity::Context, c: &serenity::ComponentInteraction, text: &str) -> Result<(), Error> {
    reply(ctx, c, CreateInteractionResponseMessage::new().content(text).ephemeral(true)).await
}

async fn reply_embed(ctx: &serenity::Context, c: &serenity::ComponentInteraction, colour: u32, text: String) -> Result<(), Error> {
    reply(ctx, c, CreateInteractionResponseMessage::new().embed(CreateEmbed::new().colour(colour).description(text))).await
}

pub async fn on_interaction(ctx: &serenity::Context, data: &Data, interaction: &serenity::Interaction) {
    let result = match interaction {
        serenity::Interaction::Component(c) if c.data.custom_id == PANEL_ID || BUTTON_IDS.contains(&c.data.custom_id.as_str()) => {
            handle_component(ctx, data, c).await
        }
        serenity::Interaction::Modal(m) if m.data.custom_id.starts_with(MODAL_PREFIX) => handle_modal(ctx, data, m).await,
        _ => return,
    };
    if let Err(e) = result {
        log::error(format!("[TICKETS] Error handling interaction: {e}"));
    }
}

async fn handle_component(ctx: &serenity::Context, data: &Data, c: &serenity::ComponentInteraction) -> Result<(), Error> {
    let Some(guild_id) = c.guild_id else { return Ok(()) };
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return reply_ephemeral(ctx, c, "❌ The Tickets cog is disabled on this server.").await;
    }

    let (setup, ticket) = {
        let tickets = data.tickets.lock().unwrap();
        (tickets.setup(guild_id.get())?, tickets.open_ticket(c.channel_id.get())?)
    };
    let Some(setup) = setup else {
        return reply_ephemeral(ctx, c, "❌ The ticket system isn't set up on this server. An admin can run /ticket-setup.").await;
    };

    if c.data.custom_id == PANEL_ID {
        return open_ticket_form(ctx, c).await;
    }

    let Some(ticket) = ticket else {
        return reply_ephemeral(ctx, c, "No data was found for this ticket, please delete manually.").await;
    };

    let user = &c.user;
    let (roles, permissions) = c.member.as_ref().map(|m| (m.roles.clone(), m.permissions)).unwrap_or_default();
    if c.data.custom_id != TRANSCRIPT && !is_handler(&setup, &roles, permissions) {
        return reply_ephemeral(ctx, c, "You can not use these buttons.").await;
    }

    let channel_id = c.channel_id;
    match c.data.custom_id.as_str() {
        CLOSE => {
            let embed = CreateEmbed::new()
                .title("Confirm")
                .description("Are you sure you want to close this ticket?")
                .footer(serenity::CreateEmbedFooter::new(format!("Action taken by: {}", user.tag())));
            reply(ctx, c, CreateInteractionResponseMessage::new().embed(embed).components(confirm_buttons(false))).await?;
        }
        CLOSE_YES => {
            // Disable the confirm buttons so the close can't be triggered twice
            let embed = CreateEmbed::new().colour(RED).description(format!("The ticket has been closed by **{}**", user.tag()));
            let update = CreateInteractionResponseMessage::new().embed(embed).components(confirm_buttons(true));
            c.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await?;

            let closing = CreateEmbed::new()
                .colour(RED)
                .description(format!("The ticket is closing, please wait {} seconds until it gets deleted.", CLOSE_DELAY.as_secs()));
            let disabled = vec![CreateActionRow::Buttons(vec![
                CreateButton::new("ticket-close").label("Close Ticket").style(ButtonStyle::Danger).disabled(true),
            ])];
            let edit = serenity::EditMessage::new().embed(closing).components(disabled);
            if let Err(e) = channel_id.edit_message(&ctx.http, ticket.message_id, edit).await {
                log::warning(format!("[TICKETS] Could not update ticket message: {e}"));
            }

            let (ctx, data, closed_by) = (ctx.clone(), data.clone(), user.id);
            tokio::spawn(async move {
                tokio::time::sleep(CLOSE_DELAY).await;
                close_ticket(&ctx, &data, &ticket, closed_by).await;
            });
        }
        CLOSE_NO => {
            let embed = CreateEmbed::new().title("Action Cancelled").description("This ticket won't be closed.").colour(GREEN);
            let update = CreateInteractionResponseMessage::new().embed(embed).components(confirm_buttons(true));
            c.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await?;

            let (http, message_id) = (ctx.http.clone(), c.message.id);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let _ = channel_id.delete_message(&http, message_id).await;
            });
        }
        LOCK | UNLOCK => {
            let lock = c.data.custom_id == LOCK;
            if ticket.locked == lock {
                let state = if lock { "locked" } else { "unlocked" };
                return reply_ephemeral(ctx, c, &format!("The ticket is already {state}.")).await;
            }
            data.tickets.lock().unwrap().set_locked(channel_id.get(), lock)?;
            for member in &ticket.members {
                set_member_access(&ctx.http, channel_id, serenity::UserId::new(*member), !lock).await?;
            }
            let text = if lock { "🔒 | This ticket is now locked for review." } else { "🔓 | This ticket is now unlocked." };
            reply_embed(ctx, c, PURPLE, text.to_string()).await?;
        }
        CLAIM => match ticket.claimed_by {
            Some(id) if id == user.id.get() => reply_ephemeral(ctx, c, "You have already claimed this ticket.").await?,
            Some(id) => reply_ephemeral(ctx, c, &format!("This ticket has already been claimed by <@{id}>")).await?,
            None => {
                data.tickets.lock().unwrap().set_claimed_by(channel_id.get(), user.id.get())?;
                reply_embed(ctx, c, PURPLE, format!("🛄 | This ticket has been claimed by <@{}>", user.id)).await?;
            }
        },
        TRANSCRIPT => {
            c.defer_ephemeral(&ctx.http).await?;
            let title = format!("{} ticket #{}", ticket.ticket_type, ticket.number());
            let channel_name = channel_id.name(ctx).await.unwrap_or_else(|_| ticket.ticket_id.clone());
            let file = transcript::build(&ctx.http, channel_id, &title, format!("{channel_name}.html")).await?;
            let text = match user.direct_message(ctx, serenity::CreateMessage::new().add_file(file)).await {
                Ok(_) => "📒 The transcript has been sent to your DMs.",
                Err(_) => "❌ I couldn't DM you. Check that you allow DMs from server members.",
            };
            c.edit_response(&ctx.http, serenity::EditInteractionResponse::new().content(text)).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Shows the "reason" form for the selected ticket type.
async fn open_ticket_form(ctx: &serenity::Context, c: &serenity::ComponentInteraction) -> Result<(), Error> {
    let serenity::ComponentInteractionDataKind::StringSelect { values } = &c.data.kind else { return Ok(()) };
    let Some((ticket_type, _, _)) = values.first().and_then(|v| TICKET_TYPES.iter().find(|(t, _, _)| t == v)) else {
        return Ok(());
    };

    let input = serenity::CreateInputText::new(serenity::InputTextStyle::Short, "What's the reason you want to contact us", REASON_ID)
        .max_length(1000);
    let modal = serenity::CreateModal::new(format!("{MODAL_PREFIX}{ticket_type}"), *ticket_type)
        .components(vec![CreateActionRow::InputText(input)]);
    c.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;

    // Reset the dropdown so the same option can be picked again
    let mut panel = (*c.message).clone();
    if let Err(e) = panel.edit(ctx, serenity::EditMessage::new().components(panel_components())).await {
        log::warning(format!("[TICKETS] Could not reset ticket panel: {e}"));
    }
    Ok(())
}

/// Creates the ticket channel once the form is submitted.
async fn handle_modal(ctx: &serenity::Context, data: &Data, m: &serenity::ModalInteraction) -> Result<(), Error> {
    let Some(guild_id) = m.guild_id else { return Ok(()) };
    let ticket_type = m.data.custom_id.trim_start_matches(MODAL_PREFIX).to_string();
    let reason = m
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == REASON_ID => input.value.clone(),
            _ => None,
        })
        .unwrap_or_default();

    m.defer_ephemeral(&ctx.http).await?;

    let (setup, number) = {
        let tickets = data.tickets.lock().unwrap();
        (tickets.setup(guild_id.get())?, tickets.next_number(guild_id.get())?)
    };
    let Some(setup) = setup else {
        m.edit_response(&ctx.http, serenity::EditInteractionResponse::new().content("❌ The ticket system isn't set up.")).await?;
        return Ok(());
    };

    use serenity::Permissions as P;
    let access = P::VIEW_CHANNEL | P::SEND_MESSAGES | P::READ_MESSAGE_HISTORY;
    let user = &m.user;
    let bot_id = ctx.cache.current_user().id;
    let overwrite = |kind, allow, deny| serenity::PermissionOverwrite { allow, deny, kind };
    let overwrites = vec![
        // @everyone shares the guild's ID
        overwrite(serenity::PermissionOverwriteType::Role(guild_id.everyone_role()), P::empty(), access),
        overwrite(serenity::PermissionOverwriteType::Role(serenity::RoleId::new(setup.handler_role_id)), access, P::empty()),
        overwrite(serenity::PermissionOverwriteType::Member(user.id), access, P::empty()),
        // Keep the bot's own access even when it isn't an administrator
        overwrite(serenity::PermissionOverwriteType::Member(bot_id), access | P::MANAGE_CHANNELS | P::MANAGE_ROLES, P::empty()),
    ];

    let ticket_id = format!("{number}_{}", user.name);
    let channel = guild_id
        .create_channel(
            &ctx.http,
            serenity::CreateChannel::new(format!("{ticket_id}_{ticket_type}_ticket"))
                .kind(serenity::ChannelType::Text)
                .category(serenity::ChannelId::new(setup.category_id))
                .topic(format!("{ticket_type} Ticket"))
                .permissions(overwrites),
        )
        .await?;

    let embed = CreateEmbed::new()
        .title(format!("{ticket_type} Ticket"))
        .description(format!(
            "Hey <@{}>! One of our support agents will get back to you shortly. Please note that mentioning people **IS NOT** allowed and might result in your ticket being closed.",
            user.id
        ))
        .field("Reason for Contact:", reason, false)
        .colour(GREEN)
        .timestamp(serenity::Timestamp::now());
    let message = channel.send_message(&ctx.http, serenity::CreateMessage::new().embed(embed).components(ticket_buttons())).await?;

    let ticket = Ticket {
        guild_id: guild_id.get(),
        channel_id: channel.id.get(),
        message_id: message.id.get(),
        ticket_id,
        ticket_type,
        creator_id: user.id.get(),
        members: vec![user.id.get()],
        locked: false,
        claimed_by: None,
    };
    data.tickets.lock().unwrap().create(&ticket, now() + IDLE_SECS)?;

    // Ping the creator so they get a notification, then tidy it away
    let ping = channel.send_message(&ctx.http, serenity::CreateMessage::new().content(format!("<@{}> Here is your ticket!", user.id))).await?;
    let http = ctx.http.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = ping.delete(&http).await;
    });

    let embed = CreateEmbed::new()
        .title("Ticket")
        .description(format!("Your ticket was just opened and can be found here: <#{}>.", channel.id))
        .colour(GREEN);
    m.edit_response(&ctx.http, serenity::EditInteractionResponse::new().embed(embed)).await?;
    log::info(format!("[TICKETS] Opened ticket {} in guild {guild_id}", ticket.ticket_id));
    Ok(())
}

/// Any message in a ticket pushes back its inactivity auto-close.
pub fn on_message(data: &Data, msg: &serenity::Message) {
    let Some(guild_id) = msg.guild_id else { return };
    if msg.author.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    if let Err(e) = data.tickets.lock().unwrap().set_expiry(msg.channel_id.get(), now() + IDLE_SECS) {
        log::error(format!("[TICKETS] Error updating ticket activity: {e}"));
    }
}

/// Closes a member's open tickets when they leave the server.
pub async fn on_member_leave(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user: &serenity::User) {
    if user.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let open = data.tickets.lock().unwrap().open_tickets_by_creator(guild_id.get(), user.id.get());
    match open {
        Ok(open) => {
            let bot_id = ctx.cache.current_user().id;
            for ticket in open {
                close_ticket(ctx, data, &ticket, bot_id).await;
            }
        }
        Err(e) => log::error(format!("[TICKETS] Error finding tickets for departed member: {e}")),
    }
}

pub async fn auto_close_loop(ctx: serenity::Context, data: Data) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        let expired = data.tickets.lock().unwrap().expired_tickets(now());
        let expired = match expired {
            Ok(expired) => expired,
            Err(e) => {
                log::error(format!("[TICKETS] Error checking for inactive tickets: {e}"));
                continue;
            }
        };
        let bot_id = ctx.cache.current_user().id;
        for ticket in expired {
            if data.is_cog_enabled(ticket.guild_id, COG_NAME) {
                close_ticket(&ctx, &data, &ticket, bot_id).await;
            }
        }
    }
}
