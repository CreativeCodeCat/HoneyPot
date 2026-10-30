//! Applications cog: admins create positions (like Moderator or Developer), each with up to
//! five questions of their own and an optional role. Members apply with `/apply`, answering
//! the questions in a pop-up, and the application is posted in the review channel with
//! Accept and Deny buttons. The applicant is told the result by DM, and accepting gives them
//! the position's role.

use poise::serenity_prelude as serenity;

use super::{truncate, Data, Error};
use crate::log;

pub use crate::database::repositories::applications::*;

pub const COG_NAME: &str = "applications";
/// Discord allows five text boxes in a pop-up
const MAX_QUESTIONS: usize = 5;
/// Discord's limit on a pop-up text box's label, which shows the question
const MAX_QUESTION_LEN: usize = 45;
const MAX_ANSWER_LEN: u16 = 1000;
/// How long the applicant has to fill in the pop-up
const FORM_TIME: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Button IDs start with this, then "apply:<position>", "accept:<application>" or "deny:<application>"
const BUTTON_PREFIX: &str = "application:";

const BLUE: u32 = 0x5865f2;
const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;

type AppContext<'a> = poise::ApplicationContext<'a, Data, Error>;

/// The pop-up for writing a position's questions.
#[derive(Debug, poise::Modal)]
#[name = "Application Questions"]
struct QuestionsForm {
    #[name = "Question 1"]
    #[placeholder = "Why do you want this position?"]
    #[max_length = 45]
    q1: String,
    #[name = "Question 2 (optional)"]
    #[placeholder = "How old are you?"]
    #[max_length = 45]
    q2: Option<String>,
    #[name = "Question 3 (optional)"]
    #[placeholder = "How many hours a week can you help?"]
    #[max_length = 45]
    q3: Option<String>,
    #[name = "Question 4 (optional)"]
    #[placeholder = "Do you have any past experience?"]
    #[max_length = 45]
    q4: Option<String>,
    #[name = "Question 5 (optional)"]
    #[placeholder = "What can you bring to the team?"]
    #[max_length = 45]
    q5: Option<String>,
}

impl QuestionsForm {
    fn questions(self) -> Vec<String> {
        [Some(self.q1), self.q2, self.q3, self.q4, self.q5]
            .into_iter()
            .flatten()
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .collect()
    }

    fn from_questions(questions: &[String]) -> Self {
        let q = |i: usize| questions.get(i).cloned();
        Self { q1: q(0).unwrap_or_default(), q2: q(1), q3: q(2), q4: q(3), q5: q(4) }
    }
}

fn guild_id(ctx: AppContext<'_>) -> u64 {
    ctx.guild_id().unwrap().get()
}

async fn say_private(ctx: AppContext<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Suggests the server's positions.
async fn autocomplete_position(ctx: AppContext<'_>, partial: &str) -> Vec<String> {
    let partial = partial.to_lowercase();
    let positions = ctx.data().applications.lock().unwrap().positions(guild_id(ctx)).unwrap_or_default();
    positions.into_iter().map(|p| p.name).filter(|name| name.to_lowercase().contains(&partial)).take(25).collect()
}

/// Set up applications: the review channel and the positions members can apply for
#[poise::command(
    slash_command,
    guild_only,
    category = "Applications",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("channel", "add", "edit", "remove", "list", "panel"),
    subcommand_required
)]
pub async fn application(_ctx: AppContext<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set the channel applications are posted in for review
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn channel(
    ctx: AppContext<'_>,
    #[description = "Where applications are posted, with Accept and Deny buttons"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    ctx.data().applications.lock().unwrap().set_channel(guild_id(ctx), channel.id.get())?;
    say_private(ctx, format!("✅ Applications will be posted in <#{}>.", channel.id)).await
}

/// Add a position members can apply for, writing its questions in a pop-up
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn add(
    ctx: AppContext<'_>,
    #[description = "The position, like Moderator or Developer"]
    #[max_length = 50]
    name: String,
    #[description = "A role to give members who are accepted"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    if ctx.data().applications.lock().unwrap().position(guild_id(ctx), &name)?.is_some() {
        return say_private(ctx, format!("❌ There's already a **{}** position. Change it with `/application edit`.", name.trim())).await;
    }
    let Some(form) = poise::execute_modal(ctx, None::<QuestionsForm>, None).await? else { return Ok(()) };
    let questions = form.questions();
    ctx.data().applications.lock().unwrap().save_position(guild_id(ctx), &name, role.as_ref().map(|r| r.id.get()), &questions)?;
    refresh_panels(ctx).await;
    let role = role.map_or(String::new(), |r| format!(" Accepted members get <@&{}>.", r.id));
    say_private(ctx, format!("✅ Added the **{}** position with {} question(s).{role} Members can apply with `/apply`.", name.trim(), questions.len())).await
}

