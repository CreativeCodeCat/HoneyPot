//! LiveStatus cog: each server follows one Twitch streamer. When they go live, a live card is
//! posted (pinging a role if set) with the game and stream title from Twitch, links to watch,
//! and a spotlight message; when the stream ends, the card is deleted. Twitch is checked
//! every minute, with the app credentials in TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use ::serenity::all as serenity;
use serde_json::{json, Value};

use crate::core::framework::{command, create_modal, execute_modal, find_modal_text, CreateReply, Modal, TextField};
use super::{Context, Data, Error};
use crate::log;

pub use crate::database::repositories::live_status::*;

pub const COG_NAME: &str = "livestatus";

const CHECK_EVERY: Duration = Duration::from_secs(60);
/// Discord's flag for messages laid out with components (containers, text, separators)
const COMPONENTS_V2: u64 = 1 << 15;
const EPHEMERAL: u64 = 1 << 6;
const MAX_SPOTLIGHT: usize = 1500;
const LINE_FILE: &str = "line.png";

/// A stream Twitch says is live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveStream {
    pub id: String,
    pub user_login: String,
    pub user_name: String,
    pub game_name: String,
    pub title: String,
}

/// Twitch's API, with the app access token it hands out.
struct Twitch {
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

/// `None` when the credentials aren't in the environment.
static TWITCH: LazyLock<Option<Twitch>> = LazyLock::new(|| {
    let client_id = std::env::var("TWITCH_CLIENT_ID").ok().filter(|v| !v.is_empty())?;
    let client_secret = std::env::var("TWITCH_CLIENT_SECRET").ok().filter(|v| !v.is_empty())?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().ok()?;
    Some(Twitch { client_id, client_secret, http, token: Mutex::new(None) })
});

const NO_CREDENTIALS: &str = "❌ Twitch isn't set up on the bot: add `TWITCH_CLIENT_ID` and `TWITCH_CLIENT_SECRET` (from an app at https://dev.twitch.tv/console) to its `.env` and restart it.";

impl Twitch {
    /// An app access token, fetching a new one when it's about to run out.
    async fn token(&self) -> Result<String, Error> {
        if let Some((token, expires)) = self.token.lock().unwrap().clone() {
            if Instant::now() + Duration::from_secs(60) < expires {
                return Ok(token);
            }
        }
        let response = self
            .http
            .post("https://id.twitch.tv/oauth2/token")
            .query(&[("client_id", self.client_id.as_str()), ("client_secret", self.client_secret.as_str()), ("grant_type", "client_credentials")])
            .send()
            .await?;
        let status = response.status();
        let body: Value = serde_json::from_str(&response.text().await?)?;
        let (Some(token), Some(expires_in)) = (body["access_token"].as_str(), body["expires_in"].as_u64()) else {
            return Err(format!("Twitch refused the credentials ({status}): {body}").into());
        };
        *self.token.lock().unwrap() = Some((token.to_string(), Instant::now() + Duration::from_secs(expires_in)));
        Ok(token.to_string())
    }

    /// Which of `logins` are live, by login.
    async fn live_streams(&self, logins: &[String]) -> Result<HashMap<String, LiveStream>, Error> {
        let mut live = HashMap::new();
        // Twitch takes up to 100 at a time
        for chunk in logins.chunks(100) {
            let token = self.token().await?;
            let query: Vec<(&str, &str)> = chunk.iter().map(|login| ("user_login", login.as_str())).collect();
            let response = self
                .http
                .get("https://api.twitch.tv/helix/streams")
                .query(&query)
                .header("Client-Id", &self.client_id)
                .bearer_auth(&token)
                .send()
                .await?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                // The token was revoked early; get a new one next time
                *self.token.lock().unwrap() = None;
                return Err("Twitch rejected the access token".into());
            }
            let body: Value = serde_json::from_str(&response.text().await?)?;
            for stream in body["data"].as_array().into_iter().flatten().filter(|s| s["type"] == "live") {
                let text = |key: &str| stream[key].as_str().unwrap_or_default().to_string();
                let found = LiveStream { id: text("id"), user_login: text("user_login").to_lowercase(), user_name: text("user_name"), game_name: text("game_name"), title: text("title") };
                live.insert(found.user_login.clone(), found);
            }
        }
        Ok(live)
    }
}

/// `text` in a code block, like the card's boxed headers.
fn header(text: &str) -> String {
    format!("```\n{text}\n```")
}

