//! Rules cog: each server writes its own rules sections, and `/rules post` posts them as
//! embeds. Posting again deletes the previous rules messages first.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use super::{Data, Error};
use crate::log;

type AppContext<'a> = poise::ApplicationContext<'a, Data, Error>;

const DB_FILE: &str = "database/rules.db";
const DEFAULT_COLOUR: u32 = 0xaa5bbe;
/// Embeds per message (Discord's limit is 10)
const EMBEDS_PER_MESSAGE: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub body: String,
    pub colour: u32,
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
            "CREATE TABLE IF NOT EXISTS sections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                colour INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS posts (
                guild_id INTEGER PRIMARY KEY,
                channel_id INTEGER NOT NULL,
                -- JSON list of the message IDs currently posted
                message_ids TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    /// Sections in display order.
    pub fn sections(&self, guild_id: u64) -> rusqlite::Result<Vec<Section>> {
        let mut stmt =
            self.conn.prepare("SELECT title, body, colour FROM sections WHERE guild_id = ? ORDER BY position, id")?;
        let rows = stmt.query_map([guild_id], |row| Ok(Section { title: row.get(0)?, body: row.get(1)?, colour: row.get(2)? }))?;
        rows.collect()
    }

    /// Row ID of the section at a 1-based position.
    fn section_id(&self, guild_id: u64, number: usize) -> rusqlite::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM sections WHERE guild_id = ? ORDER BY position, id LIMIT 1 OFFSET ?",
                params![guild_id, number.saturating_sub(1) as i64],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn add(&self, guild_id: u64, section: &Section) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO sections (guild_id, position, title, body, colour)
             VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM sections WHERE guild_id = ?1), ?2, ?3, ?4)",
            params![guild_id, section.title, section.body, section.colour],
        )?;
        Ok(())
    }

    /// Replaces section `number` (1-based). Returns false if there's no such section.
    pub fn update(&self, guild_id: u64, number: usize, section: &Section) -> rusqlite::Result<bool> {
        let Some(id) = self.section_id(guild_id, number)? else { return Ok(false) };
        self.conn.execute(
            "UPDATE sections SET title = ?, body = ?, colour = ? WHERE id = ?",
            params![section.title, section.body, section.colour, id],
        )?;
        Ok(true)
    }

    /// Deletes section `number` (1-based). Returns false if there's no such section.
    pub fn remove(&self, guild_id: u64, number: usize) -> rusqlite::Result<bool> {
        let Some(id) = self.section_id(guild_id, number)? else { return Ok(false) };
        self.conn.execute("DELETE FROM sections WHERE id = ?", [id])?;
        Ok(true)
    }

    /// The channel and messages of the current rules post.
    pub fn post(&self, guild_id: u64) -> rusqlite::Result<Option<(u64, Vec<u64>)>> {
        self.conn
            .query_row("SELECT channel_id, message_ids FROM posts WHERE guild_id = ?", [guild_id], |row| {
                let ids: String = row.get(1)?;
                Ok((row.get(0)?, serde_json::from_str(&ids).unwrap_or_default()))
            })
            .optional()
    }

    pub fn save_post(&self, guild_id: u64, channel_id: u64, message_ids: &[u64]) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO posts (guild_id, channel_id, message_ids) VALUES (?, ?, ?)",
            params![guild_id, channel_id, serde_json::to_string(message_ids).unwrap()],
        )?;
        Ok(())
    }
}

/// The form for writing a section.
#[derive(Debug, poise::Modal)]
#[name = "Rules section"]
struct SectionForm {
    #[name = "Title"]
    #[placeholder = ":white_check_mark: Our Server Rules"]
    #[max_length = 256]
    title: String,
    #[name = "Text"]
    #[placeholder = "1. Be respectful.\n2. No spam."]
    #[paragraph]
    #[max_length = 4000]
    body: String,
    #[name = "Colour (hex, optional)"]
    #[placeholder = "#aa5bbe"]
    #[max_length = 7]
    colour: Option<String>,
}

impl SectionForm {
    fn from_section(section: &Section) -> Self {
        Self {
            title: section.title.clone(),
            body: section.body.clone(),
            colour: Some(format!("#{:06x}", section.colour)),
        }
    }