/// Change a position's questions, and optionally its role
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn edit(
    ctx: AppContext<'_>,
    #[description = "The position to change"]
    #[autocomplete = "autocomplete_position"]
    position: String,
    #[description = "A new role to give accepted members (default: keep the current one)"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    let Some(current) = ctx.data().applications.lock().unwrap().position(guild_id(ctx), &position)? else {
        return say_private(ctx, "❌ There's no position with that name.").await;
    };
    let Some(form) = poise::execute_modal(ctx, Some(QuestionsForm::from_questions(&current.questions)), None).await? else { return Ok(()) };
    let role_id = role.map(|r| r.id.get()).or(current.role_id);
    ctx.data().applications.lock().unwrap().save_position(guild_id(ctx), &current.name, role_id, &form.questions())?;
    refresh_panels(ctx).await;
    say_private(ctx, format!("✅ Updated the **{}** position.", current.name)).await
}

/// Remove a position
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn remove(
    ctx: AppContext<'_>,
    #[description = "The position to remove"]
    #[autocomplete = "autocomplete_position"]
    position: String,
) -> Result<(), Error> {
    if ctx.data().applications.lock().unwrap().remove_position(guild_id(ctx), &position)? {
        refresh_panels(ctx).await;
        say_private(ctx, format!("✅ Removed the **{}** position. Any posted panels have been updated.", position.trim())).await
    } else {
        say_private(ctx, "❌ There's no position with that name.").await
    }
}

/// List the positions and their questions
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn list(ctx: AppContext<'_>) -> Result<(), Error> {
    let (positions, channel) = {
        let store = ctx.data().applications.lock().unwrap();
        (store.positions(guild_id(ctx))?, store.channel(guild_id(ctx))?)
    };
    let channel = channel.map_or("Not set. Use `/application channel`.".to_string(), |c| format!("<#{c}>"));
    if positions.is_empty() {
        return say_private(ctx, format!("There are no positions yet. Add one with `/application add`.\n**Review channel:** {channel}")).await;
    }
    let fields = positions
        .iter()
        .map(|p| {
            let role = p.role_id.map_or("No role".to_string(), |r| format!("Gives <@&{r}>"));
            let questions: Vec<String> = p.questions.iter().enumerate().map(|(i, q)| format!("{}. {q}", i + 1)).collect();
            (p.name.clone(), truncate(&format!("{role}\n{}", questions.join("\n")), 1024))
        })
        .collect();
    let page = || serenity::CreateEmbed::new().colour(BLUE).title("Application Positions").description(format!("**Review channel:** {channel}"));
    crate::pagination::paginate(ctx.into(), crate::pagination::field_pages(page, fields, 5, false)).await
}

