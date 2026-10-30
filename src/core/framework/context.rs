//! What a command gets when it runs: who ran it, where, its options, and ways to reply.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ::serenity::all as serenity;

use super::{Command, CreateReply, Framework, ReplyHandle};
use crate::core::state::Data;

#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub serenity_context: &'a serenity::Context,
    pub framework: &'a Framework,
    pub interaction: &'a serenity::CommandInteraction,
    /// The options given to the command itself (inside any subcommands)
    pub args: &'a [serenity::ResolvedOption<'a>],
    pub command: &'a Command,
    /// For a subcommand, the commands above it, top-level first
    pub parent_commands: &'a [&'a Command],
    pub(super) has_sent_initial_response: &'a AtomicBool,
}

impl<'a> Context<'a> {
    pub fn data(self) -> &'a Data {
        self.framework.data()
    }

    pub fn author(self) -> &'a serenity::User {
        &self.interaction.user
    }

    /// The person who ran the command, as a server member (not in DMs).
    pub async fn author_member(self) -> Option<Cow<'a, serenity::Member>> {
        self.interaction.member.as_deref().map(Cow::Borrowed)
    }

    pub fn guild_id(self) -> Option<serenity::GuildId> {
        self.interaction.guild_id
    }

    pub fn channel_id(self) -> serenity::ChannelId {
        self.interaction.channel_id
    }

    /// The server, from the cache.
    pub fn guild(self) -> Option<serenity::GuildRef<'a>> {
        self.guild_id()?.to_guild_cached(&self.serenity_context.cache)
    }

    pub fn cache(self) -> &'a Arc<serenity::Cache> {
        &self.serenity_context.cache
    }

    pub fn http(self) -> &'a serenity::Http {
        &self.serenity_context.http
    }

    pub fn serenity_context(self) -> &'a serenity::Context {
        self.serenity_context
    }

    pub fn framework(self) -> &'a Framework {
        self.framework
    }

    pub fn command(self) -> &'a Command {
        self.command
    }

    pub fn parent_commands(self) -> &'a [&'a Command] {
        self.parent_commands
    }

    /// The interaction's ID, which is unique to this use of the command.
    pub fn id(self) -> u64 {
        self.interaction.id.get()
    }

    pub fn created_at(self) -> serenity::Timestamp {
        self.interaction.id.created_at()
    }

    /// The gateway connection's latency.
    pub async fn ping(self) -> std::time::Duration {
        self.framework.latency(self.serenity_context.shard_id).await
    }

    pub fn set_presence(self, activity: Option<serenity::ActivityData>, status: serenity::OnlineStatus) {
        self.serenity_context.set_presence(activity, status);
    }

    /// Fills in what the reply doesn't say from the command: whether it's ephemeral.
    fn reply_builder(self, mut reply: CreateReply) -> CreateReply {
        reply.ephemeral = reply.ephemeral.or(Some(self.command.ephemeral));
        reply
    }

    pub async fn say(self, text: impl Into<String>) -> Result<ReplyHandle<'a>, serenity::Error> {
        self.send(CreateReply::default().content(text)).await
    }

    /// Replies to the command; after the first reply (or a defer), sends a follow-up.
    pub async fn send(self, reply: CreateReply) -> Result<ReplyHandle<'a>, serenity::Error> {
        let reply = self.reply_builder(reply);
        let followup = if self.has_sent_initial_response.load(Ordering::SeqCst) {
            let builder = reply.to_followup(serenity::CreateInteractionResponseFollowup::new());
            Some(Box::new(self.interaction.create_followup(self.serenity_context, builder).await?))
        } else {
            let builder = reply.to_initial_response(serenity::CreateInteractionResponseMessage::new());
            self.interaction
                .create_response(self.serenity_context, serenity::CreateInteractionResponse::Message(builder))
                .await?;
            self.has_sent_initial_response.store(true, Ordering::SeqCst);
            None
        };
        Ok(ReplyHandle { http: &self.serenity_context.http, interaction: self.interaction, followup })
    }

    async fn defer_response(self, ephemeral: bool) -> Result<(), serenity::Error> {
        if !self.has_sent_initial_response.load(Ordering::SeqCst) {
            let response = serenity::CreateInteractionResponse::Defer(serenity::CreateInteractionResponseMessage::new().ephemeral(ephemeral));
            self.interaction.create_response(self.serenity_context, response).await?;
            self.has_sent_initial_response.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Shows "thinking..." to everyone, for commands that take a while to reply.
    pub async fn defer(self) -> Result<(), serenity::Error> {
        self.defer_response(false).await
    }

    /// Shows "thinking..." only to the person who ran it.
    pub async fn defer_ephemeral(self) -> Result<(), serenity::Error> {
        self.defer_response(true).await
    }

    /// Marks the interaction as answered, after the command answered it directly (with a
    /// modal, say), so later replies are sent as follow-ups.
    pub fn mark_responded(self) {
        self.has_sent_initial_response.store(true, Ordering::SeqCst);
    }
}

impl serenity::CacheHttp for Context<'_> {
    fn http(&self) -> &serenity::Http {
        &self.serenity_context.http
    }

    fn cache(&self) -> Option<&Arc<serenity::Cache>> {
        Some(&self.serenity_context.cache)
    }
}

impl AsRef<serenity::Http> for Context<'_> {
    fn as_ref(&self) -> &serenity::Http {
        &self.serenity_context.http
    }
}

impl AsRef<serenity::Cache> for Context<'_> {
    fn as_ref(&self) -> &serenity::Cache {
        &self.serenity_context.cache
    }
}

impl AsRef<serenity::ShardMessenger> for Context<'_> {
    fn as_ref(&self) -> &serenity::ShardMessenger {
        &self.serenity_context.shard
    }
}

impl AsRef<serenity::Context> for Context<'_> {
    fn as_ref(&self) -> &serenity::Context {
        self.serenity_context
    }
}
