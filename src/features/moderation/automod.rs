//! `/automod`: creates Discord AutoMod rules. Each kind of rule is its own subcommand, so it
//! only shows the options that work with it.

use std::time::Duration;

use ::serenity::all as serenity;
use serenity::automod::{Action, EventType, KeywordPresetType, Trigger};

use crate::core::framework::{command, Choice, CreateReply};
use crate::features::{title_case, Context, Error};

/// Discord's built-in word lists
#[derive(Debug, Clone, Copy, Choice)]
pub enum WordList {
    #[name = "All"]
    All,
    #[name = "Profanity"]
    Profanity,
    #[name = "Sexual Content"]
    SexualContent,
    #[name = "Slurs"]
    Slurs,
}

impl WordList {
    fn slug(self) -> &'static str {
        match self {
            Self::All => "flagged-words",
            Self::Profanity => "flagged-profanity",
            Self::SexualContent => "flagged-sexual-content",
            Self::Slurs => "flagged-slurs",
        }
    }

    fn presets(self) -> Vec<KeywordPresetType> {
        match self {
            Self::All => vec![KeywordPresetType::Profanity, KeywordPresetType::SexualContent, KeywordPresetType::Slurs],
            Self::Profanity => vec![KeywordPresetType::Profanity],
            Self::SexualContent => vec![KeywordPresetType::SexualContent],
            Self::Slurs => vec![KeywordPresetType::Slurs],
        }
    }
}

/// Message formatting matched with a regex
#[derive(Debug, Clone, Copy, Choice)]
pub enum Pattern {
    All,
    Capitals,
    Spoilers,
    Emojis,
    Headers,
    Unicode,
    #[name = "Code Blocks"]
    CodeBlocks,
    #[name = "Bullet Lists"]
    BulletLists,
    Invites,
    #[name = "Embedded Links"]
    EmbeddedLinks,
}

impl Pattern {
    fn slug(self) -> &'static str {
        match self {
            Self::All => "flagged-patterns",
            Self::Capitals => "flagged-capital",
            Self::Spoilers => "flagged-spoilers",
            Self::Emojis => "flagged-emojis",
            Self::Headers => "flagged-headers",
            Self::Unicode => "flagged-unicode",
            Self::CodeBlocks => "flagged-code-blocks",
            Self::BulletLists => "flagged-bullet-lists",
            Self::Invites => "flagged-invites",
            Self::EmbeddedLinks => "flagged-embedded-links",
        }
    }

    /// Every pattern but `All`, which matches all of them
    const EACH: [Self; 9] = [
        Self::Capitals,
        Self::Spoilers,
        Self::Emojis,
        Self::Headers,
        Self::Unicode,
        Self::CodeBlocks,
        Self::BulletLists,
        Self::Invites,
        Self::EmbeddedLinks,
    ];

    fn regexes(self) -> Vec<String> {
        let mut regexes: Vec<String> = match self {
            Self::All => Self::EACH.iter().map(|p| p.regex().to_string()).collect(),
            _ => vec![self.regex().to_string()],
        };
        // Code blocks and bullet lists share a pattern
        regexes.dedup();
        regexes
    }

    fn regex(self) -> &'static str {
        match self {
            Self::All => unreachable!("`All` has several patterns, see `regexes`"),
            Self::Capitals => "(?-i)([A-Z]s?){1,3}(?:[A-Z]s?){0,15}",
            Self::Spoilers => "(?s)||.*||",
            Self::Emojis => r"((<a?:[a-z_0-9]+:[0-9]+>|\p{Extended_Pictographic})\s?){5,}",
            Self::Headers => "(^|s)(#{1,3}s).+",
            Self::Unicode => r"\p{M}{3,}",
            Self::CodeBlocks | Self::BulletLists => "(^|s)(`{1,3}).+",
            Self::Invites => {
                "(?:(?:https?://)?(?:www)?discord(?:app)?.(?:(?:com|gg)/invite/[a-z0-9-_]+)|(?:https?://)?(?:www)?discord.gg/[a-z0-9-_]+)"
            }
            // Masked links, [text](https://...), which hide where they go
            Self::EmbeddedLinks => r"\[[^\]]*\]\(\s*<?https?://[^)]+\)",
        }
    }
}