    /// The section, or an error message if the colour isn't valid hex.
    fn into_section(self) -> Result<Section, String> {
        let colour = match self.colour.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            None => DEFAULT_COLOUR,
            Some(hex) => u32::from_str_radix(hex.trim_start_matches('#'), 16)
                .ok()
                .filter(|c| *c <= 0xffffff)
                .ok_or_else(|| format!("❌ `{hex}` isn't a valid colour. Use hex like `#aa5bbe`."))?,
        };
        Ok(Section { title: self.title.trim().to_string(), body: self.body.trim().to_string(), colour })
    }
}

fn embed(section: &Section) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new().title(&section.title).description(&section.body).colour(section.colour)
}

fn guild_id(ctx: AppContext<'_>) -> u64 {
    ctx.guild_id().expect("rules commands are guild_only").get()
}

async fn say_private(ctx: AppContext<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Manage and post this server's rules
#[poise::command(
    slash_command,
    guild_only,
    category = "Rules",
    subcommands("add", "edit", "remove", "list", "post"),
    subcommand_required,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn rules(_ctx: AppContext<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add a rules section
#[poise::command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn add(ctx: AppContext<'_>) -> Result<(), Error> {
    let Some(form) = poise::execute_modal(ctx, None::<SectionForm>, None).await? else { return Ok(()) };
    let section = match form.into_section() {
        Ok(section) => section,
        Err(message) => return say_private(ctx, message).await,
    };

    let count = {
        let rules = ctx.data().rules.lock().unwrap();
        rules.add(guild_id(ctx), &section)?;
        rules.sections(guild_id(ctx))?.len()
    };
    let reply = poise::CreateReply::default()
        .content(format!("✅ Added section {count}. Run `/rules post` to update the rules channel."))
        .embed(embed(&section))
        .ephemeral(true);
    ctx.send(reply).await?;
    Ok(())
}

/// Edit a rules section
#[poise::command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn edit(
    ctx: AppContext<'_>,
    #[description = "Section number, as shown in /rules list"]
    #[min = 1]
    number: usize,
) -> Result<(), Error> {
    let current = ctx.data().rules.lock().unwrap().sections(guild_id(ctx))?.get(number.wrapping_sub(1)).cloned();
    let Some(current) = current else {
        return say_private(ctx, format!("❌ There's no section {number}. Use `/rules list` to see them.")).await;
    };

    let form = poise::execute_modal(ctx, Some(SectionForm::from_section(&current)), None).await?;
    let Some(form) = form else { return Ok(()) };
    let section = match form.into_section() {
        Ok(section) => section,
        Err(message) => return say_private(ctx, message).await,
    };

    ctx.data().rules.lock().unwrap().update(guild_id(ctx), number, &section)?;
    let reply = poise::CreateReply::default()
        .content(format!("✅ Updated section {number}. Run `/rules post` to update the rules channel."))
        .embed(embed(&section))
        .ephemeral(true);
    ctx.send(reply).await?;
    Ok(())
}

/// Remove a rules section
#[poise::command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn remove(
    ctx: AppContext<'_>,
    #[description = "Section number, as shown in /rules list"]
    #[min = 1]
    number: usize,
) -> Result<(), Error> {
    let removed = ctx.data().rules.lock().unwrap().remove(guild_id(ctx), number)?;
    if removed {
        say_private(ctx, format!("✅ Removed section {number}. Run `/rules post` to update the rules channel.")).await
    } else {
        say_private(ctx, format!("❌ There's no section {number}. Use `/rules list` to see them.")).await
    }
}

