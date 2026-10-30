//! Replies to commands, and editing them afterwards.

use ::serenity::all as serenity;

use super::Context;

/// A reply: text, embeds, files and buttons. Ephemeral unless the command or the reply says not.
#[derive(Default, Clone)]
pub struct CreateReply {
    pub content: Option<String>,
    pub embeds: Vec<serenity::CreateEmbed>,
    pub attachments: Vec<serenity::CreateAttachment>,
    pub ephemeral: Option<bool>,
    pub components: Option<Vec<serenity::CreateActionRow>>,
    pub allowed_mentions: Option<serenity::CreateAllowedMentions>,
}

impl CreateReply {
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn embed(mut self, embed: serenity::CreateEmbed) -> Self {
        self.embeds.push(embed);
        self
    }

    pub fn components(mut self, components: Vec<serenity::CreateActionRow>) -> Self {
        self.components = Some(components);
        self
    }

    pub fn attachment(mut self, attachment: serenity::CreateAttachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = Some(ephemeral);
        self
    }

    pub fn allowed_mentions(mut self, allowed_mentions: serenity::CreateAllowedMentions) -> Self {
        self.allowed_mentions = Some(allowed_mentions);
        self
    }

    pub(super) fn to_initial_response(self, mut builder: serenity::CreateInteractionResponseMessage) -> serenity::CreateInteractionResponseMessage {
        if let Some(content) = self.content {
            builder = builder.content(content);
        }
        if let Some(allowed_mentions) = self.allowed_mentions {
            builder = builder.allowed_mentions(allowed_mentions);
        }
        if let Some(components) = self.components {
            builder = builder.components(components);
        }
        if let Some(ephemeral) = self.ephemeral {
            builder = builder.ephemeral(ephemeral);
        }
        builder.add_files(self.attachments).embeds(self.embeds)
    }

    pub(super) fn to_followup(self, mut builder: serenity::CreateInteractionResponseFollowup) -> serenity::CreateInteractionResponseFollowup {
        if let Some(content) = self.content {
            builder = builder.content(content);
        }
        builder = builder.embeds(self.embeds);
        if let Some(components) = self.components {
            builder = builder.components(components);
        }
        if let Some(allowed_mentions) = self.allowed_mentions {
            builder = builder.allowed_mentions(allowed_mentions);
        }
        if let Some(ephemeral) = self.ephemeral {
            builder = builder.ephemeral(ephemeral);
        }
        builder.add_files(self.attachments)
    }

    fn to_response_edit(self, mut builder: serenity::EditInteractionResponse) -> serenity::EditInteractionResponse {
        if let Some(content) = self.content {
            builder = builder.content(content);
        }
        if let Some(components) = self.components {
            builder = builder.components(components);
        }
        if let Some(allowed_mentions) = self.allowed_mentions {
            builder = builder.allowed_mentions(allowed_mentions);
        }
        for attachment in self.attachments {
            builder = builder.new_attachment(attachment);
        }
        builder.embeds(self.embeds)
    }
}

/// A reply that was sent, to edit or delete later.
#[derive(Clone)]
pub struct ReplyHandle<'a> {
    pub(super) http: &'a serenity::Http,
    pub(super) interaction: &'a serenity::CommandInteraction,
    /// The follow-up message, if it wasn't the first reply
    pub(super) followup: Option<Box<serenity::Message>>,
}

impl ReplyHandle<'_> {
    /// The message the reply became.
    pub async fn message(&self) -> Result<serenity::Message, serenity::Error> {
        match &self.followup {
            Some(message) => Ok((**message).clone()),
            None => self.interaction.get_response(self.http).await,
        }
    }

    pub async fn into_message(self) -> Result<serenity::Message, serenity::Error> {
        match self.followup {
            Some(message) => Ok(*message),
            None => self.interaction.get_response(self.http).await,
        }
    }

    /// Replaces the reply's text, embeds and buttons with `reply`'s.
    pub async fn edit(&self, _ctx: Context<'_>, reply: CreateReply) -> Result<(), serenity::Error> {
        match &self.followup {
            None => {
                self.interaction.edit_response(self.http, reply.to_response_edit(serenity::EditInteractionResponse::new())).await?;
            }
            Some(message) => {
                let builder = reply.to_followup(serenity::CreateInteractionResponseFollowup::new());
                self.interaction.edit_followup(self.http, message.id, builder).await?;
            }
        }
        Ok(())
    }

    pub async fn delete(&self, _ctx: Context<'_>) -> Result<(), serenity::Error> {
        match &self.followup {
            None => self.interaction.delete_response(self.http).await,
            Some(message) => self.interaction.delete_followup(self.http, message.id).await,
        }
    }
}
