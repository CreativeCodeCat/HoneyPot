//! Rules cog: each server writes its own rules sections, and `/rules post` posts them as
//! embeds. Posting again deletes the previous rules messages first.

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply, execute_modal, create_modal, find_modal_text, Modal, TextField};
use super::{Context, Error};
use crate::log;

pub use crate::database::repositories::rules::*;


const DEFAULT_COLOUR: u32 = 0xaa5bbe;
/// Embeds per message (Discord's limit is 10)
const EMBEDS_PER_MESSAGE: usize = 10;

/// The form for writing a section.
#[derive(Debug)]
struct SectionForm {
    title: String,
    body: String,
    colour: Option<String>,
}

impl Modal for SectionForm {
    fn create(defaults: Option<Self>, custom_id: String) -> serenity::CreateInteractionResponse {
        let (title, body, colour) = match defaults {
            Some(form) => (Some(form.title), Some(form.body), form.colour),
            None => (None, None, None),
        };
        let fields = vec![
            TextField { id: "title", label: "Title", placeholder: ":white_check_mark: Our Server Rules", max_length: 256, paragraph: false, required: true, value: title },
            TextField { id: "body", label: "Text", placeholder: "1. Be respectful.\n2. No spam.", max_length: 4000, paragraph: true, required: true, value: body },
            TextField { id: "colour", label: "Colour (hex, optional)", placeholder: "#aa5bbe", max_length: 7, paragraph: false, required: false, value: colour },
        ];
        create_modal(custom_id, "Rules section", fields)
    }

    fn parse(mut data: serenity::ModalInteractionData) -> Result<Self, &'static str> {
        Ok(Self {
            title: find_modal_text(&mut data, "title").ok_or("the title is missing")?,
            body: find_modal_text(&mut data, "body").ok_or("the text is missing")?,
            colour: find_modal_text(&mut data, "colour"),
        })
    }
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

fn guild_id(ctx: Context<'_>) -> u64 {
    ctx.guild_id().expect("rules commands are guild_only").get()
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Manage and post this server's rules
#[command(
    slash_command,
    guild_only,
    category = "Rules",
    subcommands("add", "edit", "remove", "list", "post"),
    subcommand_required,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn rules(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add a rules section
#[command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn add(ctx: Context<'_>) -> Result<(), Error> {
    let Some(form) = execute_modal(ctx, None::<SectionForm>, None).await? else { return Ok(()) };
    let section = match form.into_section() {
        Ok(section) => section,
        Err(message) => return say_private(ctx, message).await,
    };

    let count = {
        let rules = ctx.data().rules.lock().unwrap();
        rules.add(guild_id(ctx), &section)?;
        rules.sections(guild_id(ctx))?.len()
    };
    let reply = CreateReply::default()
        .content(format!("✅ Added section {count}. Run `/rules post` to update the rules channel."))
        .embed(embed(&section))
        .ephemeral(true);
    ctx.send(reply).await?;
    Ok(())
}

/// Edit a rules section
#[command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn edit(
    ctx: Context<'_>,
    #[description = "Section number, as shown in /rules list"]
    #[min = 1]
    number: usize,
) -> Result<(), Error> {
    let current = ctx.data().rules.lock().unwrap().sections(guild_id(ctx))?.get(number.wrapping_sub(1)).cloned();
    let Some(current) = current else {
        return say_private(ctx, format!("❌ There's no section {number}. Use `/rules list` to see them.")).await;
    };

    let form = execute_modal(ctx, Some(SectionForm::from_section(&current)), None).await?;
    let Some(form) = form else { return Ok(()) };
    let section = match form.into_section() {
        Ok(section) => section,
        Err(message) => return say_private(ctx, message).await,
    };

    ctx.data().rules.lock().unwrap().update(guild_id(ctx), number, &section)?;
    let reply = CreateReply::default()
        .content(format!("✅ Updated section {number}. Run `/rules post` to update the rules channel."))
        .embed(embed(&section))
        .ephemeral(true);
    ctx.send(reply).await?;
    Ok(())
}

/// Remove a rules section
#[command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn remove(
    ctx: Context<'_>,
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
#[command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
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
    ctx.send(CreateReply::default().embed(embed).ephemeral(true)).await?;
    Ok(())
}

/// Post the rules, replacing the previous rules messages
#[command(slash_command, guild_only, category = "Rules", required_permissions = "MANAGE_GUILD")]
async fn post(
    ctx: Context<'_>,
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