/// The live card, as Discord's components. The lines are `LINE_FILE`, sent with it.
pub fn card(streamer: &Streamer, stream: &LiveStream) -> Value {
    let mut intro = vec![];
    if let Some(role) = streamer.ping_role_id {
        intro.push(format!("<@&{role}>"));
    }
    let game = if stream.game_name.is_empty() { "Not set" } else { &stream.game_name };
    let title = if stream.title.is_empty() { "No title" } else { &stream.title };
    intro.push(format!("Game: {game}"));
    intro.push(format!("Subject: {title}"));

    let mut links = vec![format!("📺 [Twitch](https://twitch.tv/{})", streamer.twitch_login)];
    if let Some(url) = &streamer.tiktok_url {
        links.push(format!("🎵 [Tiktok]({url})"));
    }
    if let Some(url) = &streamer.youtube_url {
        links.push(format!("▶️ [Youtube]({url})"));
    }
    if let Some(url) = &streamer.kick_url {
        links.push(format!("🟢 [Kick]({url})"));
    }

    let line = json!({ "type": 12, "items": [{ "media": { "url": format!("attachment://{LINE_FILE}") } }] });
    let text = |content: String| json!({ "type": 10, "content": content });
    let mut inside = vec![
        line.clone(),
        text(header("I AM LIVE")),
        text(format!("**CHOOSE A PLATFORM TO WATCH FROM.**\n{}", links.join("　　"))),
    ];
    if let Some(spotlight) = streamer.spotlight.as_deref().filter(|s| !s.trim().is_empty()) {
        inside.push(text(header("SPOTLIGHT")));
        inside.push(text(spotlight.to_string()));
    }
    inside.push(line);

    json!({
        "flags": COMPONENTS_V2,
        "allowed_mentions": { "parse": [], "roles": streamer.ping_role_id.map(|r| vec![r.to_string()]).unwrap_or_default() },
        "components": [
            text(intro.join("\n\n")),
            { "type": 17, "accent_color": streamer.colour, "components": inside },
        ],
    })
}

/// The thin coloured line drawn at the top and bottom of the card.
fn line_image(colour: u32) -> Option<serenity::CreateAttachment> {
    let mut pixmap = tiny_skia::Pixmap::new(800, 6)?;
    pixmap.fill(tiny_skia::Color::from_rgba8((colour >> 16) as u8, (colour >> 8) as u8, colour as u8, 255));
    Some(serenity::CreateAttachment::bytes(pixmap.encode_png().ok()?, LINE_FILE))
}

/// Posts the live card, returning the message.
async fn post_card(http: &serenity::Http, streamer: &Streamer, stream: &LiveStream) -> Result<serenity::Message, Error> {
    let files = line_image(streamer.colour).into_iter().collect();
    Ok(http.send_message(serenity::ChannelId::new(streamer.channel_id), files, &card(streamer, stream)).await?)
}

/// Deletes the posted live card, if there is one.
async fn delete_card(http: &serenity::Http, data: &Data, streamer: &Streamer) {
    if let (Some(channel), Some(message)) = (streamer.message_channel_id, streamer.message_id) {
        // Already deleted by someone is fine
        let _ = serenity::ChannelId::new(channel).delete_message(http, message).await;
    }
    if let Err(e) = data.live_status.lock().unwrap().set_live_message(streamer.guild_id, None) {
        log::error(format!("Error clearing a live message: {e}"));
    }
}