/// List the rules sections
#[poise::command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn list(ctx: AppContext<'_>) -> Result<(), Error> {
    let (sections, post) = {
        let rules = ctx.data().rules.lock().unwrap();
        (rules.sections(guild_id(ctx))?, rules.post(guild_id(ctx))?)
    };
    if sections.is_empty() {
        return say_private(ctx, "There are no rules sections yet. Add one with `/rules add`.").await;
    }

    let mut text: String =
        sections.iter().enumerate().map(|(i, s)| format!("**{}.** {}\n", i + 1, s.title)).collect();
    if let Some((channel_id, _)) = post {
        text += &format!("\nLast posted in <#{channel_id}>.");
    }
    let embed = serenity::CreateEmbed::new().title("Rules sections").description(text).colour(DEFAULT_COLOUR);
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Post the rules, replacing the previous rules messages
#[poise::command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn post(
    ctx: AppContext<'_>,
    #[description = "Channel to post the rules in"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let gid = guild_id(ctx);
    let (sections, previous) = {
        let rules = ctx.data().rules.lock().unwrap();
        (rules.sections(gid)?, rules.post(gid)?)
    };

    if sections.is_empty() {
        return say_private(ctx, "❌ There are no rules sections yet. Add one with `/rules add`.").await;
    }
    let channel_id = channel.id;

    // Delete the previous rules messages, wherever they were posted
    let mut deleted = 0;
    if let Some((old_channel, old_ids)) = previous {
        let old_channel = serenity::ChannelId::new(old_channel);
        for id in old_ids {
            match old_channel.delete_message(ctx, serenity::MessageId::new(id)).await {
                Ok(()) => deleted += 1,
                // Already deleted by hand: nothing to do
                Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r))) if r.status_code.as_u16() == 404 => {}
                Err(e) => log::warning(format!("[RULES] Could not delete old rules message {id}: {e}")),
            }
        }
    }

    // Post the sections, up to 10 embeds per message
    let mut message_ids = vec![];
    for chunk in sections.chunks(EMBEDS_PER_MESSAGE) {
        let message = serenity::CreateMessage::new().embeds(chunk.iter().map(embed).collect());
        match channel_id.send_message(ctx, message).await {
            Ok(message) => message_ids.push(message.id.get()),
            Err(e) => {
                // Remember what did get posted so the next run still cleans it up
                ctx.data().rules.lock().unwrap().save_post(gid, channel_id.get(), &message_ids)?;
                return say_private(ctx, format!("❌ Couldn't post in <#{channel_id}>: {e}")).await;
            }
        }
    }
    ctx.data().rules.lock().unwrap().save_post(gid, channel_id.get(), &message_ids)?;

    let replaced = if deleted > 0 { format!(" and removed the old ones ({deleted} messages)") } else { String::new() };
    say_private(ctx, format!("✅ Posted {} rules sections in <#{channel_id}>{replaced}.", sections.len())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(title: &str) -> Section {
        Section { title: title.to_string(), body: "body".to_string(), colour: DEFAULT_COLOUR }
    }

    #[test]
    fn sections_keep_order_and_numbering() {
        let s = Store::open_path(":memory:").unwrap();
        for title in ["Welcome", "Disclaimer", "Rules"] {
            s.add(1, &section(title)).unwrap();
        }
        s.add(2, &section("Other server")).unwrap();

        assert!(s.update(1, 2, &section("Our Disclaimer")).unwrap());
        assert!(s.remove(1, 1).unwrap());
        assert!(!s.remove(1, 9).unwrap());
        s.add(1, &section("Chats")).unwrap();

        let titles: Vec<String> = s.sections(1).unwrap().into_iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Our Disclaimer", "Rules", "Chats"]);
        assert_eq!(s.sections(2).unwrap().len(), 1);
    }

    #[test]
    fn remembers_the_last_post() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.post(1).unwrap(), None);
        s.save_post(1, 50, &[100, 101]).unwrap();
        s.save_post(1, 51, &[200]).unwrap();
        assert_eq!(s.post(1).unwrap(), Some((51, vec![200])));
    }

    #[test]
    fn parses_colours() {
        let form = |colour: Option<&str>| SectionForm { title: " T ".into(), body: "B".into(), colour: colour.map(Into::into) };
        assert_eq!(form(None).into_section().unwrap().colour, DEFAULT_COLOUR);
        assert_eq!(form(Some("")).into_section().unwrap().colour, DEFAULT_COLOUR);
        assert_eq!(form(Some("#A441C4")).into_section().unwrap().colour, 0xa441c4);
        assert_eq!(form(Some("43a441")).into_section().unwrap().colour, 0x43a441);
        assert!(form(Some("blue")).into_section().is_err());
        assert_eq!(form(None).into_section().unwrap().title, "T");
    }
}
