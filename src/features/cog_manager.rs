use ::serenity::all as serenity;

use crate::core::framework::{command, Command, CreateReply};
use super::{normalize_cog_name, Context, Data, Error, COGS, COG_MANAGER};

/// Slash and context menu registrations for the commands that pass `keep`, given
/// their cog and name. Commands without a cog count as CogManager, so they're always available.
pub fn application_commands(commands: &[Command], keep: impl Fn(&str, &str) -> bool) -> Vec<serenity::CreateCommand> {
    commands
        .iter()
        .filter(|command| keep(&command.category.as_deref().map_or(COG_MANAGER.to_string(), normalize_cog_name), &command.name))
        .flat_map(|command| [command.create_as_slash_command(), command.create_as_context_menu_command()])
        .flatten()
        .collect()
}

/// Whether a cog's commands are registered globally rather than per guild: CogManager, and
/// the owner-only Developer commands, which belong to no cog.
fn is_global(cog: &str) -> bool {
    cog == COG_MANAGER || cog == normalize_cog_name(crate::features::developer::CATEGORY)
}

/// Registers the CogManager and Developer commands globally; every other cog's commands are per guild.
pub async fn sync_global_commands(http: &serenity::Http, commands: &[Command]) -> serenity::Result<usize> {
    let global = application_commands(commands, |cog, _| is_global(cog));
    let count = global.len();
    serenity::Command::set_global_commands(http, global).await?;
    Ok(count)
}

/// Gives a guild the commands of the cogs (and games) it has enabled, removing the rest.
pub async fn sync_guild_commands(
    http: &serenity::Http,
    commands: &[Command],
    data: &Data,
    guild_id: serenity::GuildId,
) -> serenity::Result<usize> {
    let enabled = {
        let config = data.cog_config.lock().unwrap();
        application_commands(commands, |cog, name| {
            !is_global(cog)
                && config.is_enabled(guild_id.get(), cog)
                && (cog != "games" || config.is_game_enabled(guild_id.get(), name))
        })
    };
    let count = enabled.len();
    guild_id.set_commands(http, enabled).await?;
    Ok(count)
}

/// Re-syncs the current guild's commands after a cog (or game) is enabled or disabled.
pub(crate) async fn resync(ctx: Context<'_>) -> String {
    let guild_id = ctx.guild_id().unwrap();
    match sync_guild_commands(ctx.http(), &ctx.framework().commands, ctx.data(), guild_id).await {
        Ok(_) => String::new(),
        Err(e) => {
            crate::log::error(format!("Error syncing commands for guild {guild_id}: {e}"));
            "\n⚠️ Couldn't update the server's slash commands; they'll update on the next restart.".to_string()
        }
    }
}

async fn autocomplete_cog_name(
    _ctx: Context<'_>,
    partial: &str,
) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    COGS.iter()
        .filter(|cog| {
            normalize_cog_name(cog) != COG_MANAGER && cog.to_lowercase().contains(&partial)
        })
        .map(|cog| serenity::AutocompleteChoice::new(*cog, normalize_cog_name(cog)))
        .collect()
}

/// Resolves user input to a known cog's config name.
fn resolve_cog(input: &str) -> Option<String> {
    let input = normalize_cog_name(input);
    COGS.iter()
        .map(|cog| normalize_cog_name(cog))
        .find(|cog| *cog == input)
}

/// Enable a cog for this server
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn enablecog(
    ctx: Context<'_>,
    #[description = "Cog to enable"]
    #[autocomplete = "autocomplete_cog_name"]
    cog_name: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let Some(cog_name) = resolve_cog(&cog_name) else {
        ctx.say(format!("❌ Unknown cog '{cog_name}'.")).await?;
        return Ok(());
    };

    ctx.data()
        .cog_config
        .lock()
        .unwrap()
        .set_enabled(guild_id, &cog_name, true)?;
    let warning = resync(ctx).await;
    let note = if cog_name == "games" { "\nℹ️ Every game starts off; turn on the ones you want with `/enablegame`." } else { "" };
    ctx.say(format!(
        "✅ Cog '{cog_name}' has been enabled for this server.{note}{warning}"
    ))
    .await?;
    Ok(())
}