/// Posts a live card when a followed streamer goes live, and deletes it when they stop.
/// Runs whether or not the cog is enabled, so turning it off still cleans up.
pub async fn live_loop(ctx: serenity::Context, data: Data) {
    let Some(twitch) = TWITCH.as_ref() else {
        return log::info("Twitch live notifications are off: TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET aren't set");
    };
    let mut interval = tokio::time::interval(CHECK_EVERY);
    loop {
        interval.tick().await;
        let streamers = match data.live_status.lock().unwrap().all() {
            Ok(streamers) => streamers,
            Err(e) => {
                log::error(format!("Error reading streamers: {e}"));
                continue;
            }
        };
        let watched: Vec<&Streamer> = streamers.iter().filter(|s| data.is_cog_enabled(s.guild_id, COG_NAME)).collect();
        let mut logins: Vec<String> = watched.iter().map(|s| s.twitch_login.clone()).collect();
        logins.sort();
        logins.dedup();
        let live = if logins.is_empty() {
            HashMap::new()
        } else {
            match twitch.live_streams(&logins).await {
                Ok(live) => live,
                Err(e) => {
                    log::warning(format!("Couldn't check Twitch: {e}"));
                    continue;
                }
            }
        };

        for streamer in &streamers {
            let enabled = data.is_cog_enabled(streamer.guild_id, COG_NAME);
            match live.get(&streamer.twitch_login).filter(|_| enabled) {
                // A new stream (or a restart of it): replace any old card
                Some(stream) if streamer.stream_id.as_deref() != Some(&stream.id) => {
                    delete_card(&ctx.http, &data, streamer).await;
                    match post_card(&ctx.http, streamer, stream).await {
                        Ok(message) => {
                            log::info(format!("{} went live; posted in guild {}", stream.user_name, streamer.guild_id));
                            let saved = data.live_status.lock().unwrap().set_live_message(streamer.guild_id, Some((&stream.id, message.channel_id.get(), message.id.get())));
                            if let Err(e) = saved {
                                log::error(format!("Error saving a live message: {e}"));
                            }
                        }
                        Err(e) => log::warning(format!("Couldn't post the live card in guild {}: {e}", streamer.guild_id)),
                    }
                }
                Some(_) => {}
                None if streamer.message_id.is_some() => {
                    log::info(format!("{} stopped streaming; deleted the card in guild {}", streamer.twitch_login, streamer.guild_id));
                    delete_card(&ctx.http, &data, streamer).await;
                }
                None => {}
            }
        }
    }
}

fn valid_login(login: &str) -> bool {
    (4..=25).contains(&login.len()) && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A Twitch login from a username or a twitch.tv link.
fn parse_login(input: &str) -> Option<String> {
    let input = input.trim().trim_end_matches('/');
    let login = input.rsplit('/').next().unwrap_or(input).trim_start_matches('@').to_lowercase();
    valid_login(&login).then_some(login)
}

fn valid_url(url: &str) -> bool {
    url.starts_with("https://") && !url.contains(char::is_whitespace)
}

/// Twitch live notifications for this server's streamer
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "LiveStatus",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("setup", "spotlight", "preview", "status", "remove"),
    subcommand_required
)]
pub async fn live(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Choose the streamer, where their live card goes and who it pings
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
#[allow(clippy::too_many_arguments)]
async fn setup(
    ctx: Context<'_>,
    #[description = "Their Twitch username or channel link"]
    #[max_length = 100]
    twitch: String,
    #[description = "Where to post when they go live"]
    #[channel_types("Text", "News")]
    channel: serenity::GuildChannel,
    #[description = "Role to ping when they go live"] ping_role: Option<serenity::Role>,
    #[description = "Their TikTok link, shown on the card"]
    #[max_length = 200]
    tiktok: Option<String>,
    #[description = "Their YouTube link, shown on the card"]
    #[max_length = 200]
    youtube: Option<String>,
    #[description = "Their Kick link, shown on the card"]
    #[max_length = 200]
    kick: Option<String>,
    #[description = "Card colour as hex, e.g. #00d26a (the default green)"]
    #[max_length = 7]
    colour: Option<String>,
) -> Result<(), Error> {
    let Some(login) = parse_login(&twitch) else {
        ctx.say(format!("❌ `{twitch}` isn't a Twitch username (4-25 letters, numbers or underscores).")).await?;
        return Ok(());
    };
    for url in [&tiktok, &youtube, &kick].into_iter().flatten() {
        if !valid_url(url) {
            ctx.say(format!("❌ `{url}` isn't a link; it should start with `https://`.")).await?;
            return Ok(());
        }
    }
    let colour = match colour.as_deref().map(|c| u32::from_str_radix(c.trim().trim_start_matches('#'), 16)) {
        None => None,
        Some(Ok(c)) if c <= 0xffffff => Some(c),
        Some(_) => {
            ctx.say("❌ That isn't a colour; use hex like `#00d26a`.").await?;
            return Ok(());
        }
    };
    let guild_id = ctx.guild_id().unwrap().get();
    let streamer = {
        let store = ctx.data().live_status.lock().unwrap();
        let mut streamer = store.get(guild_id)?.unwrap_or_else(|| Streamer::new(guild_id, login.clone(), channel.id.get()));
        // A different streamer's card no longer applies
        if streamer.twitch_login != login {
            streamer.stream_id = None;
        }
        streamer.twitch_login = login;
        streamer.channel_id = channel.id.get();
        streamer.ping_role_id = ping_role.map(|r| r.id.get()).or(streamer.ping_role_id);
        streamer.tiktok_url = tiktok.or(streamer.tiktok_url);
        streamer.youtube_url = youtube.or(streamer.youtube_url);
        streamer.kick_url = kick.or(streamer.kick_url);
        streamer.colour = colour.unwrap_or(streamer.colour);
        store.set(&streamer)?;
        streamer
    };
    let warning = if TWITCH.is_none() { format!("\n\n{NO_CREDENTIALS}") } else { String::new() };
    let text = format!(
        "✅ Following **{}** on Twitch. When they go live, a card is posted in <#{}>{}. It's deleted when the stream ends.\n\
         Set the spotlight message with `/live spotlight`, and see the card with `/live preview`.{warning}",
        streamer.twitch_login,
        streamer.channel_id,
        streamer.ping_role_id.map_or_else(String::new, |r| format!(", pinging <@&{r}>")),
    );
    ctx.say(text).await?;
    Ok(())
}

/// The form for the spotlight message.
struct SpotlightForm {
    text: Option<String>,
}

impl Modal for SpotlightForm {
    fn create(defaults: Option<Self>, custom_id: String) -> serenity::CreateInteractionResponse {
        let field = TextField {
            id: "text",
            label: "Spotlight (leave empty to remove it)",
            placeholder: "Our new merch store is now open!",
            max_length: MAX_SPOTLIGHT as u16,
            paragraph: true,
            required: false,
            value: defaults.and_then(|d| d.text),
        };
        create_modal(custom_id, "Live card spotlight", vec![field])
    }

    fn parse(mut data: serenity::ModalInteractionData) -> Result<Self, &'static str> {
        Ok(Self { text: find_modal_text(&mut data, "text") })
    }
}