/// Why a member can't apply for a position right now, or the position and review channel.
fn can_apply(data: &Data, guild: u64, user_id: u64, position: Option<Position>) -> Result<Result<(Position, u64), String>, Error> {
    let store = data.applications.lock().unwrap();
    let Some(channel) = store.channel(guild)? else {
        return Ok(Err("❌ Applications aren't set up on this server yet.".into()));
    };
    let Some(position) = position else {
        return Ok(Err("❌ That position doesn't exist any more.".into()));
    };
    if position.questions.is_empty() {
        return Ok(Err("❌ That position has no questions yet.".into()));
    }
    if store.has_pending(guild, user_id, position.id)? {
        return Ok(Err(format!("❌ You've already applied for **{}**. Wait for it to be reviewed.", position.name)));
    }
    Ok(Ok((position, channel)))
}

/// The pop-up asking a position's own questions.
fn application_form(position: &Position, modal_id: &str) -> serenity::CreateInteractionResponse {
    let rows = position
        .questions
        .iter()
        .take(MAX_QUESTIONS)
        .enumerate()
        .map(|(i, question)| {
            let input = serenity::CreateInputText::new(serenity::InputTextStyle::Paragraph, truncate(question, MAX_QUESTION_LEN), format!("q{i}"))
                .max_length(MAX_ANSWER_LEN)
                .required(true);
            serenity::CreateActionRow::InputText(input)
        })
        .collect();
    let title = truncate(&format!("{} Application", position.name), 45);
    serenity::CreateInteractionResponse::Modal(serenity::CreateModal::new(modal_id, title).components(rows))
}

/// Waits for the application pop-up, then posts the answers for review and tells the
/// applicant how it went.
async fn finish_application(
    ctx: &serenity::Context,
    data: &Data,
    guild: u64,
    applicant: &serenity::User,
    position: &Position,
    channel: u64,
    modal_id: String,
) -> Result<(), Error> {
    let submit = serenity::ModalInteractionCollector::new(ctx)
        .filter(move |i| i.data.custom_id == modal_id)
        .timeout(FORM_TIME)
        .await;
    let Some(submit) = submit else { return Ok(()) };
    let answers: Vec<String> = submit
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .filter_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) => input.value.clone(),
            _ => None,
        })
        .collect();

    let user_id = applicant.id.get();
    let id = data.applications.lock().unwrap().add_application(guild, user_id, position.id)?;
    let mut embed = serenity::CreateEmbed::new()
        .colour(BLUE)
        .title(format!("{} Application", position.name))
        .author(serenity::CreateEmbedAuthor::new(applicant.tag()).icon_url(applicant.face()))
        .description(format!("Sent by <@{user_id}> • Account created <t:{}:R>", applicant.id.created_at().unix_timestamp()))
        .footer(serenity::CreateEmbedFooter::new(format!("Application #{id} • Pending")))
        .timestamp(serenity::Timestamp::now());
    for (question, answer) in position.questions.iter().zip(&answers) {
        embed = embed.field(question, truncate(answer, 1024), false);
    }
    let buttons = serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{BUTTON_PREFIX}accept:{id}")).label("Accept").style(serenity::ButtonStyle::Success),
        serenity::CreateButton::new(format!("{BUTTON_PREFIX}deny:{id}")).label("Deny").style(serenity::ButtonStyle::Danger),
    ]);
    let posted = serenity::ChannelId::new(channel)
        .send_message(ctx, serenity::CreateMessage::new().embed(embed).components(vec![buttons]))
        .await;

    let reply = match posted {
        Ok(_) => format!("✅ Your application for **{}** was submitted. You'll get a DM when it's reviewed.", position.name),
        Err(e) => {
            log::error(format!("Error posting application in guild {guild}: {e}"));
            // Not posted, so it can't be reviewed; let them apply again
            data.applications.lock().unwrap().decide(id, Status::Denied)?;
            "❌ Your application couldn't be posted. Please tell a staff member.".to_string()
        }
    };
    let message = serenity::CreateInteractionResponseMessage::new().content(reply).ephemeral(true);
    submit.create_response(ctx, serenity::CreateInteractionResponse::Message(message)).await?;
    Ok(())
}