/// Disable a cog for this server
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn disablecog(
    ctx: Context<'_>,
    #[description = "Cog to disable"]
    #[autocomplete = "autocomplete_cog_name"]
    cog_name: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let Some(cog_name) = resolve_cog(&cog_name) else {
        ctx.say(format!("❌ Unknown cog '{cog_name}'.")).await?;
        return Ok(());
    };

    if cog_name == COG_MANAGER {
        ctx.say("❌ You cannot disable the CogManager cog.").await?;
        return Ok(());
    }

    ctx.data()
        .cog_config
        .lock()
        .unwrap()
        .set_enabled(guild_id, &cog_name, false)?;
    let warning = resync(ctx).await;
    ctx.say(format!(
        "✅ Cog '{cog_name}' has been disabled for this server.{warning}"
    ))
    .await?;
    Ok(())
}

/// Check the status of all cogs for this server
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "CogManager",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn cogstatus(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap().get();

    let status_msg = {
        let config = ctx.data().cog_config.lock().unwrap();
        COGS.iter()
            .map(|cog| {
                let status = if config.is_enabled(guild_id, &normalize_cog_name(cog)) {
                    "✅ Enabled"
                } else {
                    "❌ Disabled"
                };
                format!("{status}: {cog}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let embed = serenity::CreateEmbed::new()
        .title("Cog Status")
        .description(status_msg)
        .colour(0x3498db);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cog_manager_commands_are_global() {
        let commands = crate::core::command_router::commands();
        let uncategorised: Vec<_> = commands.iter().filter(|c| c.category.is_none()).map(|c| c.name.clone()).collect();
        assert!(uncategorised.is_empty(), "no cog: {uncategorised:?}");
        let global = application_commands(&commands, |cog, _| is_global(cog));
        let cog_manager = commands.iter().filter(|c| matches!(c.category.as_deref(), Some("CogManager" | "Developer"))).count();
        assert_eq!(global.len(), cog_manager);
    }

    /// Discord rejects a whole batch of commands if one breaks its limits, so check them all:
    /// names 1-32 lowercase characters, descriptions 1-100, at most 25 options or choices.
    #[test]
    fn commands_fit_discords_limits() {
        fn check(command: &Command, path: &str, problems: &mut Vec<String>) {
            let path = format!("{path}/{}", command.name);
            let description = command.description.as_deref().unwrap_or_default();
            if command.context_menu_name.is_none() {
                if !(1..=32).contains(&command.name.chars().count()) || command.name.chars().any(|c| c.is_uppercase() || c.is_whitespace()) {
                    problems.push(format!("{path}: bad name"));
                }
                if !(1..=100).contains(&description.chars().count()) {
                    problems.push(format!("{path}: description is {} characters", description.chars().count()));
                }
            }
            if command.parameters.len() + command.subcommands.len() > 25 {
                problems.push(format!("{path}: more than 25 options"));
            }
            // A right-click menu's one option is the user or message clicked, with no description
            for parameter in command.parameters.iter().filter(|_| command.context_menu_name.is_none()) {
                let name = format!("{path} {}", parameter.name);
                let length = parameter.description.chars().count();
                if !(1..=100).contains(&length) {
                    problems.push(format!("{name}: description is {length} characters"));
                }
                if !(1..=32).contains(&parameter.name.chars().count()) || parameter.name.chars().any(|c| c.is_uppercase() || c.is_whitespace()) {
                    problems.push(format!("{name}: bad name"));
                }
                let option = (parameter.create)(serenity::CreateCommandOption::new(serenity::CommandOptionType::String, "x", "x"));
                let choices: Vec<String> = serde_json::to_value(option).unwrap()["choices"]
                    .as_array()
                    .map(|c| c.iter().filter_map(|c| c["name"].as_str().map(String::from)).collect())
                    .unwrap_or_default();
                if choices.len() > 25 {
                    problems.push(format!("{name}: more than 25 choices"));
                }
                for choice in &choices {
                    if !(1..=100).contains(&choice.chars().count()) {
                        problems.push(format!("{name}: choice {choice:?} is too long"));
                    }
                }
            }
            for subcommand in &command.subcommands {
                check(subcommand, &path, problems);
            }
        }
        let mut problems = vec![];
        for command in crate::core::command_router::commands() {
            check(&command, "", &mut problems);
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn every_game_can_be_toggled() {
        let mut games: Vec<_> = crate::core::command_router::commands().into_iter().filter(|c| c.category.as_deref() == Some("Games") && !["enablegame", "disablegame", "gamestatus", "games"].contains(&c.name.as_str())).map(|c| c.name).collect();
        games.sort();
        assert_eq!(games, super::super::games::GAMES);
    }
}
