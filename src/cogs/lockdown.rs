//! Lockdown cog: stops @everyone from talking in a channel, optionally for a set time.
//! The channel's original @everyone permissions are saved so unlocking restores them
//! exactly, and timed lockdowns are lifted even across restarts.

use std::time::Duration;

use chrono::Utc;
use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};
use serenity::Permissions as P;

use super::{Context, Data, Error};
use crate::log;

const DB_FILE: &str = "database/lockdown.db";

const ORANGE: u32 = 0xe67e22;
const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;

/// Permissions taken away from @everyone during a lockdown
const LOCK_PERMS: P = P::SEND_MESSAGES.union(P::SEND_MESSAGES_IN_THREADS);

/// Longest lockdown that can be set with a duration
const MAX_DURATION_SECS: i64 = 365 * 24 * 60 * 60;

/// Lockdown length when /lock is given no time
const DEFAULT_DURATION_SECS: i64 = 24 * 60 * 60;

pub struct Lockdown {
    pub channel_id: u64,
    /// @everyone's overwrite on the channel before the lockdown
    pub previous_allow: P,
    pub previous_deny: P,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        let conn = Connection::open(DB_FILE)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS lockdowns (
                channel_id INTEGER PRIMARY KEY,
                guild_id INTEGER NOT NULL,
                -- Unix time to lift the lockdown
                expires_at INTEGER,
                previous_allow INTEGER NOT NULL,
                previous_deny INTEGER NOT NULL,
                reason TEXT NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    fn lockdown_from_row(row: &rusqlite::Row) -> rusqlite::Result<Lockdown> {
        Ok(Lockdown {
            channel_id: row.get(0)?,
            previous_allow: P::from_bits_truncate(row.get(1)?),
            previous_deny: P::from_bits_truncate(row.get(2)?),
        })
    }

    pub fn get(&self, channel_id: u64) -> rusqlite::Result<Option<Lockdown>> {
        self.conn
            .query_row(
                "SELECT channel_id, previous_allow, previous_deny FROM lockdowns WHERE channel_id = ?",
                [channel_id],
                Self::lockdown_from_row,
            )
            .optional()
    }

    pub fn add(&self, guild_id: u64, lockdown: &Lockdown, expires_at: i64, reason: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO lockdowns (channel_id, guild_id, expires_at, previous_allow, previous_deny, reason)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                lockdown.channel_id,
                guild_id,
                expires_at,
                lockdown.previous_allow.bits(),
                lockdown.previous_deny.bits(),
                reason
            ],
        )?;
        Ok(())
    }

    pub fn remove(&self, channel_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM lockdowns WHERE channel_id = ?", [channel_id])?;
        Ok(())
    }

    pub fn expired(&self, now: i64) -> rusqlite::Result<Vec<Lockdown>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, previous_allow, previous_deny FROM lockdowns
             WHERE expires_at IS NOT NULL AND expires_at <= ?",
        )?;
        let rows = stmt.query_map([now], Self::lockdown_from_row)?;
        rows.collect()
    }
}

/// Parses durations like "30s", "10m", "2h", "1d", "1w" or "1h30m" into seconds.
pub fn parse_duration(text: &str) -> Option<i64> {
    let text = text.trim().to_lowercase();
    let mut total: i64 = 0;
    let mut rest = text.as_str();

    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let number: i64 = rest[..digits].parse().ok()?;
        rest = rest[digits..].trim_start();

        let unit_len = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let seconds = match &rest[..unit_len] {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86400,
            "w" | "week" | "weeks" => 604800,
            _ => return None,
        };
        total = total.checked_add(number.checked_mul(seconds)?)?;
        rest = rest[unit_len..].trim_start();
    }

    (total > 0 && total <= MAX_DURATION_SECS).then_some(total)
}

/// @everyone's current overwrite on a channel, as (allow, deny).
fn everyone_overwrite(ctx: Context<'_>, channel_id: serenity::ChannelId) -> (P, P) {
    let Some(guild) = ctx.guild() else { return (P::empty(), P::empty()) };
    let everyone = serenity::PermissionOverwriteType::Role(guild.id.everyone_role());
    guild
        .channels
        .get(&channel_id)
        .and_then(|channel| channel.permission_overwrites.iter().find(|o| o.kind == everyone))
        .map(|o| (o.allow, o.deny))
        .unwrap_or((P::empty(), P::empty()))
}

/// Puts @everyone's overwrite back to how it was before the lockdown.
async fn restore_overwrite(http: &serenity::Http, guild_id: serenity::GuildId, lockdown: &Lockdown) -> Result<(), Error> {
    let channel_id = serenity::ChannelId::new(lockdown.channel_id);
    let kind = serenity::PermissionOverwriteType::Role(guild_id.everyone_role());
    if lockdown.previous_allow.is_empty() && lockdown.previous_deny.is_empty() {
        channel_id.delete_permission(http, kind).await?;
    } else {
        let overwrite = serenity::PermissionOverwrite { allow: lockdown.previous_allow, deny: lockdown.previous_deny, kind };
        channel_id.create_permission(http, overwrite).await?;
    }
    Ok(())
}

fn lifted_embed() -> serenity::CreateEmbed {
    serenity::CreateEmbed::new().colour(GREEN).description("🔓 | The lockdown has been lifted!")
}

async fn reply_embed(ctx: Context<'_>, colour: u32, text: impl Into<String>, ephemeral: bool) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(ephemeral)).await?;
    Ok(())
}

