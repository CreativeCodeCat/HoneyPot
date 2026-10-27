//! Games cog: small mini-games. `/hack` is based on the PhantomBot hack system: guess
//! the password the bot picks at random, and crack it to win money for your wallet.

use std::time::Duration;

use chrono::Utc;
use poise::serenity_prelude as serenity;
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};

use super::economy::money;
use super::economy::store::{EcoError, Pocket};
use super::{Context, Error};

/// Money paid into the wallet for a cracked password
const HACK_REWARD: i64 = 2500;
/// Seconds between /hack attempts for each member
const HACK_COOLDOWN_SECS: i64 = 5 * 60;
const MAX_GUESS_LEN: usize = 32;

const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;
const TERMINAL: u32 = 0x2b2d31;

/// The bot picks one of these each time; guesses aren't case sensitive.
const PASSWORDS: &[&str] = &[
    "password", "123456", "qwerty", "letmein", "admin", "monkey", "dragon", "iloveyou", "sunshine", "football",
    "baseball", "welcome", "shadow", "master", "hunter2", "trustno1", "abc123", "passw0rd", "princess", "superman",
    "starwars", "whatever", "freedom", "000000", "changeme",
];

/// Lines shown while the hack "runs", one every 1.25 seconds (5 seconds in total, as in the original).
const HACK_STEPS: &[&str] = &[
    "Connecting to the mainframe...",
    "Bypassing the firewall...",
    "Injecting payload...",
    "Cracking the password...",
];

const DB_FILE: &str = "database/games.db";

/// Game cooldowns, persisted to SQLite so they survive restarts.
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cooldowns (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                game TEXT NOT NULL,
                -- Unix time the member last played
                last_played INTEGER NOT NULL,
                PRIMARY KEY (guild_id, user_id, game)
            )",
        )?;
        Ok(Self { conn })
    }

    /// Records a play at `now`, or returns the Unix time the member can play again.
    pub fn start_cooldown(&self, guild_id: u64, user_id: u64, game: &str, now: i64, cooldown: i64) -> rusqlite::Result<Result<(), i64>> {
        let last: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_played FROM cooldowns WHERE guild_id = ? AND user_id = ? AND game = ?",
                params![guild_id, user_id, game],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(ready) = last.map(|last| last + cooldown).filter(|ready| now < *ready) {
            return Ok(Err(ready));
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO cooldowns (guild_id, user_id, game, last_played) VALUES (?, ?, ?, ?)",
            params![guild_id, user_id, game, now],
        )?;
        Ok(Ok(()))
    }
}

fn terminal(lines: &[String], colour: u32) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new().title("💻 Hack").colour(colour).description(lines.join("\n"))
}

/// Try to hack the bot by guessing its password
#[poise::command(slash_command, prefix_command, guild_only, category = "Games")]
pub async fn hack(
    ctx: Context<'_>,
    #[description = "The password to try"]
    #[rest]
    password: String,
) -> Result<(), Error> {
    let guess = password.trim().to_string();
    if guess.is_empty() || guess.chars().count() > MAX_GUESS_LEN {
        let reply = format!("❌ Usage: `/hack <password>` (up to {MAX_GUESS_LEN} characters).");
        ctx.send(poise::CreateReply::default().content(reply).ephemeral(true)).await?;
        return Ok(());
    }

    let guild_id = ctx.guild_id().unwrap().get();
    let user_id = ctx.author().id;
    let cooldown =
        ctx.data().games.lock().unwrap().start_cooldown(guild_id, user_id.get(), "hack", Utc::now().timestamp(), HACK_COOLDOWN_SECS)?;
    if let Err(ready) = cooldown {
        let reply = format!("❌ The mainframe is on alert. You can hack again <t:{ready}:R>.");
        ctx.send(poise::CreateReply::default().content(reply).ephemeral(true)).await?;
        return Ok(());
    }

    let answer = PASSWORDS[rand::rng().random_range(0..PASSWORDS.len())];
    let guess_shown = guess.replace('`', "'");

    let mut lines = vec![format!("<@{user_id}> is hacking with password `{guess_shown}`. Please wait...")];
    let message = ctx.send(poise::CreateReply::default().embed(terminal(&lines, TERMINAL))).await?;
    for step in HACK_STEPS {
        tokio::time::sleep(Duration::from_millis(1250)).await;
        lines.push(format!("`>` {step}"));
        message.edit(ctx, poise::CreateReply::default().embed(terminal(&lines, TERMINAL))).await?;
    }

    if !guess.eq_ignore_ascii_case(answer) {
        lines.push(String::new());
        lines.push(format!(
            "❌ **Access denied.** Might want to try harder next time... `{guess_shown}` was incorrect. The correct password was `{answer}`."
        ));
        message.edit(ctx, poise::CreateReply::default().embed(terminal(&lines, RED))).await?;
        return Ok(());
    }

    let paid = ctx.data().economy.lock().unwrap().adjust(guild_id, user_id.get(), Pocket::Wallet, HACK_REWARD)?;
    lines.push(String::new());
    lines.push(match paid {
        Ok(_) => format!("✅ **Access granted!** `{answer}` was the password. You stole {} into your wallet!", money(HACK_REWARD)),
        Err(EcoError::NoAccount) => format!(
            "✅ **Access granted!** `{answer}` was the password. Join the economy with `/eco join` to get {} next time!",
            money(HACK_REWARD)
        ),
        Err(_) => format!("✅ **Access granted!** `{answer}` was the password, but your wallet is already full."),
    });
    message.edit(ctx, poise::CreateReply::default().embed(terminal(&lines, GREEN))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_is_per_member_server_and_game() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.start_cooldown(1, 10, "hack", 1000, 300).unwrap(), Ok(()));
        assert_eq!(s.start_cooldown(1, 10, "hack", 1299, 300).unwrap(), Err(1300));
        assert_eq!(s.start_cooldown(1, 11, "hack", 1000, 300).unwrap(), Ok(()));
        assert_eq!(s.start_cooldown(2, 10, "hack", 1000, 300).unwrap(), Ok(()));
        assert_eq!(s.start_cooldown(1, 10, "other", 1000, 300).unwrap(), Ok(()));
        assert_eq!(s.start_cooldown(1, 10, "hack", 1300, 300).unwrap(), Ok(()));
    }

    #[test]
    fn cooldown_survives_reopening() {
        let path = std::env::temp_dir().join(format!("games-test-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        Store::open_path(path).unwrap().start_cooldown(1, 10, "hack", 1000, 300).unwrap().unwrap();
        assert_eq!(Store::open_path(path).unwrap().start_cooldown(1, 10, "hack", 1100, 300).unwrap(), Err(1300));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn passwords_are_unique_and_lowercase() {
        let mut sorted = PASSWORDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), PASSWORDS.len());
        assert!(PASSWORDS.iter().all(|p| p.to_lowercase() == *p && p.len() <= MAX_GUESS_LEN));
    }
}
