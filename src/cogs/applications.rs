//! Applications cog: admins create positions (like Moderator or Developer), each with up to
//! five questions of their own and an optional role. Members apply with `/apply`, answering
//! the questions in a pop-up, and the application is posted in the review channel with
//! Accept and Deny buttons. The applicant is told the result by DM, and accepting gives them
//! the position's role.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use super::{now_iso, truncate, Data, Error};
use crate::log;

pub const COG_NAME: &str = "applications";
const DB_FILE: &str = "database/applications.db";
/// Discord allows five text boxes in a pop-up
const MAX_QUESTIONS: usize = 5;
/// Discord's limit on a pop-up text box's label, which shows the question
const MAX_QUESTION_LEN: usize = 45;
const MAX_ANSWER_LEN: u16 = 1000;
/// How long the applicant has to fill in the pop-up
const FORM_TIME: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Review buttons' IDs start with this, then "accept:<id>" or "deny:<id>"
const BUTTON_PREFIX: &str = "application:";

const BLUE: u32 = 0x5865f2;
const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;

type AppContext<'a> = poise::ApplicationContext<'a, Data, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub id: i64,
    pub name: String,
    /// Given to accepted applicants
    pub role_id: Option<u64>,
    pub questions: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// A decision on an application; new ones are pending until one is made
pub enum Status {
    Accepted,
    Denied,
}

impl Status {
    fn key(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Denied => "denied",
        }
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (
                guild_id INTEGER PRIMARY KEY,
                -- Where applications are posted for review
                channel_id INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS positions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER NOT NULL,
                name TEXT NOT NULL COLLATE NOCASE,
                role_id INTEGER,
                -- JSON array of question strings
                questions TEXT NOT NULL,
                UNIQUE (guild_id, name)
            );
            CREATE TABLE IF NOT EXISTS applications (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                position_id INTEGER NOT NULL,
                -- 'pending', 'accepted' or 'denied'
                status TEXT NOT NULL,
                created TEXT NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    pub fn channel(&self, guild_id: u64) -> rusqlite::Result<Option<u64>> {
        self.conn.query_row("SELECT channel_id FROM settings WHERE guild_id = ?", [guild_id], |row| row.get(0)).optional()
    }

    pub fn set_channel(&self, guild_id: u64, channel_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO settings (guild_id, channel_id) VALUES (?, ?)", [guild_id, channel_id])?;
        Ok(())
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Position> {
        let questions: String = row.get(3)?;
        Ok(Position { id: row.get(0)?, name: row.get(1)?, role_id: row.get(2)?, questions: serde_json::from_str(&questions).unwrap_or_default() })
    }

    pub fn positions(&self, guild_id: u64) -> rusqlite::Result<Vec<Position>> {
        let mut stmt = self.conn.prepare("SELECT id, name, role_id, questions FROM positions WHERE guild_id = ? ORDER BY name")?;
        let rows = stmt.query_map([guild_id], Self::from_row)?;
        rows.collect()
    }

    /// A position by name, ignoring case.
    pub fn position(&self, guild_id: u64, name: &str) -> rusqlite::Result<Option<Position>> {
        self.conn
            .query_row("SELECT id, name, role_id, questions FROM positions WHERE guild_id = ? AND name = ?", params![guild_id, name.trim()], Self::from_row)
            .optional()
    }

    fn position_by_id(&self, id: i64) -> rusqlite::Result<Option<Position>> {
        self.conn.query_row("SELECT id, name, role_id, questions FROM positions WHERE id = ?", [id], Self::from_row).optional()
    }

    /// Adds a position, or replaces the one with the same name.
    pub fn save_position(&self, guild_id: u64, name: &str, role_id: Option<u64>, questions: &[String]) -> rusqlite::Result<()> {
        let questions = serde_json::to_string(questions).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "INSERT INTO positions (guild_id, name, role_id, questions) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, name) DO UPDATE SET role_id = ?3, questions = ?4",
            params![guild_id, name.trim(), role_id, questions],
        )?;
        Ok(())
    }

    /// Returns false if there was no such position.
    pub fn remove_position(&self, guild_id: u64, name: &str) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM positions WHERE guild_id = ? AND name = ?", params![guild_id, name.trim()])? > 0)
    }

