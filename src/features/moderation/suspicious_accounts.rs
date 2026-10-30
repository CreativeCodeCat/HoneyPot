//! SuspiciousAccounts cog: scores members from 0 to 100 on how suspicious they look, from
//! their account and profile, what they do on the server, what their messages contain, and
//! their history (honeypot catches here and elsewhere, incidents, joining during a raid).
//! Staff are alerted when someone reaches the threshold, and can have them quarantined.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use ::serenity::all as serenity;
use chrono::Utc;
use regex::Regex;

use crate::core::framework::{command, CreateReply};
use crate::features::{logging, Context, Data, Error};
use crate::log;
use crate::shared::pagination::{chunk_lines, paginate};

pub use crate::database::repositories::suspicious_accounts::*;

pub const COG_NAME: &str = "suspiciousaccounts";

const ORANGE: u32 = 0xe67e22;
const RED: u32 = 0xed4245;
const GREEN: u32 = 0x57f287;
/// This many messages within `BURST_WINDOW` is a burst
const BURST_MESSAGES: usize = 5;
const BURST_WINDOW: Duration = Duration::from_secs(10);
/// The same message this many times within `DUPLICATE_WINDOW` is repeating
const DUPLICATE_MESSAGES: usize = 3;
const DUPLICATE_WINDOW: Duration = Duration::from_secs(60);
/// Score messages this early on even when nothing stands out, as new members are watched closest
const WATCH_FIRST_MESSAGES: u32 = 20;

/// One reason a score went up (or down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reason {
    pub points: i32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Score {
    /// 0 to 100
    pub total: u32,
    pub reasons: Vec<Reason>,
}

/// Everything a score is worked out from.
#[derive(Clone, Debug, Default)]
pub struct Signals {
    pub account_age_days: i64,
    pub default_avatar: bool,
    pub no_display_name: bool,
    pub username: String,
    /// Hours since they joined, if they're on the server
    pub hours_on_server: Option<i64>,
    /// Seconds from joining to their first message
    pub first_message_after_secs: Option<i64>,
    pub activity: Activity,
    pub honeypot_here: bool,
    pub honeypot_other_servers: usize,
    pub incidents: usize,
}

/// Usernames that look made by a script: five or more digits (four is often just a year).
fn looks_generated(name: &str) -> bool {
    name.chars().filter(char::is_ascii_digit).count() >= 5
}

/// A group of reasons, capped at `cap` points.
fn group(reasons: Vec<Reason>, cap: i32) -> Vec<Reason> {
    let total: i32 = reasons.iter().map(|r| r.points).filter(|p| *p > 0).sum();
    if total <= cap {
        return reasons;
    }
    // Trim the last reasons' points so the group adds up to the cap
    let mut left = cap;
    reasons
        .into_iter()
        .map(|r| {
            if r.points <= 0 {
                return r;
            }
            let points = r.points.min(left);
            left -= points;
            Reason { points, ..r }
        })
        .filter(|r| r.points != 0)
        .collect()
}

