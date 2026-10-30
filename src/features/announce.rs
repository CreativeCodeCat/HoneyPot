//! Announcements cog: `/announce message` posts an embed, pinging a role, in the channel
//! set with `/server-setup announcements`, whichever channel the command is used in.

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::{Context, Error};

const ANNOUNCEMENTS_KEY: &str = "channel.announcements";
const COLOUR: u32 = 0x9b59b6;

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Make announcements in the server announcements channel
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "Announcements",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("message", "help"),
    subcommand_required
)]
pub async fn announce(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Announce a message to the announcements channel
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn message(
    ctx: Context<'_>,
    #[description = "The role to ping"] role: serenity::Role,
    #[description = "The title of the announcement"]
    #[max_length = 256]
    title: String,
    #[description = "What you want to announce (use \\n for new lines)"]
    #[max_length = 4000]
    information: String,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let Some(channel_id) = ctx.data().server_setup.lock().unwrap().get(guild_id.get(), ANNOUNCEMENTS_KEY)? else {
        return say_private(ctx, "❌ No announcements channel is set. An admin can set one with `/server-setup announcements`.").await;
    };
    let channel = serenity::ChannelId::new(channel_id);

    // @everyone's role ID is the guild ID, and it has to be pinged by name
    let ping = if role.id.get() == guild_id.get() { "@everyone".to_string() } else { format!("<@&{}>", role.id) };
    let author = ctx.author();
    let embed = serenity::CreateEmbed::new()
        .colour(COLOUR)
        .title(title)
        .description(information.replace("\\n", "\n"))
        .footer(serenity::CreateEmbedFooter::new(format!("Announced by {}", author.name)).icon_url(author.face()))
        .timestamp(serenity::Timestamp::now());
    let message = serenity::CreateMessage::new()
        .content(ping)
        .embed(embed)
        .allowed_mentions(serenity::CreateAllowedMentions::new().roles([role.id]).everyone(role.id.get() == guild_id.get()));

    let posted = match channel.send_message(ctx, message).await {
        Ok(posted) => posted,
        Err(e) => return say_private(ctx, format!("❌ Couldn't post in <#{channel_id}>: {e}")).await,
    };
    // In an announcement channel, publish it so servers following the channel get it too
    let is_news = ctx.guild().is_some_and(|g| g.channels.get(&channel).is_some_and(|c| c.kind == serenity::ChannelType::News));
    if is_news {
        if let Err(e) = posted.crosspost(ctx).await {
            return say_private(ctx, format!("⚠️ Posted in <#{channel_id}>, but couldn't publish it to following servers: {e}")).await;
        }
    }
    say_private(ctx, format!("✅ Your announcement has been posted in <#{channel_id}>.")).await
}

/// Get help with the command
#[command(slash_command, guild_only, ephemeral)]
async fn help(ctx: Context<'_>) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(COLOUR).title("Announce Help").description(
        "> `/announce message <role> <title> <information>` • Post an announcement in the \
         announcements channel, pinging `role`. Use `\\n` for new lines.\n\
         > `/announce help` • Show this message.\n\n\
         The channel is set with `/server-setup announcements`.",
    );
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