/// What to send Discord for a new rule.
enum Rule {
    /// A message rule serenity can build. `raid_protection` is only for mention spam.
    Message { trigger: Trigger, timeout_minutes: Option<u32>, raid_protection: bool },
    /// A "Block Words in Member Profile Names" rule
    ProfileNames { words: Vec<String>, allow_list: Vec<String> },
}

/// Splits a comma-separated option into its trimmed, non-empty entries.
fn split_list(list: Option<String>) -> Vec<String> {
    list.unwrap_or_default().split(',').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect()
}

// Discord API values, for the ones serenity 0.12 doesn't know about or when sending raw JSON
const MEMBER_UPDATE_EVENT: u8 = 2;
const KEYWORD_TRIGGER: u64 = 1;
const MEMBER_PROFILE_TRIGGER: u8 = 6;
const PROFILE_BLOCK_ACTION: u8 = 4;

/// A "Block Words in Member Profile Names" rule, which serenity can't build itself.
/// Members whose name matches are blocked from talking or interacting until they change it.
fn profile_rule(name: &str, words: &[String], allow_list: &[String], alert_channel: Option<serenity::ChannelId>) -> serde_json::Value {
    let mut actions = vec![serde_json::json!({ "type": PROFILE_BLOCK_ACTION })];
    if let Some(channel_id) = alert_channel {
        actions.push(serde_json::json!({ "type": 2, "metadata": { "channel_id": channel_id.to_string() } }));
    }
    serde_json::json!({
        "name": name,
        "event_type": MEMBER_UPDATE_EVENT,
        "trigger_type": MEMBER_PROFILE_TRIGGER,
        "trigger_metadata": { "keyword_filter": words, "allow_list": allow_list },
        "actions": actions,
        "enabled": true,
    })
}

/// Creates the rule, named after `slug`, or overwrites the one the bot already made with that
/// name. Discord allows only one rule of each kind except keyword rules, so for those an
/// existing rule of the same kind is overwritten instead.
/// Alerts go to `alert_channel`. If none is given, an overwritten rule keeps its alert
/// channel, and a new one uses the server's system channel.
async fn create(ctx: Context<'_>, slug: &str, rule: Rule, alert_channel: Option<serenity::GuildChannel>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap();
    let bot_name = ctx.cache().current_user().name.clone();
    let rule_name = format!(
        "AutoMod {} by {}",
        title_case(&slug.replace('-', " ")),
        bot_name
    );

    let trigger_type = match &rule {
        Rule::ProfileNames { .. } => u64::from(MEMBER_PROFILE_TRIGGER),
        Rule::Message { trigger, .. } => u64::from(u8::from(trigger.kind())),
    };
    let existing = guild_id.automod_rules(ctx.http()).await?;
    let replacing = if trigger_type == KEYWORD_TRIGGER {
        existing.iter().find(|rule| rule.name == rule_name)
    } else {
        existing.iter().find(|rule| u64::from(u8::from(rule.trigger.kind())) == trigger_type)
    };

    let alert_channel = match (alert_channel, replacing) {
        (Some(channel), _) => Some(channel.id),
        (None, Some(old)) => old.actions.iter().find_map(|action| match action {
            Action::Alert(channel_id) => Some(*channel_id),
            _ => None,
        }),
        (None, None) => ctx.guild().and_then(|g| g.system_channel_id),
    };
    let mut body = match rule {
        Rule::ProfileNames { words, allow_list } => profile_rule(&rule_name, &words, &allow_list, alert_channel),
        Rule::Message { trigger, timeout_minutes, raid_protection } => {
            let mut actions = vec![Action::BlockMessage {
                custom_message: Some(format!(
                    "This message was prevented by AutoMod rule created by {bot_name}"
                )),
            }];
            if let Some(channel_id) = alert_channel {
                actions.push(Action::Alert(channel_id));
            }
            if let Some(minutes) = timeout_minutes {
                actions.push(Action::Timeout(Duration::from_secs(u64::from(minutes) * 60)));
            }
            let rule = serenity::EditAutoModRule::new()
                .name(&rule_name)
                .event_type(EventType::MessageSend)
                .trigger(trigger)
                .actions(actions)
                .enabled(true);
            let mut rule = serde_json::to_value(&rule)?;
            if raid_protection {
                // serenity has no raid protection setting, so it's added to the request by hand
                rule["trigger_metadata"]["mention_raid_protection_enabled"] = true.into();
            }
            rule
        }
    };

    let reason = format!("Set by {}", ctx.author().tag());
    let created = match replacing {
        Some(old) => {
            ctx.say(format!("Overwriting the existing AutoMod rule '{}'...", old.name)).await?;
            // A rule's kind can't be changed, so Discord doesn't take it when editing
            body.as_object_mut().unwrap().remove("trigger_type");
            ctx.http().edit_automod_rule(guild_id, old.id, &body, Some(&reason)).await
        }
        None => {
            ctx.say("Creating AutoMod rule...").await?;
            ctx.http().create_automod_rule(guild_id, &body, Some(&reason)).await
        }
    };

    match created {
        Ok(_) => {
            let embed = serenity::CreateEmbed::new()
                .title(if replacing.is_some() { "✅ AutoMod Rule Updated" } else { "✅ AutoMod Rule Created" })
                .description(format!("Successfully set up '{rule_name}'"))
                .colour(0x2ecc71);
            ctx.send(CreateReply::default().embed(embed)).await?;
        }
        Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(resp)))
            if resp.status_code.as_u16() == 403 =>
        {
            crate::log::error(format!(
                "Permission error creating AutoMod rule: {}",
                resp.error.message
            ));
            ctx.say("❌ I don't have permission to create AutoMod rules. I need 'Manage Guild' permission (and 'Moderate Members' for timeouts).").await?;
        }
        Err(e) => {
            crate::log::error(format!("Error creating AutoMod rule: {e}"));
            ctx.say(format!("❌ Error creating AutoMod rule: {e}"))
                .await?;
        }
    }
    Ok(())
}