pub fn score(s: &Signals) -> Score {
    let reason = |points: i32, text: String| Reason { points, text };
    let mut account = vec![];
    match s.account_age_days {
        d if d < 1 => account.push(reason(25, "Account is less than a day old".into())),
        d if d < 7 => account.push(reason(15, format!("Account is {d} days old"))),
        d if d < 30 => account.push(reason(5, format!("Account is {d} days old"))),
        _ => {}
    }
    if s.default_avatar {
        account.push(reason(10, "Default avatar".into()));
    }
    if s.no_display_name {
        account.push(reason(5, "No display name".into()));
    }
    if looks_generated(&s.username) {
        account.push(reason(10, format!("Username `{}` looks auto-generated", s.username)));
    }

    let a = &s.activity;
    let mut activity = vec![];
    if a.bursts > 0 {
        activity.push(reason(15, format!("Posted in bursts ({}×)", a.bursts)));
    }
    if a.duplicates > 0 {
        activity.push(reason(15, format!("Repeated the same message ({}×)", a.duplicates)));
    }
    if s.first_message_after_secs.is_some_and(|secs| secs < 60) {
        activity.push(reason(5, "Posted within a minute of joining".into()));
    }
    match s.hours_on_server {
        Some(hours) if hours < 24 => activity.push(reason(5, "Joined less than a day ago".into())),
        Some(hours) if hours >= 30 * 24 && a.messages >= 100 => {
            activity.push(reason(-15, format!("Active member: {} days here, {} messages", hours / 24, a.messages)))
        }
        _ => {}
    }

    let mut content = vec![];
    if a.invites > 0 {
        content.push(reason(if a.invites >= 3 { 25 } else { 15 }, format!("Posted Discord invites ({}×)", a.invites)));
    }
    if a.mass_mentions > 0 {
        content.push(reason(15, format!("Mass mentions or @everyone ({}×)", a.mass_mentions)));
    }
    if a.links > 0 && a.messages < 10 {
        content.push(reason(5, format!("Posted links while new ({}×)", a.links)));
    }
    if a.noisy >= 3 {
        content.push(reason(5, format!("Shouting or repeated characters ({}×)", a.noisy)));
    }

    let mut history = vec![];
    if s.honeypot_here {
        history.push(reason(30, "Caught by a honeypot here".into()));
    }
    if s.honeypot_other_servers > 0 {
        history.push(reason(20, format!("Caught by honeypots on {} other server(s)", s.honeypot_other_servers)));
    }
    if s.incidents > 0 {
        history.push(reason((10 * s.incidents as i32).min(20), format!("{} past incident(s) here", s.incidents)));
    }
    if a.joined_during_raid {
        history.push(reason(15, "Joined during a raid".into()));
    }

    let reasons: Vec<Reason> = [group(account, 40), group(activity, 30), group(content, 30), group(history, 50)].concat();
    let total = reasons.iter().map(|r| r.points).sum::<i32>().clamp(0, 100) as u32;
    Score { total, reasons }
}

/// What a message's text gives away.
pub fn analyse(content: &str) -> MessageFlags {
    static INVITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(discord\.gg|discord(app)?\.com/invite)/\S+").unwrap());
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)https?://\S+").unwrap());
    static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>").unwrap());
    let letters: Vec<char> = content.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    let shouting = letters.len() >= 10 && upper * 10 >= letters.len() * 7;
    let chars: Vec<char> = content.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let repeated = chars.windows(6).any(|w| w.iter().all(|c| *c == w[0]));
    MessageFlags {
        invite: INVITE.is_match(content),
        mass_mention: MENTION.find_iter(content).count() >= 5 || content.contains("@everyone") || content.contains("@here"),
        link: LINK.is_match(content),
        noisy: shouting || repeated,
        burst: false,
        duplicate: false,
    }
}

/// Each member's recent messages: when, and a fingerprint of the text.
static RECENT: LazyLock<Mutex<HashMap<(u64, u64), VecDeque<(Instant, String)>>>> = LazyLock::new(Default::default);

/// Whether this message makes a burst, or repeats one just sent.
fn burst_and_duplicate(guild_id: u64, user_id: u64, content: &str) -> (bool, bool) {
    let now = Instant::now();
    let text = content.trim().to_lowercase();
    let mut recent = RECENT.lock().unwrap();
    recent.retain(|_, messages| messages.back().is_some_and(|(at, _)| now.duration_since(*at) < DUPLICATE_WINDOW));
    let messages = recent.entry((guild_id, user_id)).or_default();
    messages.push_back((now, text.clone()));
    while messages.front().is_some_and(|(at, _)| now.duration_since(*at) > DUPLICATE_WINDOW) {
        messages.pop_front();
    }
    let in_burst = messages.iter().filter(|(at, _)| now.duration_since(*at) <= BURST_WINDOW).count();
    let same = if text.is_empty() { 0 } else { messages.iter().filter(|(_, t)| *t == text).count() };
    // Counted once, as the threshold is reached, not for every message after
    (in_burst == BURST_MESSAGES, same == DUPLICATE_MESSAGES)
}

/// Works out a member's score. `joined_at` is when they joined, if they're on the server.
pub fn score_user(data: &Data, guild_id: serenity::GuildId, user: &serenity::User, joined_at: Option<serenity::Timestamp>) -> Result<Score, Error> {
    let now = Utc::now().timestamp();
    let activity = data.suspicious_accounts.lock().unwrap().activity(guild_id.get(), user.id.get())?;
    let honeypot_here = !data.honeypot.lock().unwrap().activity_logs(guild_id.get(), Some(user.id.get()), Some(1))?.is_empty();
    let honeypot_other_servers =
        data.honeypot.lock().unwrap().offender(user.id.get())?.map_or(0, |o| o.servers.iter().filter(|s| **s != guild_id.get()).count());
    let incidents = data.incidents.lock().unwrap().list(guild_id.get(), Some(user.id.get()))?.len();
    let joined = joined_at.map(|t| t.unix_timestamp());
    let signals = Signals {
        account_age_days: (now - user.id.created_at().unix_timestamp()) / 86_400,
        default_avatar: user.avatar.is_none(),
        no_display_name: user.global_name.is_none(),
        username: user.name.clone(),
        hours_on_server: joined.map(|j| (now - j) / 3600),
        first_message_after_secs: joined.zip(activity.first_message_at).map(|(j, first)| first - j),
        activity,
        honeypot_here,
        honeypot_other_servers,
        incidents,
    };
    Ok(score(&signals))
}