    /// Whether the member already has an application for the position waiting for review.
    pub fn has_pending(&self, guild_id: u64, user_id: u64, position_id: i64) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM applications WHERE guild_id = ? AND user_id = ? AND position_id = ? AND status = 'pending'",
            params![guild_id, user_id, position_id],
            |row| row.get(0),
        )
    }

    /// Records a new application, returning its ID.
    pub fn add_application(&self, guild_id: u64, user_id: u64, position_id: i64) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO applications (guild_id, user_id, position_id, status, created) VALUES (?, ?, ?, 'pending', ?)",
            params![guild_id, user_id, position_id, now_iso()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Decides a pending application, returning its (guild, applicant, position), or `None`
    /// if it was already decided or doesn't exist.
    pub fn decide(&self, id: i64, status: Status) -> rusqlite::Result<Option<(u64, u64, i64)>> {
        self.conn
            .query_row(
                "UPDATE applications SET status = ? WHERE id = ? AND status = 'pending' RETURNING guild_id, user_id, position_id",
                params![status.key(), id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
    }

    /// Removes everything for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!(
            "DELETE FROM settings WHERE guild_id NOT IN ({ids});
             DELETE FROM positions WHERE guild_id NOT IN ({ids});
             DELETE FROM applications WHERE guild_id NOT IN ({ids});"
        ))
    }
}

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
    subcommands("channel", "add", "edit", "remove", "list"),
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
        say_private(ctx, format!("✅ Removed the **{}** position.", position.trim())).await
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

/// Apply for a position, answering its questions in a pop-up
#[poise::command(slash_command, guild_only, category = "Applications")]
pub async fn apply(
    ctx: AppContext<'_>,
    #[description = "The position to apply for"]
    #[autocomplete = "autocomplete_position"]
    position: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx);
    let user_id = ctx.author().id.get();
    let (position, channel) = {
        let store = ctx.data().applications.lock().unwrap();
        (store.position(guild, &position)?, store.channel(guild)?)
    };
    let Some(channel) = channel else {
        return say_private(ctx, "❌ Applications aren't set up on this server yet.").await;
    };
    let Some(position) = position else {
        return say_private(ctx, "❌ There's no position with that name. Pick one from the list.").await;
    };
    if position.questions.is_empty() {
        return say_private(ctx, "❌ That position has no questions yet.").await;
    }
    if ctx.data().applications.lock().unwrap().has_pending(guild, user_id, position.id)? {
        return say_private(ctx, format!("❌ You've already applied for **{}**. Wait for it to be reviewed.", position.name)).await;
    }

    // The pop-up is built from the position's own questions
    let modal_id = format!("{}apply", ctx.id());
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
    let modal = serenity::CreateModal::new(&modal_id, title).components(rows);
    ctx.interaction.create_response(ctx, serenity::CreateInteractionResponse::Modal(modal)).await?;
    ctx.has_sent_initial_response.store(true, std::sync::atomic::Ordering::SeqCst);

    let submit = serenity::ModalInteractionCollector::new(ctx.serenity_context())
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

    let id = ctx.data().applications.lock().unwrap().add_application(guild, user_id, position.id)?;
    let author = ctx.author();
    let mut embed = serenity::CreateEmbed::new()
        .colour(BLUE)
        .title(format!("{} Application", position.name))
        .author(serenity::CreateEmbedAuthor::new(author.tag()).icon_url(author.face()))
        .description(format!("Sent by <@{user_id}> • Account created <t:{}:R>", author.id.created_at().unix_timestamp()))
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
            ctx.data().applications.lock().unwrap().decide(id, Status::Denied)?;
            "❌ Your application couldn't be posted. Please tell a staff member.".to_string()
        }
    };
    let message = serenity::CreateInteractionResponseMessage::new().content(reply).ephemeral(true);
    submit.create_response(ctx, serenity::CreateInteractionResponse::Message(message)).await?;
    Ok(())
}

/// Handles the Accept and Deny buttons on posted applications. They work after restarts, as
/// their IDs hold the application's ID.
pub async fn on_interaction(ctx: &serenity::Context, data: &Data, interaction: &serenity::Interaction) {
    let serenity::Interaction::Component(press) = interaction else { return };
    let Some(rest) = press.data.custom_id.strip_prefix(BUTTON_PREFIX) else { return };
    let Some(guild_id) = press.guild_id else { return };
    if let Err(e) = review(ctx, data, press, guild_id, rest).await {
        log::error(format!("Error reviewing application in guild {guild_id}: {e}"));
    }
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