/// Setup Discord AutoMod rules
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "AutoMod",
    required_permissions = "ADMINISTRATOR",
    subcommands("word_list", "words", "profile_names", "pattern", "mention_spam", "message_spam"),
    subcommand_required
)]
pub async fn automod(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Block one of Discord's built-in word lists in messages
#[command(slash_command, guild_only, ephemeral, rename = "word-list", required_permissions = "ADMINISTRATOR")]
async fn word_list(
    ctx: Context<'_>,
    #[description = "The word list to block"] list: WordList,
    #[description = "Comma-separated words that never trigger the rule"]
    #[max_length = 4000]
    allow_list: Option<String>,
    #[description = "Channel to send alerts to (default: the server's system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let trigger = Trigger::KeywordPreset { presets: list.presets(), allow_list: split_list(allow_list) };
    create(ctx, list.slug(), Rule::Message { trigger, timeout_minutes: None, raid_protection: false }, alert_channel).await
}

/// Block your own words in messages
#[command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn words(
    ctx: Context<'_>,
    #[description = "Comma-separated words to block, * as a wildcard"]
    #[max_length = 4000]
    words: String,
    #[description = "Comma-separated words that never trigger the rule"]
    #[max_length = 4000]
    allow_list: Option<String>,
    #[description = "Also time members out for this many minutes"]
    #[min = 1]
    #[max = 40320]
    timeout_minutes: Option<u32>,
    #[description = "Channel to send alerts to (default: the server's system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let words = split_list(Some(words));
    if words.is_empty() {
        ctx.say("❌ Give the words to block, separated by commas.").await?;
        return Ok(());
    }
    let trigger = Trigger::Keyword { strings: words, regex_patterns: vec![], allow_list: split_list(allow_list) };
    create(ctx, "flagged-custom-words", Rule::Message { trigger, timeout_minutes, raid_protection: false }, alert_channel).await
}

