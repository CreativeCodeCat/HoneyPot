//! The bot's presence: its status (Online, Idle...) and activity ("Playing ...", or a custom
//! status), set by the owner with `/presence` and kept across restarts. Maintenance mode
//! overrides it until it's turned off.

use poise::serenity_prelude as serenity;
use serde_json::json;

use crate::cogs::{Context, Error};
use crate::{developer, log};

/// Where the presence is saved
const PRESENCE_FILE: &str = "database/presence.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Status {
    Online,
    Idle,
    #[name = "Do Not Disturb"]
    DoNotDisturb,
    Invisible,
}

impl Status {
    fn key(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::DoNotDisturb => "dnd",
            Self::Invisible => "invisible",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "idle" => Self::Idle,
            "dnd" => Self::DoNotDisturb,
            "invisible" => Self::Invisible,
            _ => Self::Online,
        }
    }

    fn online_status(self) -> serenity::OnlineStatus {
        match self {
            Self::Online => serenity::OnlineStatus::Online,
            Self::Idle => serenity::OnlineStatus::Idle,
            Self::DoNotDisturb => serenity::OnlineStatus::DoNotDisturb,
            Self::Invisible => serenity::OnlineStatus::Invisible,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum Activity {
    #[name = "Custom status"]
    Custom,
    Playing,
    Watching,
    #[name = "Listening to"]
    Listening,
    #[name = "Competing in"]
    Competing,
    #[name = "No activity"]
    Nothing,
}

impl Activity {
    fn key(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Playing => "playing",
            Self::Watching => "watching",
            Self::Listening => "listening",
            Self::Competing => "competing",
            Self::Nothing => "none",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "playing" => Self::Playing,
            "watching" => Self::Watching,
            "listening" => Self::Listening,
            "competing" => Self::Competing,
            "none" => Self::Nothing,
            _ => Self::Custom,
        }
    }

    fn data(self, text: &str) -> Option<serenity::ActivityData> {
        Some(match self {
            Self::Custom => serenity::ActivityData::custom(text),
            Self::Playing => serenity::ActivityData::playing(text),
            Self::Watching => serenity::ActivityData::watching(text),
            Self::Listening => serenity::ActivityData::listening(text),
            Self::Competing => serenity::ActivityData::competing(text),
            Self::Nothing => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub status: Status,
    pub activity: Activity,
    /// The activity's text; the default text if empty
    pub text: String,
}

impl Default for Presence {
    fn default() -> Self {
        Self { status: Status::Online, activity: Activity::Custom, text: String::new() }
    }
}

impl Presence {
    fn load() -> Self {
        let Ok(saved) = std::fs::read_to_string(PRESENCE_FILE) else { return Self::default() };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&saved) else { return Self::default() };
        Self {
            status: Status::from_key(value["status"].as_str().unwrap_or_default()),
            activity: Activity::from_key(value["activity"].as_str().unwrap_or_default()),
            text: value["text"].as_str().unwrap_or_default().to_string(),
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let value = json!({ "status": self.status.key(), "activity": self.activity.key(), "text": self.text });
        std::fs::write(PRESENCE_FILE, value.to_string())
    }

    /// The activity's text, or "Development of <bot> v<version>" if none was given.
    fn text(&self, bot_name: &str) -> String {
        if self.text.is_empty() {
            format!("Development of {bot_name} v{}", env!("CARGO_PKG_VERSION"))
        } else {
            self.text.clone()
        }
    }
}

/// Shows the saved presence, or maintenance mode's (Do Not Disturb, "Under maintenance") while it's on.
pub fn apply(ctx: &serenity::Context) {
    if developer::maintenance() {
        ctx.set_presence(Some(serenity::ActivityData::custom("🛠️ Under maintenance")), serenity::OnlineStatus::DoNotDisturb);
        return;
    }
    let presence = Presence::load();
    let bot_name = ctx.cache.current_user().name.clone();
    ctx.set_presence(presence.activity.data(&presence.text(&bot_name)), presence.status.online_status());
}

/// Set the bot's status and activity (bot owner only)
#[poise::command(
    slash_command,
    owners_only,
    ephemeral,
    category = "Developer",
    // Hidden from everyone but admins; owners_only stops anyone else using it
    default_member_permissions = "ADMINISTRATOR"
)]
pub async fn presence(
    ctx: Context<'_>,
    #[description = "The bot's status"] status: Status,
    #[description = "What kind of activity to show (default: custom status)"] activity: Option<Activity>,
    #[description = "The activity's text (default: Development of the bot and its version)"]
    #[max_length = 128]
    text: Option<String>,
) -> Result<(), Error> {
    let presence = Presence {
        status,
        activity: activity.unwrap_or(Activity::Custom),
        text: text.map(|t| t.trim().to_string()).unwrap_or_default(),
    };
    if let Err(e) = presence.save() {
        log::error(format!("Error saving the presence: {e}"));
    }
    apply(ctx.serenity_context());

    let bot_name = ctx.cache().current_user().name.clone();
    let shown = match presence.activity {
        Activity::Nothing => "No activity".to_string(),
        Activity::Custom => presence.text(&bot_name),
        kind => format!("{} {}", poise::ChoiceParameter::name(&kind), presence.text(&bot_name)),
    };
    let mut text = format!("**Status:** {}\n**Activity:** {shown}", poise::ChoiceParameter::name(&status));
    if developer::maintenance() {
        text += "\n\nMaintenance mode is on, so this will show once it's turned off with `/maintenance`.";
    }
    let embed = serenity::CreateEmbed::new().colour(0x2ecc71).title("✅ Presence Updated").description(text);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for status in [Status::Online, Status::Idle, Status::DoNotDisturb, Status::Invisible] {
            assert_eq!(Status::from_key(status.key()), status);
        }
        for activity in [Activity::Custom, Activity::Playing, Activity::Watching, Activity::Listening, Activity::Competing, Activity::Nothing] {
            assert_eq!(Activity::from_key(activity.key()), activity);
        }
        assert_eq!(Presence::default().text("Bot"), format!("Development of Bot v{}", env!("CARGO_PKG_VERSION")));
    }
}
