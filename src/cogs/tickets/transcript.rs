//! Renders a channel's full message history as a standalone HTML page.

use poise::serenity_prelude as serenity;

use crate::cogs::Error;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Fetches every message in the channel, oldest first.
async fn fetch_all(http: &serenity::Http, channel_id: serenity::ChannelId) -> Result<Vec<serenity::Message>, Error> {
    let mut messages = vec![];
    let mut before = None;
    loop {
        let mut request = serenity::GetMessages::new().limit(100);
        if let Some(id) = before {
            request = request.before(id);
        }
        let batch = channel_id.messages(http, request).await?;
        let done = batch.len() < 100;
        before = batch.last().map(|m| m.id);
        messages.extend(batch);
        if done {
            break;
        }
    }
    messages.reverse();
    Ok(messages)
}

fn render_message(message: &serenity::Message) -> String {
    let author = &message.author;
    let name = escape(author.global_name.as_deref().unwrap_or(&author.name));
    let time = message.timestamp.format("%Y-%m-%d %H:%M:%S UTC");

    let mut body = String::new();
    if !message.content.is_empty() {
        body += &format!("<div class=\"content\">{}</div>", escape(&message.content).replace('\n', "<br>"));
    }
    for embed in &message.embeds {
        body += "<div class=\"embed\">";
        if let Some(title) = &embed.title {
            body += &format!("<div class=\"embed-title\">{}</div>", escape(title));
        }
        if let Some(description) = &embed.description {
            body += &format!("<div>{}</div>", escape(description).replace('\n', "<br>"));
        }
        for field in &embed.fields {
            body += &format!(
                "<div class=\"field\"><b>{}</b><br>{}</div>",
                escape(&field.name),
                escape(&field.value).replace('\n', "<br>")
            );
        }
        body += "</div>";
    }
    for attachment in &message.attachments {
        let url = escape(&attachment.url);
        if attachment.content_type.as_deref().is_some_and(|t| t.starts_with("image/")) {
            body += &format!("<a href=\"{url}\"><img class=\"image\" src=\"{url}\"></a>");
        } else {
            body += &format!("<div><a href=\"{url}\">📎 {}</a></div>", escape(&attachment.filename));
        }
    }

    format!(
        "<div class=\"message\"><img class=\"avatar\" src=\"{}\"><div><div><span class=\"author\">{name}</span>{}<span class=\"time\">{time}</span></div>{body}</div></div>",
        escape(&author.face()),
        if author.bot { "<span class=\"bot\">BOT</span>" } else { "" },
    )
}

/// Builds the transcript for a channel as an HTML attachment named `filename`.
pub async fn build(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    title: &str,
    filename: String,
) -> Result<serenity::CreateAttachment, Error> {
    let messages = fetch_all(http, channel_id).await?;
    let rendered: String = messages.iter().map(render_message).collect();

    let html = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
body {{ background: #313338; color: #dbdee1; font-family: "gg sans", "Helvetica Neue", Arial, sans-serif; margin: 0; padding: 16px; }}
h1 {{ font-size: 18px; border-bottom: 1px solid #3f4147; padding-bottom: 12px; }}
.message {{ display: flex; gap: 12px; padding: 8px 0; }}
.avatar {{ width: 40px; height: 40px; border-radius: 50%; flex-shrink: 0; }}
.author {{ color: #f2f3f5; font-weight: 600; }}
.bot {{ background: #5865f2; color: #fff; font-size: 10px; padding: 1px 4px; border-radius: 3px; margin-left: 4px; }}
.time {{ color: #949ba4; font-size: 12px; margin-left: 8px; }}
.content {{ margin-top: 2px; white-space: normal; overflow-wrap: anywhere; }}
.embed {{ border-left: 4px solid #5865f2; background: #2b2d31; padding: 8px 12px; margin-top: 4px; border-radius: 4px; max-width: 520px; }}
.embed-title {{ font-weight: 600; margin-bottom: 4px; }}
.field {{ margin-top: 6px; }}
.image {{ max-width: 400px; max-height: 300px; margin-top: 4px; border-radius: 4px; }}
a {{ color: #00a8fc; }}
</style></head>
<body><h1>{title} · {count} messages</h1>
{rendered}
</body></html>"#,
        title = escape(title),
        count = messages.len(),
    );

    Ok(serenity::CreateAttachment::bytes(html.into_bytes(), filename))
}
