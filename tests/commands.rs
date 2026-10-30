//! Checks on the full command list, as the bot registers it.

use std::collections::HashSet;

use honeypot_bot::core::command_router::commands;
use honeypot_bot::features::{developer, normalize_cog_name, COGS};

#[test]
fn command_names_are_unique() {
    let mut seen = HashSet::new();
    for command in commands() {
        assert!(seen.insert(command.name.clone()), "/{} is registered twice", command.name);
    }
}

#[test]
fn every_command_belongs_to_a_known_cog() {
    let cogs: HashSet<String> = COGS.iter().map(|cog| normalize_cog_name(cog)).collect();
    for command in commands() {
        let Some(category) = command.category.as_deref() else { continue };
        assert!(
            category == developer::CATEGORY || cogs.contains(&normalize_cog_name(category)),
            "/{} is in the unknown cog {category}",
            command.name
        );
    }
}

#[test]
fn commands_are_only_hidden_from_members_who_cant_use_them() {
    use honeypot_bot::core::framework::Command;
    // Every subcommand a member can run must need at least the permissions the command is
    // shown to, or Discord would hide it from people allowed to use it
    fn check(command: &Command, shown_to: serenity::all::Permissions, path: &str) {
        let path = format!("{path} {}", command.name);
        if command.subcommands.is_empty() {
            if command.slash_action.is_some() && !command.owners_only {
                assert!(command.required_permissions.contains(shown_to), "/{} is hidden from members who can use it", path.trim());
            }
            return;
        }
        for sub in &command.subcommands {
            check(sub, shown_to, &path);
        }
    }
    let mut hidden = 0;
    for command in commands() {
        // Commands that set who sees them on purpose are left as they are
        if !command.default_member_permissions.is_empty() {
            continue;
        }
        let shown_to = command.visible_to();
        if !shown_to.is_empty() {
            hidden += 1;
        }
        check(&command, shown_to, "");
    }
    assert!(hidden > 0, "no command is hidden from members without its permissions");
}
