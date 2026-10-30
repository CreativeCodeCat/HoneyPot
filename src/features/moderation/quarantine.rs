//! Quarantine cog: isolates suspicious members until staff review them. Their roles are taken
//! away (and saved) and they get a Quarantine role that can only see one quarantine channel,
//! where they can talk to staff. A review card with Release, Kick and Ban buttons is posted
//! for staff. Members are quarantined by the honeypot or raid protection (when set to), or
//! on joining with an account younger than the set age. Leaving and rejoining doesn't get
//! anyone out.

use ::serenity::all as serenity;
use chrono::Utc;

use crate::core::framework::{command, CreateReply};
use crate::features::{Context, Data, Error};
use crate::log;
use crate::shared::pagination::{chunk_lines, paginate};

pub use crate::database::repositories::quarantine::*;

pub const COG_NAME: &str = "quarantine";

const ORANGE: u32 = 0xe67e22;
const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;
const GREY: u32 = 0x95a5a6;
/// Review card buttons are "quarantine:<action>:<user id>"
const BUTTON_PREFIX: &str = "quarantine:";
/// How long a member is timed out instead when quarantine isn't set up
const FALLBACK_TIMEOUT_HOURS: i64 = 24;

type P = serenity::Permissions;

/// The Quarantine role's overwrite on every other channel
fn hidden(role: serenity::RoleId) -> serenity::PermissionOverwrite {
    serenity::PermissionOverwrite {
        allow: P::empty(),
        deny: P::VIEW_CHANNEL | P::SEND_MESSAGES | P::SEND_MESSAGES_IN_THREADS | P::CONNECT,
        kind: serenity::PermissionOverwriteType::Role(role),
    }
}

/// Roles the bot can take away: not @everyone, not managed by an integration, below its own.
/// Returns (removable, kept).
fn split_roles(ctx: &serenity::Context, guild_id: serenity::GuildId, roles: &[serenity::RoleId]) -> (Vec<serenity::RoleId>, Vec<serenity::RoleId>) {
    let Some(guild) = ctx.cache.guild(guild_id) else { return (vec![], roles.to_vec()) };
    let bot_top = guild.members.get(&ctx.cache.current_user().id).and_then(|m| guild.member_highest_role(m)).map_or(0, |r| r.position);
    roles.iter().partition(|id| guild.roles.get(id).is_some_and(|r| !r.managed && r.position < bot_top && r.id != guild_id.everyone_role()))
}

fn buttons(user_id: u64, disabled: bool) -> Vec<serenity::CreateActionRow> {
    let button = |action: &str, label: &str, style| {
        serenity::CreateButton::new(format!("{BUTTON_PREFIX}{action}:{user_id}")).label(label).style(style).disabled(disabled)
    };
    vec![serenity::CreateActionRow::Buttons(vec![
        button("release", "Release", serenity::ButtonStyle::Success),
        button("kick", "Kick", serenity::ButtonStyle::Secondary),
        button("ban", "Ban", serenity::ButtonStyle::Danger),
    ])]
}

fn review_embed(user: &serenity::User, record: &Quarantined, kept: &[serenity::RoleId], suspicion: Option<&super::suspicious_accounts::Score>) -> serenity::CreateEmbed {
    let created = user.id.created_at().unix_timestamp();
    let roles = if record.saved_roles.is_empty() { "None".to_string() } else { record.saved_roles.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" ") };
    let mut embed = serenity::CreateEmbed::new()
        .colour(ORANGE)
        .title("🔒 Member quarantined")
        .thumbnail(user.face())
        .field("Member", format!("<@{}> {}\nID `{}`", user.id, user.tag(), user.id), true)
        .field("Account created", format!("<t:{created}:f>\n(<t:{created}:R>)"), true)
        .field("Quarantined by", &record.source, true)
        .field("Reason", &record.reason, false)
        .field("Roles taken (given back on release)", roles, false)
        .timestamp(serenity::Timestamp::now());
    if let Some(score) = suspicion {
        let reasons = crate::features::truncate(&super::suspicious_accounts::reasons_text(score), 900);
        embed = embed.field("Suspicion score", format!("{}\n{reasons}", super::suspicious_accounts::label(score)), false);
    }
    if !kept.is_empty() {
        let kept: Vec<String> = kept.iter().map(|r| format!("<@&{r}>")).collect();
        embed = embed.field("⚠️ Roles the bot couldn't take", format!("{}\nIf these let them see channels, quarantine won't hide them.", kept.join(" ")), false);
    }
    embed
}

