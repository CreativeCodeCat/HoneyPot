//! Per-guild command cooldowns. Admins set how many seconds must pass between uses of a
//! command, either for each member or for the whole server at once; commands without a
//! setting fall back to `DEFAULTS` (per member), or no cooldown at all.


use ::serenity::all as serenity;

use crate::core::framework::{command, Command, CreateReply};
use super::{Context, Error};

pub use crate::database::repositories::cooldowns::*;

/// Longest cooldown that can be set: one week
const MAX_COOLDOWN_SECS: u64 = 7 * 24 * 60 * 60;

/// Things with a cooldown that aren't commands, so `/cooldown set` accepts them too
const NON_COMMANDS: &[&str] = &["guessing"];

/// Commands that start their own cooldown (after checking their input), so the
/// framework-wide check leaves them alone. `heist` cools down for the whole server
/// once a heist ends, and `fight` for both fighters once a fight ends.
const SELF_MANAGED: &[&str] = &["hack", "heist", "fight"];

/// Framework-wide check: starts the member's cooldown, or fails with a message if it's still running.
pub fn check(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let command = &ctx.command().qualified_name;
    if SELF_MANAGED.contains(&command.as_str()) {
        return Ok(());
    }
    let (started, scope) = {
        let cooldowns = ctx.data().cooldowns.lock().unwrap();
        let started = cooldowns.start(guild_id.get(), ctx.author().id.get(), command, chrono::Utc::now().timestamp())?;
        (started, cooldowns.scope(guild_id.get(), command))
    };
    let whom = if scope == Scope::Global { " for everyone" } else { "" };
    started.map_err(|ready| format!("⏱️ This command is on cooldown{whom}. Try again <t:{ready}:R>.").into())
}

/// Qualified names of every runnable command, subcommands included.
fn command_names(commands: &[Command], out: &mut Vec<String>) {
    for command in commands {
        if command.subcommands.is_empty() {
            out.push(command.qualified_name.clone());
        } else {
            command_names(&command.subcommands, out);
        }
    }
}

fn all_command_names(ctx: Context<'_>) -> Vec<String> {
    let mut names = Vec::new();
    command_names(&ctx.framework().commands, &mut names);
    names.extend(NON_COMMANDS.iter().map(|name| name.to_string()));
    names.sort();
    names
}

async fn autocomplete_command(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let partial = partial.to_lowercase();
    all_command_names(ctx).into_iter().filter(|name| name.contains(&partial)).take(25).collect()
}

fn format_secs(secs: u64) -> String {
    match secs {
        0 => "none".to_string(),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// Manage command cooldowns for this server
#[command(
    slash_command,
    guild_only,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR",
    subcommands("cooldown_set", "cooldown_list")
)]
pub async fn cooldown(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set how many seconds must pass between uses of a command (0 removes it)
#[command(slash_command, guild_only, ephemeral, rename = "set", required_permissions = "ADMINISTRATOR")]
async fn cooldown_set(
    ctx: Context<'_>,
    #[description = "Command, e.g. \"ping\" or \"eco work\""]
    #[autocomplete = "autocomplete_command"]
    command: String,
    #[description = "Cooldown in seconds (0 for none)"]
    #[max = 604800]
    seconds: u64,
    #[description = "User: each member waits on their own (default). Global: one use makes everyone wait"] scope: Option<Scope>,
) -> Result<(), Error> {
    let scope = scope.unwrap_or_default();
    let command = command.trim().to_lowercase();
    if !all_command_names(ctx).contains(&command) {
        ctx.say(format!("❌ Unknown command '{command}'.")).await?;
        return Ok(());
    }
    let seconds = seconds.min(MAX_COOLDOWN_SECS);
    ctx.data().cooldowns.lock().unwrap().set(ctx.guild_id().unwrap().get(), &command, seconds, scope)?;
    let whom = if scope == Scope::Global { "the whole server" } else { "each member" };
    ctx.say(format!("✅ Cooldown for `/{command}` set to {} for {whom}.", format_secs(seconds))).await?;
    Ok(())
}

/// List the commands that have a cooldown on this server
#[command(slash_command, guild_only, ephemeral, rename = "list", required_permissions = "ADMINISTRATOR")]
async fn cooldown_list(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let lines: Vec<String> = {
        let cooldowns = ctx.data().cooldowns.lock().unwrap();
        all_command_names(ctx)
            .into_iter()
            .filter_map(|name| {
                let secs = cooldowns.seconds(guild_id, &name);
                let scope = if cooldowns.scope(guild_id, &name) == Scope::Global { "global" } else { "per user" };
                (secs > 0).then(|| format!("`/{name}` — {} ({scope})", format_secs(secs)))
            })
            .collect()
    };
    let description = if lines.is_empty() { "No commands have a cooldown.".to_string() } else { lines.join("\n") };
    let embed = serenity::CreateEmbed::new().title("Command Cooldowns").description(description).colour(0x3498db);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        assert_eq!(c.seconds(1, "ping"), 300);
        assert_eq!(c.seconds(1, "eco work"), 0);
        c.set(1, "ping", 0, Scope::User).unwrap();
        c.set(1, "eco work", 10, Scope::User).unwrap();
        assert_eq!(c.seconds(1, "ping"), 0);
        assert_eq!(c.seconds(1, "eco work"), 10);
        assert_eq!(c.seconds(2, "ping"), 300);
    }

    #[test]
    fn cooldown_is_per_member_server_and_command() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        c.set(1, "rank", 10, Scope::User).unwrap();
        assert_eq!(c.start(1, 5, "rank", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(1, 5, "rank", 1004).unwrap(), Err(1010));
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Ok(()));
        assert_eq!(c.start(2, 5, "hack", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(2, 5, "hack", 1299).unwrap(), Err(1300));
        assert_eq!(c.start(1, 5, "eco work", 1004).unwrap(), Ok(()));
        assert_eq!(c.start(1, 5, "rank", 1010).unwrap(), Ok(()));
    }

    #[test]
    fn global_cooldowns_are_shared() {
        let mut c = Cooldowns::open_path(":memory:").unwrap();
        c.set(1, "rank", 10, Scope::Global).unwrap();
        assert_eq!(c.start(1, 5, "rank", 1000).unwrap(), Ok(()));
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Err(1010));
        assert_eq!(c.ready_at(1, 7, "rank", 1004).unwrap(), Some(1010));
        // Other servers have their own
        assert_eq!(c.start(2, 6, "rank", 1004).unwrap(), Ok(()));
        // Back to per member
        c.set(1, "rank", 10, Scope::User).unwrap();
        assert_eq!(c.start(1, 6, "rank", 1004).unwrap(), Ok(()));
    }

    #[test]
    fn cooldown_survives_reopening() {
        let path = std::env::temp_dir().join(format!("cooldowns-test-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        Cooldowns::open_path(path).unwrap().start(1, 10, "hack", 1000).unwrap().unwrap();
        assert_eq!(Cooldowns::open_path(path).unwrap().start(1, 10, "hack", 1100).unwrap(), Err(1300));
        std::fs::remove_file(path).ok();
    }
}