/// A member's score, looking them up first. `None` if the cog is off here or they can't be found.
pub async fn score_for(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user_id: serenity::UserId) -> Option<Score> {
    if !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return None;
    }
    let member = guild_id.member(ctx, user_id).await.ok();
    let user = match &member {
        Some(member) => member.user.clone(),
        None => user_id.to_user(ctx).await.ok()?,
    };
    score_user(data, guild_id, &user, member.and_then(|m| m.joined_at)).ok()
}

/// "62/100 🟥" style label.
pub fn label(score: &Score) -> String {
    let mark = match score.total {
        0..=29 => "🟩",
        30..=59 => "🟨",
        _ => "🟥",
    };
    format!("{}/100 {mark}", score.total)
}

/// The reasons, one per line, like "+25 Account is less than a day old".
pub fn reasons_text(score: &Score) -> String {
    if score.reasons.is_empty() {
        return "Nothing suspicious".to_string();
    }
    score.reasons.iter().map(|r| format!("`{:+}` {}", r.points, r.text)).collect::<Vec<_>>().join("\n")
}

/// Alerts staff (once per member) when a score reaches the threshold, and quarantines them
/// if that's turned on.
async fn check_threshold(ctx: &serenity::Context, data: &Data, guild_id: serenity::GuildId, user: &serenity::User, joined_at: Option<serenity::Timestamp>) {
    let (settings, alerted) = {
        let store = data.suspicious_accounts.lock().unwrap();
        match store.settings(guild_id.get()).and_then(|s| Ok((s, store.activity(guild_id.get(), user.id.get())?.alerted))) {
            Ok(found) => found,
            Err(e) => return log::error(format!("Error reading suspicious account settings for guild {guild_id}: {e}")),
        }
    };
    if alerted {
        return;
    }
    let score = match score_user(data, guild_id, user, joined_at) {
        Ok(score) => score,
        Err(e) => return log::error(format!("Error scoring {} in guild {guild_id}: {e}", user.name)),
    };
    if score.total < settings.threshold {
        return;
    }
    if let Err(e) = data.suspicious_accounts.lock().unwrap().set_alerted(guild_id.get(), user.id.get()) {
        log::error(format!("Error saving a suspicious account alert: {e}"));
    }
    log::warning(format!("Suspicious account in guild {guild_id}: {} scored {}", user.name, score.total));

    let quarantined = settings.auto_quarantine
        && super::quarantine::isolate_or_timeout(ctx, data, guild_id, user.id, &format!("Suspicion score {}/100", score.total), "Suspicious accounts")
            .await
            .map_err(|e| log::warning(format!("Couldn't quarantine suspicious account {}: {e}", user.name)))
            .is_ok();
    let channel = settings
        .alert_channel
        .or_else(|| data.quarantine.lock().unwrap().settings(guild_id.get()).ok().and_then(|q| q.review_channel_id))
        .map(serenity::ChannelId::new)
        .or_else(|| logging::log_channel(data, guild_id, "joinleave"))
        .or_else(|| ctx.cache.guild(guild_id).and_then(|g| g.system_channel_id));
    let Some(channel) = channel else { return };
    let mut embed = serenity::CreateEmbed::new()
        .colour(RED)
        .title("⚠️ Suspicious account")
        .thumbnail(user.face())
        .description(format!("<@{}> {} reached the threshold ({}).", user.id, user.tag(), settings.threshold))
        .field("Score", label(&score), true)
        .field("ID", format!("`{}`", user.id), true)
        .field("Why", reasons_text(&score), false)
        .timestamp(serenity::Timestamp::now());
    if quarantined {
        embed = embed.field("Action", "🔒 Quarantined (or timed out if quarantine isn't set up)", false);
    }
    let message = serenity::CreateMessage::new().embed(embed).allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Err(e) = channel.send_message(ctx, message).await {
        log::warning(format!("Couldn't post a suspicious account alert in guild {guild_id}: {e}"));
    }
}