/// Quarantines a member. Returns false if quarantine isn't turned on and set up here.
pub async fn isolate(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user_id: serenity::UserId, reason: &str, source: &str) -> Result<bool, Error> {
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return Ok(false);
    }
    let settings = data.quarantine.lock().unwrap().settings(guild_id.get())?;
    let Some(role_id) = settings.role_id.map(serenity::RoleId::new) else { return Ok(false) };
    if data.quarantine.lock().unwrap().get(guild_id.get(), user_id.get())?.is_some() {
        return Ok(true);
    }
    let member = guild_id.member(ctx, user_id).await?;
    let (removable, kept) = split_roles(ctx, guild_id, &member.roles);
    let mut roles = kept.clone();
    roles.push(role_id);
    let audit_reason = format!("Quarantined: {reason}");
    let edit = serenity::EditMember::new().roles(roles).audit_log_reason(&audit_reason);
    guild_id.edit_member(ctx, user_id, edit).await?;

    let record = Quarantined {
        guild_id: guild_id.get(),
        user_id: user_id.get(),
        reason: reason.to_string(),
        source: source.to_string(),
        saved_roles: removable.iter().map(|r| r.get()).collect(),
        quarantined_at: Utc::now().timestamp(),
        review_message_id: None,
    };
    data.quarantine.lock().unwrap().add(&record)?;
    log::info(format!("Quarantined {} in guild {guild_id} ({source}: {reason})", member.user.name));

    if let Some(channel) = settings.review_channel_id.map(serenity::ChannelId::new) {
        let suspicion = super::suspicious_accounts::score_for(ctx, data, guild_id, user_id).await;
        let message = serenity::CreateMessage::new().embed(review_embed(&member.user, &record, &kept, suspicion.as_ref())).components(buttons(user_id.get(), false));
        match channel.send_message(ctx, message).await {
            Ok(posted) => data.quarantine.lock().unwrap().set_review_message(guild_id.get(), user_id.get(), posted.id.get())?,
            Err(e) => log::warning(format!("Couldn't post a quarantine review card in guild {guild_id}: {e}")),
        }
    }
    if let Some(channel) = settings.channel_id.map(serenity::ChannelId::new) {
        let text = format!("<@{user_id}> You've been placed in quarantine while staff review your account. You can talk to staff here; please be patient.");
        let message = serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new().users([user_id]));
        if let Err(e) = channel.send_message(ctx, message).await {
            log::warning(format!("Couldn't greet a quarantined member in guild {guild_id}: {e}"));
        }
    }
    Ok(true)
}

/// Quarantines a member, or times them out for a day if quarantine isn't set up here (or
/// fails), so they're never let through untouched.
pub async fn isolate_or_timeout(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user_id: serenity::UserId, reason: &str, source: &str) -> Result<(), Error> {
    match isolate(ctx, data, guild_id, user_id, reason, source).await {
        Ok(true) => return Ok(()),
        Ok(false) => log::warning(format!("Quarantine isn't set up in guild {guild_id}; timing {user_id} out instead")),
        Err(e) => log::warning(format!("Couldn't quarantine {user_id} in guild {guild_id} ({e}); timing them out instead")),
    }
    let until = Utc::now() + chrono::Duration::hours(FALLBACK_TIMEOUT_HOURS);
    let edit = serenity::EditMember::new().disable_communication_until_datetime(until.into()).audit_log_reason(reason);
    guild_id.edit_member(ctx, user_id, edit).await?;
    Ok(())
}