/// Block words in member names; matching members can't chat until they change it
#[command(slash_command, guild_only, ephemeral, rename = "profile-names", required_permissions = "ADMINISTRATOR")]
async fn profile_names(
    ctx: Context<'_>,
    #[description = "Comma-separated words to block, * as a wildcard"]
    #[max_length = 4000]
    words: String,
    #[description = "Comma-separated words that never trigger the rule"]
    #[max_length = 4000]
    allow_list: Option<String>,
    #[description = "Channel to send alerts to (default: the server's system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let words = split_list(Some(words));
    if words.is_empty() {
        ctx.say("❌ Give the words to block, separated by commas.").await?;
        return Ok(());
    }
    create(ctx, "flagged-profile-names", Rule::ProfileNames { words, allow_list: split_list(allow_list) }, alert_channel).await
}

/// Block message formatting like capitals, spoilers or invites
#[command(slash_command, guild_only, ephemeral, required_permissions = "ADMINISTRATOR")]
async fn pattern(
    ctx: Context<'_>,
    #[description = "What to block"] pattern: Pattern,
    #[description = "Comma-separated words that never trigger the rule"]
    #[max_length = 4000]
    allow_list: Option<String>,
    #[description = "Also time members out for this many minutes"]
    #[min = 1]
    #[max = 40320]
    timeout_minutes: Option<u32>,
    #[description = "Channel to send alerts to (default: the server's system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let trigger = Trigger::Keyword { strings: vec![], regex_patterns: pattern.regexes(), allow_list: split_list(allow_list) };
    create(ctx, pattern.slug(), Rule::Message { trigger, timeout_minutes, raid_protection: false }, alert_channel).await
}

/// Block messages with too many mentions
#[command(slash_command, guild_only, ephemeral, rename = "mention-spam", required_permissions = "ADMINISTRATOR")]
async fn mention_spam(
    ctx: Context<'_>,
    #[description = "Mentions allowed per message (default 10)"]
    #[min = 1]
    #[max = 50]
    mention_limit: Option<u8>,
    #[description = "Also time members out for this many minutes"]
    #[min = 1]
    #[max = 40320]
    timeout_minutes: Option<u32>,
    #[description = "Also block mention raids across messages"] raid_protection: Option<bool>,
    #[description = "Channel to send alerts to (default: the server's system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let trigger = Trigger::MentionSpam { mention_total_limit: mention_limit.unwrap_or(10) };
    create(ctx, "spammed-mentions", Rule::Message { trigger, timeout_minutes, raid_protection: raid_protection.unwrap_or(false) }, alert_channel).await
}

/// Block messages Discord thinks are spam
#[command(slash_command, guild_only, ephemeral, rename = "message-spam", required_permissions = "ADMINISTRATOR")]
async fn message_spam(
    ctx: Context<'_>,
    #[description = "Channel to send alerts to (default: the server's system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    create(ctx, "spammed-messages", Rule::Message { trigger: Trigger::Spam, timeout_minutes: None, raid_protection: false }, alert_channel).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_serialize_for_discord() {
        let trigger = Trigger::MentionSpam { mention_total_limit: 5 };
        let rule = serde_json::to_value(serenity::EditAutoModRule::new().trigger(trigger)).unwrap();
        assert_eq!(rule["trigger_metadata"]["mention_total_limit"], 5);
        assert_eq!(rule["trigger_type"], 5);

        let words = vec!["scam*".to_string()];
        let allow = vec!["scamp".to_string()];
        let trigger = Trigger::Keyword { strings: words.clone(), regex_patterns: vec![], allow_list: allow.clone() };
        let rule = serde_json::to_value(serenity::EditAutoModRule::new().trigger(trigger)).unwrap();
        assert_eq!(rule["trigger_metadata"]["keyword_filter"][0], "scam*");
        assert_eq!(rule["trigger_metadata"]["allow_list"][0], "scamp");

        assert_eq!(Pattern::All.regexes().len(), 8);
        assert_eq!(Pattern::Invites.regexes().len(), 1);

        let rule = profile_rule("x", &words, &allow, None);
        assert_eq!(rule["trigger_type"], 6);
        assert_eq!(rule["trigger_metadata"]["allow_list"][0], "scamp");
        assert_eq!(split_list(Some(" a, ,b ".into())), ["a", "b"]);
    }
}