pub async fn on_member_join(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    let guild_id = member.guild_id;
    if member.user.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let during_raid = data.raid_protection.lock().unwrap().raid(guild_id.get()).ok().flatten().is_some();
    if let Err(e) = data.suspicious_accounts.lock().unwrap().joined(guild_id.get(), member.user.id.get(), during_raid) {
        return log::error(format!("Error recording a join for suspicious accounts: {e}"));
    }
    check_threshold(ctx, data, guild_id, &member.user, member.joined_at).await;
}

pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild_id) = message.guild_id else { return };
    if message.author.bot || !data.is_cog_enabled(guild_id.get(), COG_NAME) {
        return;
    }
    let (burst, duplicate) = burst_and_duplicate(guild_id.get(), message.author.id.get(), &message.content);
    let flags = MessageFlags { burst, duplicate, ..analyse(&message.content) };
    let messages = {
        let store = data.suspicious_accounts.lock().unwrap();
        if let Err(e) = store.message(guild_id.get(), message.author.id.get(), Utc::now().timestamp(), flags) {
            return log::error(format!("Error recording a message for suspicious accounts: {e}"));
        }
        store.activity(guild_id.get(), message.author.id.get()).map(|a| a.messages).unwrap_or(0)
    };
    // Rescore only when something stands out, or while they're new
    let notable = flags != MessageFlags::default();
    if notable || messages <= WATCH_FIRST_MESSAGES {
        let joined_at = message.member.as_ref().and_then(|m| m.joined_at);
        check_threshold(ctx, data, guild_id, &message.author, joined_at).await;
    }
}