/// Apply for a position, answering its questions in a pop-up
#[poise::command(slash_command, guild_only, category = "Applications")]
pub async fn apply(
    ctx: AppContext<'_>,
    #[description = "The position to apply for"]
    #[autocomplete = "autocomplete_position"]
    position: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx);
    let found = ctx.data().applications.lock().unwrap().position(guild, &position)?;
    let (position, channel) = match can_apply(ctx.data(), guild, ctx.author().id.get(), found)? {
        Ok(ok) => ok,
        Err(problem) => return say_private(ctx, problem).await,
    };
    let modal_id = format!("{}apply", ctx.id());
    ctx.interaction.create_response(ctx, application_form(&position, &modal_id)).await?;
    ctx.has_sent_initial_response.store(true, std::sync::atomic::Ordering::SeqCst);
    finish_application(ctx.serenity_context(), ctx.data(), guild, ctx.author(), &position, channel, modal_id).await
}

/// A panel's embed and buttons for the guild's current positions.
fn panel_content(positions: &[Position]) -> (serenity::CreateEmbed, Vec<serenity::CreateActionRow>) {
    let list = if positions.is_empty() {
        "There are no open positions right now. Check back later!".to_string()
    } else {
        let lines: Vec<String> = positions
            .iter()
            .map(|p| match p.role_id {
                Some(role) => format!("• **{}** (<@&{role}>)", p.name),
                None => format!("• **{}**", p.name),
            })
            .collect();
        format!("**Open positions:**\n{}", lines.join("\n"))
    };
    let embed = serenity::CreateEmbed::new()
        .colour(BLUE)
        .title("📝 Applications")
        .description(format!("Want to join the team? Press a button below to apply, and answer a few questions.\n\n{list}"));
    // Up to 25 buttons, five to a row
    let buttons: Vec<serenity::CreateButton> = positions
        .iter()
        .take(25)
        .map(|p| {
            serenity::CreateButton::new(format!("{BUTTON_PREFIX}apply:{}", p.id))
                .label(truncate(&p.name, 80))
                .emoji('📝')
                .style(serenity::ButtonStyle::Primary)
        })
        .collect();
    (embed, buttons.chunks(5).map(|row| serenity::CreateActionRow::Buttons(row.to_vec())).collect())
}

/// Updates every posted panel to the guild's current positions, forgetting deleted ones.
async fn refresh_panels(ctx: AppContext<'_>) {
    let guild = guild_id(ctx);
    let (positions, panels) = {
        let store = ctx.data().applications.lock().unwrap();
        (store.positions(guild).unwrap_or_default(), store.panels(guild).unwrap_or_default())
    };
    for (channel_id, message_id) in panels {
        let (embed, rows) = panel_content(&positions);
        let edit = serenity::EditMessage::new().embed(embed).components(rows);
        match serenity::ChannelId::new(channel_id).edit_message(ctx, message_id, edit).await {
            Ok(_) => {}
            // The panel or its channel was deleted
            Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(resp))) if resp.status_code.as_u16() == 404 => {
                let _ = ctx.data().applications.lock().unwrap().remove_panel(message_id);
            }
            Err(e) => log::error(format!("Error updating application panel {message_id} in guild {guild}: {e}")),
        }
    }
}