/// Disables a review card's buttons and notes what happened.
async fn close_card(ctx: &serenity::Context, data: &Data, record: &Quarantined, outcome: &str, colour: u32) {
    let settings = match data.quarantine.lock().unwrap().settings(record.guild_id) {
        Ok(settings) => settings,
        Err(_) => return,
    };
    let (Some(channel), Some(message_id)) = (settings.review_channel_id, record.review_message_id) else { return };
    let channel = serenity::ChannelId::new(channel);
    let Ok(message) = channel.message(ctx, message_id).await else { return };
    let Some(embed) = message.embeds.first().cloned() else { return };
    let embed = serenity::CreateEmbed::from(embed).colour(colour).field("Outcome", outcome, false);
    let edit = serenity::EditMessage::new().embed(embed).components(buttons(record.user_id, true));
    if let Err(e) = channel.edit_message(ctx, message_id, edit).await {
        log::warning(format!("Couldn't update a quarantine review card: {e}"));
    }
}

/// Takes a member out of quarantine and gives their roles back. Returns false if they weren't
/// quarantined.
pub async fn release(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user_id: serenity::UserId, by: &str) -> Result<bool, Error> {
    let (record, settings) = {
        let store = data.quarantine.lock().unwrap();
        (store.remove(guild_id.get(), user_id.get())?, store.settings(guild_id.get())?)
    };
    let Some(record) = record else { return Ok(false) };
    // They may have left; then there's nothing to give back
    if let Ok(member) = guild_id.member(ctx, user_id).await {
        let quarantine_role = settings.role_id.map(serenity::RoleId::new);
        let saved: Vec<serenity::RoleId> = record.saved_roles.iter().map(|r| serenity::RoleId::new(*r)).collect();
        let (giveable, _) = split_roles(ctx, guild_id, &saved);
        let mut roles: Vec<serenity::RoleId> = member.roles.iter().copied().filter(|r| Some(*r) != quarantine_role).collect();
        roles.extend(giveable.into_iter().filter(|r| !member.roles.contains(r)));
        let audit_reason = format!("Released from quarantine by {by}");
        let edit = serenity::EditMember::new().roles(roles).audit_log_reason(&audit_reason);
        guild_id.edit_member(ctx, user_id, edit).await?;
    }
    close_card(ctx, data, &record, &format!("✅ Released by {by} <t:{}:R>", Utc::now().timestamp()), GREEN).await;
    log::info(format!("Released {user_id} from quarantine in guild {guild_id} ({by})"));
    Ok(true)
}

/// Keeps quarantined members in quarantine if they rejoin, and quarantines new accounts.
pub async fn on_member_join(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    let guild_id = member.guild_id;
    if member.user.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let (settings, existing) = {
        let store = data.quarantine.lock().unwrap();
        match store.settings(guild_id.get()).and_then(|s| Ok((s, store.get(guild_id.get(), member.user.id.get())?))) {
            Ok(found) => found,
            Err(e) => return log::error(format!("Error reading quarantine for guild {guild_id}: {e}")),
        }
    };
    let Some(role_id) = settings.role_id.map(serenity::RoleId::new) else { return };
    if existing.is_some() {
        if let Err(e) = member.add_role(ctx, role_id).await {
            log::warning(format!("Couldn't put {} back in quarantine in guild {guild_id}: {e}", member.user.name));
        }
        return;
    }
    let Some(min_days) = settings.new_account_days.filter(|d| *d > 0) else { return };
    let age_days = (Utc::now().timestamp() - member.user.id.created_at().unix_timestamp()) / 86_400;
    if age_days < min_days as i64 {
        let reason = format!("Account is {age_days} day{} old (the minimum is {min_days})", if age_days == 1 { "" } else { "s" });
        if let Err(e) = isolate(ctx, data, guild_id, member.user.id, &reason, "New account").await {
            log::error(format!("Couldn't quarantine new account {} in guild {guild_id}: {e}", member.user.name));
        }
    }
}

/// Hides channels made after setup from the Quarantine role.
pub async fn on_channel_create(ctx: &serenity::Context, data: &Data, channel: &serenity::GuildChannel) {
    if !data.is_cog_enabled(channel.guild_id.get(), COG_NAME) {
        return;
    }
    let Ok(settings) = data.quarantine.lock().unwrap().settings(channel.guild_id.get()) else { return };
    let Some(role_id) = settings.role_id.map(serenity::RoleId::new) else { return };
    if Some(channel.id.get()) == settings.channel_id {
        return;
    }
    if let Err(e) = channel.id.create_permission(ctx, hidden(role_id)).await {
        log::warning(format!("Couldn't hide new channel {} from quarantine: {e}", channel.id));
    }
}