/// Write the spotlight message shown on the live card
#[command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn spotlight(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let Some(current) = ctx.data().live_status.lock().unwrap().get(guild_id)? else {
        ctx.send(CreateReply::default().content("❌ Choose a streamer with `/live setup` first.").ephemeral(true)).await?;
        return Ok(());
    };
    let Some(form) = execute_modal(ctx, Some(SpotlightForm { text: current.spotlight.clone() }), None).await? else { return Ok(()) };
    let text = form.text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let saved = {
        let store = ctx.data().live_status.lock().unwrap();
        match store.get(guild_id)? {
            Some(mut streamer) => {
                streamer.spotlight = text.clone();
                store.set(&streamer)?;
                true
            }
            None => false,
        }
    };
    let reply = match (saved, text) {
        (false, _) => "❌ The streamer was removed while you were writing.",
        (true, Some(_)) => "✅ Spotlight saved. It shows on the next live card; see it with `/live preview`.",
        (true, None) => "✅ Spotlight removed.",
    };
    ctx.send(CreateReply::default().content(reply).ephemeral(true)).await?;
    Ok(())
}

/// See the live card as it'll look (only you can see it)
#[command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
async fn preview(ctx: Context<'_>) -> Result<(), Error> {
    let Some(streamer) = ctx.data().live_status.lock().unwrap().get(ctx.guild_id().unwrap().get())? else {
        ctx.send(CreateReply::default().content("❌ Choose a streamer with `/live setup` first.").ephemeral(true)).await?;
        return Ok(());
    };
    // Their real stream if they're live, else an example
    let live = match TWITCH.as_ref() {
        Some(twitch) => twitch.live_streams(std::slice::from_ref(&streamer.twitch_login)).await.ok().and_then(|mut l| l.remove(&streamer.twitch_login)),
        None => None,
    };
    let stream = live.unwrap_or_else(|| LiveStream { game_name: "Escape from Tarkov".into(), title: "Example stream title".into(), ..Default::default() });
    let mut message = card(&streamer, &stream);
    message["flags"] = json!(COMPONENTS_V2 | EPHEMERAL);
    // Nobody is pinged by a preview
    message["allowed_mentions"] = json!({ "parse": [] });
    let response = json!({ "type": 4, "data": message });
    let files = line_image(streamer.colour).into_iter().collect();
    ctx.http().create_interaction_response(ctx.interaction.id, &ctx.interaction.token, &response, files).await?;
    ctx.mark_responded();
    Ok(())
}