/// Post the panel with an Apply button for each position, replacing the old one
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn panel(
    ctx: AppContext<'_>,
    #[description = "Where to post the panel"]
    #[channel_types("Text", "News")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    // Posted even with no positions yet; it fills in as they're added
    let (positions, old) = {
        let store = ctx.data().applications.lock().unwrap();
        (store.positions(guild_id(ctx))?, store.panels(guild_id(ctx))?)
    };
    let (embed, rows) = panel_content(&positions);
    let message = channel.send_message(ctx, serenity::CreateMessage::new().embed(embed).components(rows)).await?;

    // A server has one panel, so this replaces the old one
    for (old_channel, old_message) in &old {
        // Already gone if someone deleted it
        let _ = serenity::ChannelId::new(*old_channel).delete_message(ctx, *old_message).await;
        ctx.data().applications.lock().unwrap().remove_panel(*old_message)?;
    }
    ctx.data().applications.lock().unwrap().add_panel(guild_id(ctx), channel.id.get(), message.id.get())?;
    let replaced = if old.is_empty() { "" } else { " The old panel has been removed." };
    let more = if positions.len() > 25 { " Only the first 25 positions fit as buttons." } else { "" };
    say_private(ctx, format!("✅ Posted the application panel in <#{}>.{replaced}{more} It updates itself when positions change.", channel.id)).await
}

/// Handles the panel's Apply buttons, and the Accept and Deny buttons on posted
/// applications. They work after restarts, as their IDs hold the position's or application's ID.
pub async fn on_interaction(ctx: &serenity::Context, data: &Data, interaction: &serenity::Interaction) {
    let serenity::Interaction::Component(press) = interaction else { return };
    let Some(rest) = press.data.custom_id.strip_prefix(BUTTON_PREFIX) else { return };
    let Some(guild_id) = press.guild_id else { return };
    let result = match rest.strip_prefix("apply:") {
        Some(position_id) => apply_button(ctx, data, press, guild_id, position_id).await,
        None => review(ctx, data, press, guild_id, rest).await,
    };
    if let Err(e) = result {
        log::error(format!("Error handling an application button in guild {guild_id}: {e}"));
    }
}