/// Handles the Release, Kick and Ban buttons on review cards.
pub async fn on_interaction(ctx: &serenity::Context, data: &Data, interaction: &serenity::Interaction) {
    let serenity::Interaction::Component(press) = interaction else { return };
    let Some(rest) = press.data.custom_id.strip_prefix(BUTTON_PREFIX) else { return };
    let Some((action, user_id)) = rest.split_once(':').and_then(|(a, id)| Some((a, serenity::UserId::new(id.parse().ok()?)))) else { return };
    let Some(guild_id) = press.guild_id else { return };
    let perms = press.member.as_ref().and_then(|m| m.permissions).unwrap_or_default();
    let allowed = match action {
        "release" => perms.moderate_members(),
        "kick" => perms.kick_members(),
        "ban" => perms.ban_members(),
        _ => return,
    };
    let reply = |text: String| {
        serenity::CreateInteractionResponse::Message(serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if !allowed {
        let _ = press.create_response(ctx, reply(format!("❌ You need permission to {action} members."))).await;
        return;
    }
    let by = press.user.name.clone();
    let result = match action {
        "release" => release(ctx, data, guild_id, user_id, &by).await.map(|done| if done { "✅ Released.".to_string() } else { "They're no longer in quarantine.".to_string() }),
        kick_or_ban => {
            let record = match data.quarantine.lock().unwrap().remove(guild_id.get(), user_id.get()) {
                Ok(record) => record,
                Err(e) => return log::error(format!("Error reading quarantine: {e}")),
            };
            let reason = format!("Quarantine review by {by}");
            let done = if kick_or_ban == "kick" {
                guild_id.kick_with_reason(ctx, user_id, &reason).await
            } else {
                guild_id.ban_with_reason(ctx, user_id, 0, &reason).await
            };
            match done {
                Ok(()) => {
                    let past = if kick_or_ban == "kick" { "Kicked" } else { "Banned" };
                    if let Some(record) = record {
                        close_card(ctx, data, &record, &format!("{} {past} by {by} <t:{}:R>", if kick_or_ban == "kick" { "👢" } else { "🔨" }, Utc::now().timestamp()), RED).await;
                    }
                    Ok(format!("✅ {past}."))
                }
                Err(e) => {
                    // Still in quarantine if it didn't work
                    if let Some(record) = record {
                        let _ = data.quarantine.lock().unwrap().add(&record);
                    }
                    Err(e.into())
                }
            }
        }
    };
    let text = result.unwrap_or_else(|e: Error| format!("❌ Couldn't do that: {e}"));
    if let Err(e) = press.create_response(ctx, reply(text)).await {
        log::warning(format!("Couldn't answer a quarantine button: {e}"));
    }
}

/// Isolates suspicious members until staff review them
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "Quarantine",
    required_permissions = "MODERATE_MEMBERS",
    default_member_permissions = "MODERATE_MEMBERS",
    subcommands("setup", "list", "release_member"),
    subcommand_required
)]
pub async fn quarantine(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Create the Quarantine role and channel, and choose where review cards go
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD | MANAGE_ROLES | MANAGE_CHANNELS")]
async fn setup(
    ctx: Context<'_>,
    #[description = "Where staff review quarantined members"]
    #[channel_types("Text")]
    review_channel: serenity::GuildChannel,
    #[description = "Quarantine accounts younger than this many days when they join (0 = off)"]
    #[min = 0]
    #[max = 365]
    new_account_days: Option<u32>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap();
    let mut settings = ctx.data().quarantine.lock().unwrap().settings(guild_id.get())?;
    let (existing_role, existing_channel) = {
        let guild = ctx.guild().ok_or("The server isn't loaded yet; try again in a moment.")?;
        (
            settings.role_id.map(serenity::RoleId::new).filter(|r| guild.roles.contains_key(r)),
            settings.channel_id.map(serenity::ChannelId::new).filter(|c| guild.channels.contains_key(c)),
        )
    };

    let role_id = match existing_role {
        Some(role) => role,
        None => {
            let role = serenity::EditRole::new().name("Quarantined").colour(GREY).permissions(P::empty()).audit_log_reason("Quarantine setup");
            guild_id.create_role(ctx, role).await?.id
        }
    };
    let bot_id = ctx.cache().current_user().id;
    let channel_id = match existing_channel {
        Some(channel) => channel,
        None => {
            let talk = P::VIEW_CHANNEL | P::SEND_MESSAGES | P::READ_MESSAGE_HISTORY;
            let overwrites = vec![
                serenity::PermissionOverwrite { allow: P::empty(), deny: P::VIEW_CHANNEL, kind: serenity::PermissionOverwriteType::Role(guild_id.everyone_role()) },
                serenity::PermissionOverwrite { allow: talk, deny: P::empty(), kind: serenity::PermissionOverwriteType::Role(role_id) },
                serenity::PermissionOverwrite { allow: talk | P::MANAGE_MESSAGES, deny: P::empty(), kind: serenity::PermissionOverwriteType::Member(bot_id) },
            ];
            let channel = serenity::CreateChannel::new("quarantine")
                .kind(serenity::ChannelType::Text)
                .topic("Quarantined members wait here while staff review them.")
                .permissions(overwrites)
                .audit_log_reason("Quarantine setup");
            guild_id.create_channel(ctx, channel).await?.id
        }
    };

    // Hide every other channel from the role
    let others: Vec<serenity::ChannelId> = ctx.guild().map(|g| g.channels.keys().copied().filter(|c| *c != channel_id).collect()).unwrap_or_default();
    let mut failed = 0;
    for channel in &others {
        if channel.create_permission(ctx, hidden(role_id)).await.is_err() {
            failed += 1;
        }
    }

    settings.role_id = Some(role_id.get());
    settings.channel_id = Some(channel_id.get());
    settings.review_channel_id = Some(review_channel.id.get());
    if let Some(days) = new_account_days {
        settings.new_account_days = (days > 0).then_some(days);
    }
    ctx.data().quarantine.lock().unwrap().set_settings(guild_id.get(), &settings)?;

    let new_accounts = match settings.new_account_days {
        Some(days) => format!("Accounts younger than **{days} days** are quarantined when they join."),
        None => "New accounts aren't quarantined on join (set `new_account_days` to turn it on).".to_string(),
    };
    let warning = if failed > 0 { format!("\n⚠️ Couldn't hide {failed} channels from the role; check the bot's permissions there.") } else { String::new() };
    let text = format!(
        "✅ Quarantine is set up.\nRole: <@&{role_id}> · Channel: <#{channel_id}> · Review cards: <#{}>\nHid {} channels from the role.{warning}\n\n{new_accounts}\n\
         To quarantine honeypot catches, set a honeypot's punishment to `quarantine`; for raids, use `/raid setup joiners: Quarantine`.",
        review_channel.id,
        others.len() - failed
    );
    ctx.say(text).await?;
    Ok(())
}

/// See who's in quarantine
#[command(slash_command, guild_only, ephemeral, required_permissions = "MODERATE_MEMBERS")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let members = ctx.data().quarantine.lock().unwrap().list(ctx.guild_id().unwrap().get())?;
    if members.is_empty() {
        ctx.say("Nobody is in quarantine.").await?;
        return Ok(());
    }
    let lines: Vec<String> = members
        .iter()
        .map(|q| format!("<@{}> since <t:{}:R> · **{}**: {}", q.user_id, q.quarantined_at, q.source, q.reason))
        .collect();
    let title = format!("In quarantine ({})", members.len());
    let pages = chunk_lines(&lines, 10).into_iter().map(|page| serenity::CreateEmbed::new().title(&title).colour(ORANGE).description(page)).collect();
    paginate(ctx, pages).await
}

/// Release a member from quarantine and give their roles back
#[command(slash_command, guild_only, ephemeral, rename = "release", required_permissions = "MODERATE_MEMBERS")]
async fn release_member(ctx: Context<'_>, #[description = "The member to release"] user: serenity::User) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let released = release(ctx.serenity_context(), ctx.data(), ctx.guild_id().unwrap(), user.id, &ctx.author().name).await?;
    let text = if released { format!("✅ <@{}> has been released and their roles given back.", user.id) } else { format!("<@{}> isn't in quarantine.", user.id) };
    ctx.send(CreateReply::default().content(text)).await?;
    Ok(())
}
