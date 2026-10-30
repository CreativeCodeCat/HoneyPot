//! The framework: Serenity hands it every gateway event. Slash commands, context menu
//! commands and autocomplete are run here; every event then goes to the event router.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ::serenity::all as serenity;
use futures_util::FutureExt;

use super::{BoxFuture, Command, ContextMenuAction, Context};
use crate::core::error::Error;
use crate::core::state::Data;
use crate::core::{command_router, event_router, permissions};
use crate::log;

/// Why a command didn't run, or failed.
pub enum FrameworkError<'a> {
    /// The command returned an error (or an option couldn't be read)
    Command { ctx: Context<'a>, error: Error },
    /// The command panicked
    Panic { ctx: Context<'a> },
    /// A check said no; with a message to show, or `None` for "this cog is disabled"
    CommandCheckFailed { ctx: Context<'a>, error: Option<Error> },
    MissingUserPermissions { ctx: Context<'a>, missing: serenity::Permissions },
    MissingBotPermissions { ctx: Context<'a>, missing: serenity::Permissions },
    NotAnOwner { ctx: Context<'a> },
    GuildOnly { ctx: Context<'a> },
}

pub struct Framework {
    pub commands: Vec<Command>,
    /// Who owns the bot (or is on the team that does), from Discord
    pub owners: HashSet<serenity::UserId>,
    pub bot_id: serenity::UserId,
    data: Data,
    shard_manager: Option<Arc<serenity::ShardManager>>,
}

impl Framework {
    pub fn new(mut commands: Vec<Command>, data: Data) -> Self {
        for command in &mut commands {
            command.qualify_subcommands();
        }
        Self { commands, owners: HashSet::new(), bot_id: serenity::UserId::new(1), data, shard_manager: None }
    }

    pub fn data(&self) -> &Data {
        &self.data
    }

    /// The latency of a shard's gateway connection.
    pub(super) async fn latency(&self, shard_id: serenity::ShardId) -> std::time::Duration {
        let Some(manager) = &self.shard_manager else { return std::time::Duration::ZERO };
        manager.runners.lock().await.get(&shard_id).and_then(|runner| runner.latency).unwrap_or_default()
    }

    async fn run_command(&self, ctx: &serenity::Context, interaction: &serenity::CommandInteraction) {
        let options = interaction.data.options();
        let mut parents = vec![];
        let Some((command, args)) = find_command(&interaction.data.name, &options, &self.commands, &mut parents) else {
            return log::warning(format!("Received unknown command /{}", interaction.data.name));
        };
        let sent = AtomicBool::new(false);
        let ctx = Context {
            serenity_context: ctx,
            framework: self,
            interaction,
            args,
            command,
            parent_commands: &parents,
            has_sent_initial_response: &sent,
        };
        let running: BoxFuture<'_, Result<(), FrameworkError<'_>>> = Box::pin(run(ctx));
        let result = match AssertUnwindSafe(running).catch_unwind().await {
            Ok(result) => result,
            Err(_) => Err(FrameworkError::Panic { ctx }),
        };
        if let Err(error) = result {
            let reporting: BoxFuture<'_, ()> = Box::pin(command_router::on_error(error));
            reporting.await;
        }
    }

    async fn run_autocomplete(&self, ctx: &serenity::Context, interaction: &serenity::CommandInteraction) {
        let options = interaction.data.options();
        let mut parents = vec![];
        let Some((command, args)) = find_command(&interaction.data.name, &options, &self.commands, &mut parents) else { return };
        let sent = AtomicBool::new(false);
        let ctx = Context {
            serenity_context: ctx,
            framework: self,
            interaction,
            args,
            command,
            parent_commands: &parents,
            has_sent_initial_response: &sent,
        };
        // Server lists and the like mustn't be suggested to people who can't use the command
        if access_checks(ctx).is_err() {
            return;
        }
        let Some(focused) = interaction.data.autocomplete() else { return };
        let Some(callback) = command.parameters.iter().find(|p| p.name == focused.name).and_then(|p| p.autocomplete) else { return };
        let partial: &str = focused.value;
        let mut choices = callback(ctx, partial).await;
        // Discord's limit
        choices.truncate(25);
        let response = serenity::CreateInteractionResponse::Autocomplete(serenity::CreateAutocompleteResponse::new().set_choices(choices));
        if let Err(e) = interaction.create_response(ctx.serenity_context, response).await {
            log::warning(format!("Couldn't send autocomplete choices for /{}: {e}", command.qualified_name));
        }
    }
}

#[serenity::async_trait]
impl serenity::Framework for Framework {
    async fn init(&mut self, client: &serenity::Client) {
        self.shard_manager = Some(client.shard_manager.clone());
        match client.http.get_current_application_info().await {
            Ok(info) => {
                self.owners.extend(info.owner.map(|owner| owner.id));
                if let Some(team) = info.team {
                    self.owners.extend(team.members.into_iter().map(|member| member.user.id));
                }
            }
            Err(e) => log::error(format!("Couldn't read who owns the bot: {e}")),
        }
        match client.http.get_current_user().await {
            Ok(user) => self.bot_id = user.id,
            Err(e) => log::error(format!("Couldn't read the bot's own user: {e}")),
        }
    }

    async fn dispatch(&self, ctx: serenity::Context, event: serenity::FullEvent) {
        // Each future is boxed as Send where it's made: rustc can't prove one big nested
        // future is Send for every lifetime, but can for each of these
        let handler: BoxFuture<'_, Result<(), Error>> = Box::pin(event_router::event_handler(&ctx, &event, self, &self.data));
        if let Err(e) = handler.await {
            log::error(format!("Error handling {} event: {e}", event.snake_case_name()));
        }
        if let serenity::FullEvent::InteractionCreate { interaction } = &event {
            let run: BoxFuture<'_, ()> = match interaction {
                serenity::Interaction::Command(command) => Box::pin(self.run_command(&ctx, command)),
                serenity::Interaction::Autocomplete(autocomplete) => Box::pin(self.run_autocomplete(&ctx, autocomplete)),
                _ => return,
            };
            run.await;
        }
    }
}

/// The command an interaction is for, following its subcommands, and the options given to it.
fn find_command<'a, 'b>(
    name: &str,
    options: &'b [serenity::ResolvedOption<'b>],
    commands: &'a [Command],
    parents: &mut Vec<&'a Command>,
) -> Option<(&'a Command, &'b [serenity::ResolvedOption<'b>])> {
    let command = commands.iter().find(|c| c.name == name || c.context_menu_name.as_deref() == Some(name))?;
    let sub = options.iter().find_map(|option| match &option.value {
        serenity::ResolvedValue::SubCommand(sub) | serenity::ResolvedValue::SubCommandGroup(sub) => Some((option.name, sub)),
        _ => None,
    });
    match sub {
        Some((sub_name, sub_options)) => {
            parents.push(command);
            find_command(sub_name, sub_options, &command.subcommands, parents)
        }
        None => Some((command, options)),
    }
}

/// The checks on who can use the command at all, for it and each command above it.
fn access_checks(ctx: Context<'_>) -> Result<(), FrameworkError<'_>> {
    for command in ctx.parent_commands.iter().copied().chain([ctx.command]) {
        if command.owners_only && !ctx.framework.owners.contains(&ctx.author().id) {
            return Err(FrameworkError::NotAnOwner { ctx });
        }
        if command.guild_only && ctx.guild_id().is_none() {
            return Err(FrameworkError::GuildOnly { ctx });
        }
        // Discord sends the member's permissions in this channel; in DMs there's nothing to check
        if let Some(member) = &ctx.interaction.member {
            let have = member.permissions.unwrap_or_else(serenity::Permissions::empty);
            if !have.contains(command.required_permissions) {
                return Err(FrameworkError::MissingUserPermissions { ctx, missing: command.required_permissions - have });
            }
        }
        if let Some(have) = ctx.interaction.app_permissions {
            if !have.contains(command.required_bot_permissions) {
                return Err(FrameworkError::MissingBotPermissions { ctx, missing: command.required_bot_permissions - have });
            }
        }
    }
    Ok(())
}

/// Runs the checks, then the command.
async fn run(ctx: Context<'_>) -> Result<(), FrameworkError<'_>> {
    access_checks(ctx)?;
    let global: super::CheckFn = |ctx| Box::pin(permissions::command_check(ctx));
    let mut checks = vec![global];
    for command in ctx.parent_commands.iter().copied().chain([ctx.command]) {
        checks.extend(command.checks.iter().copied());
    }
    for check in checks {
        match check(ctx).await {
            Ok(true) => {}
            Ok(false) => return Err(FrameworkError::CommandCheckFailed { ctx, error: None }),
            Err(error) => return Err(FrameworkError::CommandCheckFailed { ctx, error: Some(error) }),
        }
    }

    let result = match ctx.interaction.data.kind {
        serenity::CommandType::ChatInput => match ctx.command.slash_action {
            Some(action) => action(ctx).await,
            None => return Ok(()),
        },
        kind => match (ctx.command.context_menu_action, ctx.interaction.data.target()) {
            (Some(ContextMenuAction::User(action)), Some(serenity::ResolvedTarget::User(user, _))) => action(ctx, user.clone()).await,
            (Some(ContextMenuAction::Message(action)), Some(serenity::ResolvedTarget::Message(message))) => action(ctx, message.clone()).await,
            _ => return Ok(log::warning(format!("Received a {kind:?} interaction /{} can't take", ctx.command.qualified_name))),
        },
    };
    result.map_err(|error| FrameworkError::Command { ctx, error })
}