/// Scores members on how suspicious their accounts look
#[command(
    slash_command,
    guild_only,
    ephemeral,
    category = "SuspiciousAccounts",
    required_permissions = "MODERATE_MEMBERS",
    default_member_permissions = "MODERATE_MEMBERS",
    subcommands("check", "list", "setup"),
    subcommand_required
)]
pub async fn suspicious(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// See a member's suspicion score and what makes it up
#[command(slash_command, guild_only, ephemeral, required_permissions = "MODERATE_MEMBERS")]
async fn check(ctx: Context<'_>, #[description = "The member to check"] user: serenity::User) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let joined_at = guild_id.member(ctx, user.id).await.ok().and_then(|m| m.joined_at);
    let score = score_user(ctx.data(), guild_id, &user, joined_at)?;
    let colour = match score.total {
        0..=29 => GREEN,
        30..=59 => ORANGE,
        _ => RED,
    };
    let embed = serenity::CreateEmbed::new()
        .colour(colour)
        .title(format!("Suspicion score: {}", user.tag()))
        .thumbnail(user.face())
        .field("Score", label(&score), true)
        .field("ID", format!("`{}`", user.id), true)
        .field("Why", reasons_text(&score), false);
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// See the most suspicious members on the server
#[command(slash_command, guild_only, ephemeral, required_permissions = "MODERATE_MEMBERS")]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().unwrap();
    // Members who've done something, plus anyone who joined in the last 30 days
    let tracked = ctx.data().suspicious_accounts.lock().unwrap().members(guild_id.get())?;
    let recent_cutoff = Utc::now().timestamp() - 30 * 86_400;
    let members: Vec<serenity::Member> = ctx
        .guild()
        .map(|g| {
            g.members
                .values()
                .filter(|m| !m.user.bot)
                .filter(|m| tracked.contains(&m.user.id.get()) || m.joined_at.is_some_and(|j| j.unix_timestamp() >= recent_cutoff))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut scored: Vec<(u32, serenity::UserId, String)> = vec![];
    for member in &members {
        let score = score_user(ctx.data(), guild_id, &member.user, member.joined_at)?;
        if score.total > 0 {
            let top = score.reasons.iter().filter(|r| r.points > 0).max_by_key(|r| r.points).map(|r| r.text.clone()).unwrap_or_default();
            scored.push((score.total, member.user.id, top));
        }
    }
    if scored.is_empty() {
        ctx.say("Nobody here looks suspicious.").await?;
        return Ok(());
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(50);
    let lines: Vec<String> = scored.iter().map(|(total, id, top)| format!("**{total}** <@{id}>: {top}")).collect();
    let pages = chunk_lines(&lines, 10)
        .into_iter()
        .map(|page| serenity::CreateEmbed::new().colour(ORANGE).title("Most suspicious members").description(format!("{page}\n\nSee why with `/suspicious check`.")))
        .collect();
    paginate(ctx, pages).await
}

/// Set the alert threshold, where alerts go, and whether to quarantine automatically
#[command(slash_command, guild_only, ephemeral, required_permissions = "MANAGE_GUILD")]
async fn setup(
    ctx: Context<'_>,
    #[description = "Alert staff when a score reaches this (default 60)"]
    #[min = 1]
    #[max = 100]
    threshold: Option<u32>,
    #[description = "Where to post alerts (default: quarantine review channel, join & leave log, or system channel)"]
    #[channel_types("Text", "News")]
    alert_channel: Option<serenity::GuildChannel>,
    #[description = "Also quarantine members who reach the threshold (default no)"] auto_quarantine: Option<bool>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let settings = {
        let store = ctx.data().suspicious_accounts.lock().unwrap();
        let mut settings = store.settings(guild_id)?;
        settings.threshold = threshold.unwrap_or(settings.threshold);
        settings.alert_channel = alert_channel.map(|c| c.id.get()).or(settings.alert_channel);
        settings.auto_quarantine = auto_quarantine.unwrap_or(settings.auto_quarantine);
        store.set_settings(guild_id, &settings)?;
        settings
    };
    let alerts = settings.alert_channel.map_or_else(|| "the quarantine review channel, join & leave log, or system channel".to_string(), |c| format!("<#{c}>"));
    let text = format!(
        "✅ Staff are alerted in {alerts} when a member's score reaches **{}**.\nAuto-quarantine: **{}**",
        settings.threshold,
        if settings.auto_quarantine { "on" } else { "off" }
    );
    ctx.say(text).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn established() -> Signals {
        Signals { account_age_days: 1000, username: "someone".into(), hours_on_server: Some(24 * 365), ..Default::default() }
    }

    #[test]
    fn established_members_score_nothing() {
        assert_eq!(score(&established()).total, 0);
    }

    #[test]
    fn fresh_throwaways_score_high() {
        let s = Signals {
            account_age_days: 0,
            default_avatar: true,
            no_display_name: true,
            username: "user84721".into(),
            hours_on_server: Some(0),
            first_message_after_secs: Some(10),
            activity: Activity { messages: 3, invites: 3, links: 3, ..Default::default() },
            ..Default::default()
        };
        let result = score(&s);
        // Account 40 (capped from 50), activity 10, content 30
        assert_eq!(result.total, 80);
    }

    #[test]
    fn activity_can_lower_a_score_but_not_below_zero() {
        let s = Signals { account_age_days: 20, activity: Activity { messages: 500, ..Default::default() }, hours_on_server: Some(24 * 60), ..established() };
        assert_eq!(score(&s).total, 0);
        assert!(score(&s).reasons.iter().any(|r| r.points < 0));
    }

    #[test]
    fn history_counts_most() {
        let s = Signals { honeypot_here: true, honeypot_other_servers: 2, incidents: 5, ..established() };
        let result = score(&s);
        // Honeypot here 30 + elsewhere 20, capped at 50 before incidents
        assert_eq!(result.total, 50);
    }

    #[test]
    fn analyses_messages() {
        assert_eq!(analyse("hello there friend"), MessageFlags::default());
        assert!(analyse("join discord.gg/abc").invite);
        assert!(analyse("see https://example.com").link);
        assert!(analyse("@everyone free nitro").mass_mention);
        assert!(analyse("<@1> <@2> <@3> <@4> <@5>").mass_mention);
        assert!(!analyse("<@1> <@2>").mass_mention);
        assert!(analyse("THIS IS SO LOUD RIGHT NOW").noisy);
        assert!(analyse("noooooooo").noisy);
        // Ordinary long words aren't shouting
        assert!(!analyse("Absolutely wonderful").noisy);
    }

    #[test]
    fn spots_generated_names() {
        assert!(looks_generated("user84721"));
        assert!(!looks_generated("creativecat"));
        assert!(!looks_generated("cat2024x"));
    }

    #[test]
    fn detects_bursts_and_repeats_once() {
        let (g, u) = (999_001, 1);
        let mut bursts = 0;
        let mut repeats = 0;
        for i in 0..6 {
            let (burst, dup) = burst_and_duplicate(g, u, if i < 4 { "buy now" } else { "other" });
            bursts += burst as u32;
            repeats += dup as u32;
        }
        assert_eq!(bursts, 1);
        assert_eq!(repeats, 1);
    }
}
