//! A command: what Discord is told about it, the checks it needs and the function it runs.

use ::serenity::all as serenity;

use super::{BoxFuture, Context};
use crate::core::error::Error;

pub type SlashAction = for<'a> fn(Context<'a>) -> BoxFuture<'a, Result<(), Error>>;
pub type CheckFn = for<'a> fn(Context<'a>) -> BoxFuture<'a, Result<bool, Error>>;
pub type AutocompleteFn = for<'a> fn(Context<'a>, &'a str) -> BoxFuture<'a, Vec<serenity::AutocompleteChoice>>;

/// What a right-click menu command runs on.
#[derive(Clone, Copy)]
pub enum ContextMenuAction {
    User(for<'a> fn(Context<'a>, serenity::User) -> BoxFuture<'a, Result<(), Error>>),
    Message(for<'a> fn(Context<'a>, serenity::Message) -> BoxFuture<'a, Result<(), Error>>),
}

/// One option of a slash command.
pub struct Parameter {
    pub name: String,
    pub description: String,
    pub required: bool,
    /// Sets the option's type and limits
    pub create: fn(serenity::CreateCommandOption) -> serenity::CreateCommandOption,
    pub autocomplete: Option<AutocompleteFn>,
}

impl Parameter {
    fn create_as_option(&self) -> serenity::CreateCommandOption {
        let option = serenity::CreateCommandOption::new(serenity::CommandOptionType::String, &self.name, &self.description);
        (self.create)(option).required(self.required)
    }
}

pub struct Command {
    pub name: String,
    /// With its parent commands, e.g. "sticky stick"
    pub qualified_name: String,
    pub description: Option<String>,
    /// The cog it belongs to; subcommands use their top-level command's
    pub category: Option<String>,
    pub guild_only: bool,
    pub owners_only: bool,
    /// Whether replies are only shown to the person who ran it, unless a reply says otherwise
    pub ephemeral: bool,
    pub subcommand_required: bool,
    pub required_permissions: serenity::Permissions,
    pub required_bot_permissions: serenity::Permissions,
    /// Who Discord shows the command to by default
    pub default_member_permissions: serenity::Permissions,
    pub checks: Vec<CheckFn>,
    pub subcommands: Vec<Command>,
    pub parameters: Vec<Parameter>,
    pub slash_action: Option<SlashAction>,
    pub context_menu_action: Option<ContextMenuAction>,
    pub context_menu_name: Option<String>,
}

impl Command {
    fn description_or_default(&self) -> &str {
        self.description.as_deref().unwrap_or("A slash command")
    }

    /// The registration for a subcommand, nested in its parent's.
    fn create_as_subcommand(&self) -> Option<serenity::CreateCommandOption> {
        self.slash_action?;
        let kind = if self.subcommands.is_empty() {
            serenity::CommandOptionType::SubCommand
        } else {
            serenity::CommandOptionType::SubCommandGroup
        };
        let mut option = serenity::CreateCommandOption::new(kind, &self.name, self.description_or_default());
        if self.subcommands.is_empty() {
            for param in &self.parameters {
                option = option.add_sub_option(param.create_as_option());
            }
        } else {
            for sub in self.subcommands.iter().filter_map(Command::create_as_subcommand) {
                option = option.add_sub_option(sub);
            }
        }
        Some(option)
    }

    /// The slash command registration, if it's a slash command.
    pub fn create_as_slash_command(&self) -> Option<serenity::CreateCommand> {
        self.slash_action?;
        let mut command = serenity::CreateCommand::new(&self.name).description(self.description_or_default());
        // Discord takes empty default permissions to mean admins only
        let visible_to = self.visible_to();
        if !visible_to.is_empty() {
            command = command.default_member_permissions(visible_to);
        }
        if self.guild_only {
            command = command.contexts(vec![serenity::InteractionContext::Guild]);
        }
        if self.subcommands.is_empty() {
            for param in &self.parameters {
                command = command.add_option(param.create_as_option());
            }
        } else {
            for sub in self.subcommands.iter().filter_map(Command::create_as_subcommand) {
                command = command.add_option(sub);
            }
        }
        Some(command)
    }

    /// The right-click menu registration, if it's a context menu command.
    pub fn create_as_context_menu_command(&self) -> Option<serenity::CreateCommand> {
        let kind = match self.context_menu_action? {
            ContextMenuAction::User(_) => serenity::CommandType::User,
            ContextMenuAction::Message(_) => serenity::CommandType::Message,
        };
        let mut command = serenity::CreateCommand::new(self.context_menu_name.as_deref().unwrap_or(&self.name)).kind(kind);
        let visible_to = self.visible_to();
        if !visible_to.is_empty() {
            command = command.default_member_permissions(visible_to);
        }
        if self.guild_only {
            command = command.contexts(vec![serenity::InteractionContext::Guild]);
        }
        Some(command)
    }

    /// The permissions Discord shows the command to: `default_member_permissions` if it's
    /// set, otherwise the ones every way of running it needs, so members only see commands
    /// they can use. Server admins, and anyone a server gives access in its settings, see it anyway.
    pub fn visible_to(&self) -> serenity::Permissions {
        if !self.default_member_permissions.is_empty() {
            return self.default_member_permissions;
        }
        self.needed_permissions()
    }

    /// Permissions needed to run the command, or for a group, the ones every subcommand needs.
    fn needed_permissions(&self) -> serenity::Permissions {
        let runnable = self.subcommands.iter().filter(|sub| sub.slash_action.is_some() && !sub.owners_only);
        runnable
            .map(Command::needed_permissions)
            .reduce(|a, b| a & b)
            .unwrap_or(self.required_permissions)
    }

    /// Fills in `qualified_name` for this command's subcommands, all the way down.
    pub(super) fn qualify_subcommands(&mut self) {
        for sub in &mut self.subcommands {
            sub.qualified_name = format!("{} {}", self.qualified_name, sub.name);
            sub.qualify_subcommands();
        }
    }
}
