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
