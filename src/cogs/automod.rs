use poise::serenity_prelude as serenity;
use serenity::automod::{Action, EventType, KeywordPresetType, Trigger};

use super::{title_case, Context, Error};

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum RuleType {
    #[name = "Flagged Words"]
    #[name = "flagged-words"]
    FlaggedWords,
    #[name = "Flagged Capitals"]
    #[name = "flagged-capital"]
    FlaggedCapital,
    #[name = "Flagged Spoilers"]
    #[name = "flagged-spoilers"]
    FlaggedSpoilers,
    #[name = "Flagged Emojis"]
    #[name = "flagged-emojis"]
    FlaggedEmojis,
    #[name = "Flagged Headers"]
    #[name = "flagged-headers"]
    FlaggedHeaders,
    #[name = "Flagged Unicode"]
    #[name = "flagged-unicode"]
    FlaggedUnicode,
    #[name = "Flagged Code Blocks"]
    #[name = "flagged-code-blocks"]
    FlaggedCodeBlocks,
    #[name = "Flagged Bullet Lists"]
    #[name = "flagged-bullet-lists"]
    FlaggedBulletLists,
    #[name = "Flagged Invites"]
    #[name = "flagged-invites"]
    FlaggedInvites,
    #[name = "Spammed Mentions"]
    #[name = "spammed-mentions"]
    SpammedMentions,
    #[name = "Spammed Messages"]
    #[name = "spammed-messages"]
    SpammedMessages,
}

impl RuleType {
    fn slug(self) -> &'static str {
        match self {
            Self::FlaggedWords => "flagged-words",
            Self::FlaggedCapital => "flagged-capital",
            Self::FlaggedSpoilers => "flagged-spoilers",
            Self::FlaggedEmojis => "flagged-emojis",
            Self::FlaggedHeaders => "flagged-headers",
            Self::FlaggedUnicode => "flagged-unicode",
            Self::FlaggedCodeBlocks => "flagged-code-blocks",
            Self::FlaggedBulletLists => "flagged-bullet-lists",
            Self::FlaggedInvites => "flagged-invites",
            Self::SpammedMentions => "spammed-mentions",
            Self::SpammedMessages => "spammed-messages",
        }
    }

    fn trigger(self, mention_limit: u8) -> Trigger {
        let regex = |pattern: &str| Trigger::Keyword {
            strings: vec![],
            regex_patterns: vec![pattern.to_string()],
            allow_list: vec![],
        };

        match self {
            Self::FlaggedWords => Trigger::KeywordPreset {
                presets: vec![KeywordPresetType::Profanity, KeywordPresetType::SexualContent, KeywordPresetType::Slurs],
                allow_list: vec![],
            },
            Self::FlaggedCapital => regex("(?-i)([A-Z]s?){1,3}(?:[A-Z]s?){0,15}"),
            Self::FlaggedSpoilers => regex("(?s)||.*||"),
            Self::FlaggedEmojis => regex(r"((<a?:[a-z_0-9]+:[0-9]+>|\p{Extended_Pictographic})\s?){5,}"),
            Self::FlaggedHeaders => regex("(^|s)(#{1,3}s).+"),
            Self::FlaggedUnicode => regex(r"\p{M}{3,}"),
            Self::FlaggedCodeBlocks | Self::FlaggedBulletLists => regex("(^|s)(`{1,3}).+"),
            Self::FlaggedInvites => regex(
                "(?:(?:https?://)?(?:www)?discord(?:app)?.(?:(?:com|gg)/invite/[a-z0-9-_]+)|(?:https?://)?(?:www)?discord.gg/[a-z0-9-_]+)",
            ),
            Self::SpammedMentions => Trigger::MentionSpam { mention_total_limit: mention_limit },
            Self::SpammedMessages => Trigger::Spam,
        }
    }
}

/// Setup Discord AutoMod rules
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    ephemeral,
    category = "AutoMod",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn automod(
    ctx: Context<'_>,
    #[description = "Type of AutoMod rule to create"] rule_type: RuleType,
    #[description = "Mention limit (for spammed-mentions)"]
    #[min = 1]
    #[max = 50]
    mention_limit: Option<u8>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap();
    let bot_name = ctx.cache().current_user().name.clone();
    let rule_name = format!(
        "AutoMod {} by {}",
        title_case(&rule_type.slug().replace('-', " ")),
        bot_name
    );

    let existing = guild_id.automod_rules(ctx.http()).await?;
    if existing.iter().any(|rule| rule.name == rule_name) {
        ctx.say(format!("❌ AutoMod rule '{rule_name}' already exists!"))
            .await?;
        return Ok(());
    }

    ctx.say("Creating AutoMod rule...").await?;

    let mut actions = vec![Action::BlockMessage {
        custom_message: Some(format!(
            "This message was prevented by AutoMod rule created by {bot_name}"
        )),
    }];
    let system_channel = ctx.guild().and_then(|g| g.system_channel_id);
    if let Some(channel_id) = system_channel {
        actions.push(Action::Alert(channel_id));
    }

    let reason = format!("Created by {}", ctx.author().tag());
    let rule = serenity::EditAutoModRule::new()
        .name(&rule_name)
        .event_type(EventType::MessageSend)
        .trigger(rule_type.trigger(mention_limit.unwrap_or(10)))
        .actions(actions)
        .audit_log_reason(&reason);

    match guild_id.create_automod_rule(ctx.http(), rule).await {
        Ok(_) => {
            let embed = serenity::CreateEmbed::new()
                .title("✅ AutoMod Rule Created")
                .description(format!("Successfully created '{rule_name}'"))
                .colour(0x2ecc71);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(resp)))
            if resp.status_code.as_u16() == 403 =>
        {
            crate::log::error(format!(
                "Permission error creating AutoMod rule: {}",
                resp.error.message
            ));
            ctx.say("❌ I don't have permission to create AutoMod rules. I need 'Manage Guild' permission.").await?;
        }
        Err(e) => {
            crate::log::error(format!("Error creating AutoMod rule: {e}"));
            ctx.say(format!("❌ Error creating AutoMod rule: {e}"))
                .await?;
        }
    }
    Ok(())
}
