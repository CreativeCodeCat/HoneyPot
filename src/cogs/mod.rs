//! Bot modules ("cogs"). Every command declares its cog through poise's `category`
//! attribute, and a framework-wide check uses it to enforce the per-guild
//! enabled/disabled flag stored by the cog manager.

pub mod announce;
pub mod automod;
pub mod cog_manager;
pub mod cooldowns;
pub mod economy;
pub mod games;
pub mod general;
pub mod honeypot;
pub mod info;
pub mod levels;
pub mod lockdown;
pub mod moderation;
pub mod logs;
pub mod pets;
pub mod profile;
pub mod rules;
pub mod server_setup;
pub mod staff_policy;
pub mod stats;
pub mod sticky;
pub mod suggestions;
pub mod tickets;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, NaiveDateTime, Utc};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;
pub type Data = Arc<State>;

/// Every cog compiled into the bot, by display name.
pub const COGS: &[&str] = &[
    "Announcements",
    "AutoMod",
    "CogManager",
    "Economy",
    "Games",
    "General",
    "HoneyPot",
    "Info",
    "Levels",
    "Lockdown",
    "Moderation",
    "Logs",
    "Pets",
    "Profile",
    "Rules",
    "ServerStats",
    "StaffPolicy",
    "Sticky",
    "Suggestions",
    "Tickets",
];

/// The cog that manages the others; it can never be disabled.
pub const COG_MANAGER: &str = "cogmanager";

pub struct State {
    pub start_time: Instant,
    pub cog_config: Mutex<cog_manager::CogConfig>,
    pub blacklist: Mutex<crate::developer::Blacklist>,
    pub cooldowns: Mutex<cooldowns::Cooldowns>,
    pub counting: Mutex<games::counting::Store>,
    pub economy: Mutex<economy::store::Store>,
    pub guessing: Mutex<games::guessing::Store>,
    pub honeypot: Mutex<honeypot::store::Store>,
    pub levels: Mutex<levels::store::Store>,
    pub lockdown: Mutex<lockdown::Store>,
    pub pets: Mutex<pets::store::Store>,
    pub profiles: Mutex<profile::Store>,
    pub rules: Mutex<rules::Store>,
    pub server_setup: Mutex<server_setup::Store>,
    pub staff_policy: Mutex<staff_policy::Store>,
    pub sticky: Mutex<sticky::Store>,
    pub suggestions: Mutex<suggestions::Store>,
    pub tickets: Mutex<tickets::store::Store>,
}

impl State {
    pub fn is_cog_enabled(&self, guild_id: u64, cog_name: &str) -> bool {
        self.cog_config
            .lock()
            .unwrap()
            .is_enabled(guild_id, cog_name)
    }
}

/// The top-level command being run: for a subcommand like `/sticky stick`, `/sticky`.
pub fn root_command<'a>(ctx: Context<'a>) -> &'a poise::Command<Data, Error> {
    ctx.parent_commands().first().copied().unwrap_or(ctx.command())
}

/// The cog of the command being run. Subcommands take their top-level command's, as only
/// it says which cog it belongs to.
pub fn command_category(ctx: Context<'_>) -> Option<&str> {
    root_command(ctx).category.as_deref()
}

/// The name a cog is stored under in the config table, e.g. "ServerStats" -> "serverstats".
pub fn normalize_cog_name(name: &str) -> String {
    name.to_lowercase().replace(' ', "")
}

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}

/// Parses a timestamp written by `now_iso` or by Python's `datetime.isoformat()`.
/// Timestamps without an offset are treated as UTC.
pub fn parse_iso(timestamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f").map(|dt| dt.and_utc())
        })
        .ok()
}

pub fn format_time(timestamp: &str) -> String {
    parse_iso(timestamp)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

/// Equivalent of Python's `str.title()`.
pub fn title_case(text: &str) -> String {
    let mut prev_alpha = false;
    text.chars()
        .map(|c| {
            let out = if prev_alpha {
                c.to_lowercase().to_string()
            } else {
                c.to_uppercase().to_string()
            };
            prev_alpha = c.is_alphabetic();
            out
        })
        .collect()
}

/// Truncates to at most `max` characters (Discord limits are in characters).
pub fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_python() {
        assert_eq!(
            title_case("user_banned".replace('_', " ").as_str()),
            "User Banned"
        );
        assert_eq!(title_case("flagged code-blocks"), "Flagged Code-Blocks");
        assert_eq!(
            format_time("2026-09-30T13:26:35.965988+00:00"),
            "2026-09-30 13:26:35"
        );
        assert_eq!(
            format_time("2026-09-30T13:26:35.965988"),
            "2026-09-30 13:26:35"
        );
        assert!(parse_iso(&now_iso()).is_some());
        assert_eq!(normalize_cog_name("Server Stats"), "serverstats");
    }
}