/// A panel's Apply button: the same as `/apply` for that position.
async fn apply_button(
    ctx: &serenity::Context,
    data: &Data,
    press: &serenity::ComponentInteraction,
    guild_id: serenity::GuildId,
    position_id: &str,
) -> Result<(), Error> {
    let private = |text: String| {
        serenity::CreateInteractionResponse::Message(serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        press.create_response(ctx, private("❌ The Applications cog is disabled on this server.".into())).await?;
        return Ok(());
    }
    let found = match position_id.parse::<i64>() {
        Ok(id) => data.applications.lock().unwrap().position_by_id(id)?,
        Err(_) => None,
    };
    let (position, channel) = match can_apply(data, guild_id.get(), press.user.id.get(), found)? {
        Ok(ok) => ok,
        Err(problem) => {
            press.create_response(ctx, private(problem)).await?;
            return Ok(());
        }
    };
    let modal_id = format!("{}apply", press.id);
    press.create_response(ctx, application_form(&position, &modal_id)).await?;
    finish_application(ctx, data, guild_id.get(), &press.user, &position, channel, modal_id).await
}

async fn review(
    ctx: &serenity::Context,
    data: &Data,
    press: &serenity::ComponentInteraction,
    guild_id: serenity::GuildId,
    action: &str,
) -> Result<(), Error> {
    let private = |text: &str| {
        serenity::CreateInteractionResponse::Message(serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        press.create_response(ctx, private("❌ The Applications cog is disabled on this server.")).await?;
        return Ok(());
    }
    let can_review = press.member.as_ref().and_then(|m| m.permissions).is_some_and(|p| p.manage_guild() || p.administrator());
    if !can_review {
        press.create_response(ctx, private("❌ You need the **Manage Server** permission to review applications.")).await?;
        return Ok(());
    }
    let Some((kind, id)) = action.split_once(':') else { return Ok(()) };
    let Ok(id) = id.parse::<i64>() else { return Ok(()) };
    let status = if kind == "accept" { Status::Accepted } else { Status::Denied };

    let decided = data.applications.lock().unwrap().decide(id, status)?;
    let Some((_, applicant, position_id)) = decided else {
        press.create_response(ctx, private("❌ This application has already been reviewed.")).await?;
        return Ok(());
    };
    let position = data.applications.lock().unwrap().position_by_id(position_id)?;
    let position_name = position.as_ref().map_or("a position".to_string(), |p| p.name.clone());
    let applicant = serenity::UserId::new(applicant);
    let server = ctx.cache.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();

    // Accepted members get the position's role, if it has one
    let mut notes = vec![];
    if status == Status::Accepted {
        if let Some(role) = position.as_ref().and_then(|p| p.role_id) {
            let reason = format!("Application #{id} accepted by {}", press.user.tag());
            match ctx.http.add_member_role(guild_id, applicant, serenity::RoleId::new(role), Some(&reason)).await {
                Ok(()) => notes.push(format!("Given <@&{role}>.")),
                Err(e) => notes.push(format!("⚠️ Couldn't give <@&{role}>: {e}")),
            }
        }
    }
    let dm = match status {
        Status::Accepted => format!("🎉 Your application for **{position_name}** in **{server}** has been **accepted**!"),
        _ => format!("Your application for **{position_name}** in **{server}** was **denied**. Thank you for applying."),
    };
    let dm_sent = match applicant.create_dm_channel(ctx).await {
        Ok(channel) => channel.say(ctx, dm).await.is_ok(),
        Err(_) => false,
    };
    if !dm_sent {
        notes.push("⚠️ Their DMs are closed, so they weren't told.".to_string());
    }

    // Mark the posted application, keeping its answers
    let (colour, label) = match status {
        Status::Accepted => (GREEN, "Accepted"),
        _ => (RED, "Denied"),
    };
    let original = press.message.embeds.first().cloned();
    let mut embed = original.map(serenity::CreateEmbed::from).unwrap_or_default().colour(colour).footer(serenity::CreateEmbedFooter::new(format!(
        "Application #{id} • {label} by {}",
        press.user.tag()
    )));
    if !notes.is_empty() {
        embed = embed.field("Notes", notes.join("\n"), false);
    }
    let update = serenity::CreateInteractionResponseMessage::new().embed(embed).components(vec![]);
    press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_applications() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.channel(1).unwrap(), None);
        s.set_channel(1, 10).unwrap();
        assert_eq!(s.channel(1).unwrap(), Some(10));

        let questions = vec!["Why?".to_string(), "Age?".to_string()];
        s.save_position(1, "Moderator", Some(5), &questions).unwrap();
        let position = s.position(1, "moderator").unwrap().unwrap();
        assert_eq!((position.name.as_str(), position.role_id, position.questions.clone()), ("Moderator", Some(5), questions));
        s.save_position(1, "Moderator", None, &["New?".to_string()]).unwrap();
        assert_eq!(s.positions(1).unwrap().len(), 1);

        let id = s.add_application(1, 2, position.id).unwrap();
        assert!(s.has_pending(1, 2, position.id).unwrap());
        assert_eq!(s.decide(id, Status::Accepted).unwrap(), Some((1, 2, position.id)));
        // Can't be decided twice
        assert_eq!(s.decide(id, Status::Denied).unwrap(), None);
        assert!(!s.has_pending(1, 2, position.id).unwrap());

        s.add_panel(1, 10, 100).unwrap();
        assert_eq!(s.panels(1).unwrap(), vec![(10, 100)]);
        s.remove_panel(100).unwrap();
        assert!(s.panels(1).unwrap().is_empty());

        assert!(s.remove_position(1, "MODERATOR").unwrap());
        assert!(!s.remove_position(1, "Moderator").unwrap());
    }

    #[test]
    fn questions_form_skips_blanks() {
        let form = QuestionsForm { q1: "Why?".into(), q2: Some("  ".into()), q3: None, q4: Some("Hours?".into()), q5: None };
        assert_eq!(form.questions(), vec!["Why?".to_string(), "Hours?".to_string()]);
        assert_eq!(QuestionsForm::from_questions(&["A".into()]).q1, "A");
    }
}