/// See who this server follows and whether they're live
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    let Some(s) = ctx.data().live_status.lock().unwrap().get(ctx.guild_id().unwrap().get())? else {
        ctx.say("This server doesn't follow a streamer. Choose one with `/live setup`.").await?;
        return Ok(());
    };
    let live = match (s.message_channel_id, s.message_id) {
        (Some(channel), Some(message)) => format!("🔴 Live now: https://discord.com/channels/{}/{channel}/{message}", s.guild_id),
        _ => "⚫ Not live".to_string(),
    };
    let links: Vec<&str> = [s.tiktok_url.as_deref(), s.youtube_url.as_deref(), s.kick_url.as_deref()].into_iter().flatten().collect();
    let embed = serenity::CreateEmbed::new()
        .colour(s.colour)
        .title("Live notifications")
        .field("Streamer", format!("[{0}](https://twitch.tv/{0})", s.twitch_login), true)
        .field("Channel", format!("<#{}>", s.channel_id), true)
        .field("Pings", s.ping_role_id.map_or_else(|| "Nobody".to_string(), |r| format!("<@&{r}>")), true)
        .field("Other links", if links.is_empty() { "None".to_string() } else { links.join("\n") }, false)
        .field("Spotlight", if s.spotlight.is_some() { "Set (see `/live preview`)" } else { "None" }, true)
        .field("Status", live, false);
    let mut reply = CreateReply::default().embed(embed);
    if TWITCH.is_none() {
        reply = reply.content(NO_CREDENTIALS);
    }
    ctx.send(reply).await?;
    Ok(())
}

/// Stop following the streamer
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn remove(ctx: Context<'_>) -> Result<(), Error> {
    let removed = ctx.data().live_status.lock().unwrap().remove(ctx.guild_id().unwrap().get())?;
    let Some(streamer) = removed else {
        ctx.say("This server doesn't follow a streamer.").await?;
        return Ok(());
    };
    if let (Some(channel), Some(message)) = (streamer.message_channel_id, streamer.message_id) {
        let _ = serenity::ChannelId::new(channel).delete_message(ctx, message).await;
    }
    ctx.say(format!("✅ No longer following **{}**.", streamer.twitch_login)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_logins_from_names_and_links() {
        assert_eq!(parse_login("SomeStreamer").as_deref(), Some("somestreamer"));
        assert_eq!(parse_login("https://www.twitch.tv/some_streamer/").as_deref(), Some("some_streamer"));
        assert_eq!(parse_login("@cool_guy99").as_deref(), Some("cool_guy99"));
        assert_eq!(parse_login("abc"), None);
        assert_eq!(parse_login("not a name"), None);
    }

    #[test]
    fn card_matches_the_layout() {
        let mut streamer = Streamer::new(1, "streamer".into(), 2);
        streamer.ping_role_id = Some(42);
        streamer.youtube_url = Some("https://youtube.com/@streamer".into());
        streamer.kick_url = Some("https://kick.com/streamer".into());
        streamer.spotlight = Some("Merch is out!".into());
        let stream = LiveStream { id: "9".into(), game_name: "EFT".into(), title: "PVE Halloween Event".into(), ..Default::default() };
        let card = card(&streamer, &stream);
        assert_eq!(card["flags"], COMPONENTS_V2);
        assert_eq!(card["allowed_mentions"]["roles"][0], "42");
        let intro = card["components"][0]["content"].as_str().unwrap();
        assert_eq!(intro, "<@&42>\n\nGame: EFT\n\nSubject: PVE Halloween Event");
        let container = &card["components"][1];
        assert_eq!(container["type"], 17);
        assert_eq!(container["accent_color"], DEFAULT_COLOUR);
        let parts = container["components"].as_array().unwrap();
        // Line, I AM LIVE, platforms, SPOTLIGHT, spotlight text, line
        assert_eq!(parts.len(), 6);
        assert!(parts[1]["content"].as_str().unwrap().contains("I AM LIVE"));
        let platforms = parts[2]["content"].as_str().unwrap();
        assert!(platforms.contains("https://twitch.tv/streamer") && platforms.contains("Youtube") && !platforms.contains("Tiktok"));
        assert!(platforms.contains("[Kick](https://kick.com/streamer)"));
        assert_eq!(parts[4]["content"], "Merch is out!");
    }

    #[test]
    fn card_without_spotlight_or_role() {
        let streamer = Streamer::new(1, "streamer".into(), 2);
        let card = card(&streamer, &LiveStream::default());
        assert!(card["components"][0]["content"].as_str().unwrap().starts_with("Game: Not set"));
        assert_eq!(card["components"][1]["components"].as_array().unwrap().len(), 4);
        assert_eq!(card["allowed_mentions"]["roles"].as_array().unwrap().len(), 0);
    }
}
