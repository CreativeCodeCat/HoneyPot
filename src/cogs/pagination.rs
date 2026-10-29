//! Shared paging for any cog: `paginate` replies with the first of several embeds and
//! Previous / Next / Delete buttons to move between them.

use std::time::Duration;

use poise::serenity_prelude as serenity;

use super::{Context, Error};

/// How long the buttons keep working after the last press
const TIMEOUT: Duration = Duration::from_secs(120);

/// Splits `lines` into pages of at most `per_page` lines, joined with newlines.
pub fn chunk_lines(lines: &[String], per_page: usize) -> Vec<String> {
    lines.chunks(per_page.max(1)).map(|chunk| chunk.join("\n")).collect()
}

/// Builds pages from `make_page()` with up to `per_page` of `fields` (name, value) on each,
/// all full width. With `spacers`, a blank field separates the entries on a page.
pub fn field_pages(
    make_page: impl Fn() -> serenity::CreateEmbed,
    fields: Vec<(String, String)>,
    per_page: usize,
    spacers: bool,
) -> Vec<serenity::CreateEmbed> {
    fields
        .chunks(per_page.max(1))
        .map(|chunk| {
            let mut embed = make_page();
            for (i, (name, value)) in chunk.iter().enumerate() {
                if spacers && i > 0 {
                    embed = embed.field("\u{200b}", "\u{200b}", false);
                }
                embed = embed.field(name, value, false);
            }
            embed
        })
        .collect()
}

/// Replies with `pages`, one at a time. Previous and Next wrap around, and Delete removes
/// the reply; only the member who ran the command can use them. Once the buttons go
/// unused for 2 minutes they're disabled. A single page is sent without buttons.
///
/// Each page's footer is replaced with "Page x / n".
pub async fn paginate(ctx: Context<'_>, pages: Vec<serenity::CreateEmbed>) -> Result<(), Error> {
    if pages.len() <= 1 {
        let mut reply = poise::CreateReply::default();
        if let Some(page) = pages.into_iter().next() {
            reply = reply.embed(page);
        }
        ctx.send(reply).await?;
        return Ok(());
    }

    let prefix = ctx.id();
    let (prev_id, next_id, delete_id) = (format!("{prefix}page_prev"), format!("{prefix}page_next"), format!("{prefix}page_delete"));
    let page_embed = |page: usize| {
        pages[page].clone().footer(serenity::CreateEmbedFooter::new(format!("Page {} / {}", page + 1, pages.len())))
    };
    let buttons = |enabled: bool| {
        let mut row = vec![
            serenity::CreateButton::new(&prev_id).emoji('◀').label("Previous").style(serenity::ButtonStyle::Secondary).disabled(!enabled),
            serenity::CreateButton::new(&next_id).emoji('▶').label("Next").style(serenity::ButtonStyle::Secondary).disabled(!enabled),
        ];
        if enabled {
            row.push(serenity::CreateButton::new(&delete_id).emoji('🗑').label("Delete").style(serenity::ButtonStyle::Danger));
        }
        vec![serenity::CreateActionRow::Buttons(row)]
    };

    let mut page = 0;
    let reply = ctx.send(poise::CreateReply::default().embed(page_embed(page)).components(buttons(true))).await?;

    loop {
        let ids = [prev_id.clone(), next_id.clone(), delete_id.clone()];
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .filter(move |i| ids.contains(&i.data.custom_id))
            .timeout(TIMEOUT)
            .await;
        let Some(press) = press else { break };

        if press.user.id != ctx.author().id {
            let message = serenity::CreateInteractionResponseMessage::new().content("❌ These buttons aren't yours.").ephemeral(true);
            press.create_response(ctx, serenity::CreateInteractionResponse::Message(message)).await?;
            continue;
        }

        if press.data.custom_id == delete_id {
            press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
            reply.delete(ctx).await?;
            return Ok(());
        }
        page = if press.data.custom_id == prev_id { (page + pages.len() - 1) % pages.len() } else { (page + 1) % pages.len() };
        let update = serenity::CreateInteractionResponseMessage::new().embed(page_embed(page)).components(buttons(true));
        press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
    }

    // Already gone if someone deleted it
    let _ = reply.edit(ctx, poise::CreateReply::default().embed(page_embed(page)).components(buttons(false))).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_split_into_pages() {
        let lines: Vec<String> = (1..=5).map(|n| n.to_string()).collect();
        assert_eq!(chunk_lines(&lines, 2), ["1\n2", "3\n4", "5"]);
        assert_eq!(chunk_lines(&lines, 0).len(), 5);
        assert!(chunk_lines(&[], 10).is_empty());
    }
}
