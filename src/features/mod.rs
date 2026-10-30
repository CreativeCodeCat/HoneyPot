//! Bot modules ("cogs"). Every command declares its cog through the `category` option of
//! `#[command]`, and a framework-wide check uses it to enforce the per-guild
//! enabled/disabled flag stored by the cog manager.

pub mod afk;
pub mod announce;
pub mod applications;
pub mod cog_manager;
pub mod cooldowns;
pub mod developer;
pub mod economy;
pub mod games;
pub mod general;
pub mod honeypot;
pub mod incidents;
pub mod info;
pub mod leveling;
pub mod live_status;
pub mod moderation;
pub mod logging;
pub mod pets;
pub mod presence;
pub mod private_mode;
pub mod profile;
pub mod reaction_roles;
pub mod rules;
pub mod server_setup;
pub mod stats;
pub mod sticky;
pub mod suggestions;
pub mod tickets;
pub mod welcome;

// Moderation tools live under `moderation/`, but are their own cogs
pub use moderation::{automod, lockdown, quarantine, raid_protection, staff_policy, suspicious_accounts};

pub use crate::core::error::{Context, Error};
use crate::core::framework::Command;
pub use crate::core::state::{Data, State};
pub use crate::shared::utils::{format_time, now_iso, parse_iso, title_case, truncate};

/// Every cog compiled into the bot, by display name.
pub const COGS: &[&str] = &[
    "AFK",
    "Announcements",
    "Applications",
    "AutoMod",
    "CogManager",
    "Economy",
    "Games",
    "General",
    "HoneyPot",
    "Incidents",
    "Info",
    "Levels",
    "LiveStatus",
    "Lockdown",
    "Moderation",
    "Logs",
    "Pets",
    "Profile",
    "Quarantine",
    "RaidProtection",
    "ReactionRoles",
    "Rules",
    "ServerStats",
    "StaffPolicy",
    "Sticky",
    "Suggestions",
    "SuspiciousAccounts",
    "Tickets",
];

/// The cog that manages the others; it can never be disabled.
pub const COG_MANAGER: &str = "cogmanager";

/// The top-level command being run: for a subcommand like `/sticky stick`, `/sticky`.
pub fn root_command<'a>(ctx: Context<'a>) -> &'a Command {
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