/// Lockdown this channel.
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Lockdown",
    required_permissions = "MANAGE_CHANNELS",
    default_member_permissions = "MANAGE_CHANNELS"
)]
pub async fn lock(
    ctx: Context<'_>,
    #[description = "How long to lock for, e.g. 30m, 2h, 1d (default 24h)"] time: Option<String>,
    #[description = "Reason for this lockdown"]
    #[rest]
    reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let channel_id = ctx.channel_id();
    let reason = reason.unwrap_or_else(|| "no specified reason".to_string());

    let duration = match time.as_deref().map(parse_duration) {
        None => DEFAULT_DURATION_SECS,
        Some(Some(secs)) => secs,
        Some(None) => {
            return reply_embed(ctx, RED, "🟥 | Invalid time. Use something like `30m`, `2h`, `1d` or `1h30m` (max 1 year).", true).await;
        }
    };

    let already_locked = ctx.data().lockdown.lock().unwrap().get(channel_id.get())?.is_some();
    let (allow, deny) = everyone_overwrite(ctx, channel_id);
    if already_locked || deny.contains(P::SEND_MESSAGES) {
        return reply_embed(ctx, RED, "🟥 | This channel has already been locked!", true).await;
    }

    let overwrite = serenity::PermissionOverwrite {
        allow: allow - LOCK_PERMS,
        deny: deny | LOCK_PERMS,
        kind: serenity::PermissionOverwriteType::Role(guild_id.everyone_role()),
    };
    channel_id.create_permission(ctx, overwrite).await?;

    let expires_at = Utc::now().timestamp() + duration;
    let lockdown = Lockdown { channel_id: channel_id.get(), previous_allow: allow, previous_deny: deny };
    ctx.data().lockdown.lock().unwrap().add(guild_id.get(), &lockdown, expires_at, &reason)?;

    let text = format!(
        "🔒 | This channel is now on lockdown for: {reason}\nThe lockdown will be lifted <t:{expires_at}:R>."
    );
    reply_embed(ctx, ORANGE, text, false).await
}

/// Remove lockdown from this channel.
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Lockdown",
    required_permissions = "MANAGE_CHANNELS",
    default_member_permissions = "MANAGE_CHANNELS"
)]
pub async fn unlock(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();
    let channel_id = ctx.channel_id();

    let saved = ctx.data().lockdown.lock().unwrap().get(channel_id.get())?;
    let lockdown = match saved {
        Some(lockdown) => lockdown,
        None => {
            // Locked some other way (e.g. by hand): just give the permissions back
            let (allow, deny) = everyone_overwrite(ctx, channel_id);
            if !deny.contains(P::SEND_MESSAGES) {
                return reply_embed(ctx, RED, "🟥 | This channel has already been unlocked!", true).await;
            }
            Lockdown { channel_id: channel_id.get(), previous_allow: allow, previous_deny: deny - LOCK_PERMS }
        }
    };

    restore_overwrite(ctx.http(), guild_id, &lockdown).await?;
    ctx.data().lockdown.lock().unwrap().remove(channel_id.get())?;
    ctx.send(poise::CreateReply::default().embed(lifted_embed())).await?;
    Ok(())
}

/// Lifts timed lockdowns once they expire. Runs whether or not the cog is enabled,
/// so disabling it never leaves a channel locked.
pub async fn expiry_loop(ctx: serenity::Context, data: Data) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let expired = data.lockdown.lock().unwrap().expired(Utc::now().timestamp());
        let expired = match expired {
            Ok(expired) => expired,
            Err(e) => {
                log::error(format!("[LOCKDOWN] Error checking for expired lockdowns: {e}"));
                continue;
            }
        };

        for lockdown in expired {
            let channel_id = serenity::ChannelId::new(lockdown.channel_id);
            let guild_id = match channel_id.to_channel(&ctx).await {
                Ok(serenity::Channel::Guild(channel)) => channel.guild_id,
                Ok(_) => continue,
                Err(e) => {
                    // The channel is gone (or we lost access), so there's nothing left to unlock
                    log::warning(format!("[LOCKDOWN] Dropping lockdown for channel {channel_id}: {e}"));
                    let _ = data.lockdown.lock().unwrap().remove(lockdown.channel_id);
                    continue;
                }
            };

            if let Err(e) = restore_overwrite(&ctx.http, guild_id, &lockdown).await {
                log::error(format!("[LOCKDOWN] Could not lift lockdown in channel {channel_id}: {e}"));
                continue;
            }
            if let Err(e) = data.lockdown.lock().unwrap().remove(lockdown.channel_id) {
                log::error(format!("[LOCKDOWN] Error removing lockdown: {e}"));
            }
            let _ = channel_id.send_message(&ctx.http, serenity::CreateMessage::new().embed(lifted_embed())).await;
            log::info(format!("[LOCKDOWN] Lifted expired lockdown in channel {channel_id}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30s"), Some(30));
        assert_eq!(parse_duration("10m"), Some(600));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("1d"), Some(86400));
        assert_eq!(parse_duration("1w"), Some(604800));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("1 hour 30 mins"), Some(5400));
        assert_eq!(parse_duration("2 Days"), Some(172800));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("0m"), None);
        assert_eq!(parse_duration("2y"), None);
        assert_eq!(parse_duration("999999999999w"), None);
    }
}
