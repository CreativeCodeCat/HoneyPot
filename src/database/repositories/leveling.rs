//! Per-server XP, levels and reputation, persisted to SQLite.
//!
//! Levels follow the original bot's curve: reaching level `L` takes
//! `(L² - L) × rate / 2` total XP, so each level needs `rate` more XP than the last.
//! Everyone starts at level 1. Levels are always computed from XP, never stored.
//!
//! A message earns `base_xp`, or `max_xp` instead when it's sent within the engagement
//! window after the latest message from a different member in the same channel, so
//! conversation is rewarded rather than message volume.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use crate::database::{connection, migrations};

const DB_FILE: &str = "database/levels.db";

/// Limits for each setting, shared by `/rank-setup` and `Settings::validate`
pub const RATE_RANGE: (i64, i64) = (250, 7500);
pub const XP_RANGE: (i64, i64) = (1, 500);
pub const WINDOW_MINS_RANGE: (i64, i64) = (1, 60);
pub const COOLDOWN_SECS_RANGE: (i64, i64) = (0, 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// XP difference between consecutive levels
    pub rate: i64,
    /// XP for an ordinary message
    pub base_xp: i64,
    /// XP for a message that replies to someone else within the engagement window
    pub max_xp: i64,
    pub engagement_window_mins: i64,
    /// A member earns XP at most once per this many seconds
    pub xp_cooldown_secs: i64,
    /// Where level-ups are announced; `None` uses the system channel
    pub levelup_channel_id: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { rate: 2500, base_xp: 25, max_xp: 300, engagement_window_mins: 5, xp_cooldown_secs: 5 * 60, levelup_channel_id: None }
    }
}

impl Settings {
    /// Explains the first invalid setting, if any.
    pub fn validate(&self) -> Result<(), String> {
        let check = |name: &str, value: i64, (min, max): (i64, i64)| {
            if (min..=max).contains(&value) { Ok(()) } else { Err(format!("{name} must be between {min} and {max} (got {value}).")) }
        };
        check("The rate", self.rate, RATE_RANGE)?;
        check("Base XP", self.base_xp, XP_RANGE)?;
        check("Max XP", self.max_xp, XP_RANGE)?;
        check("The engagement window", self.engagement_window_mins, WINDOW_MINS_RANGE)?;
        check("The XP cooldown", self.xp_cooldown_secs, COOLDOWN_SECS_RANGE)?;
        if self.max_xp < self.base_xp {
            return Err(format!("Max XP ({}) can't be less than base XP ({}).", self.max_xp, self.base_xp));
        }
        Ok(())
    }

    /// XP a message earns: `max_xp` if it follows another member's message (`last`, as author
    /// and Unix milliseconds) within the engagement window, otherwise `base_xp`.
    pub fn message_xp(&self, last: Option<(u64, i64)>, author: u64, now_ms: i64) -> i64 {
        let window_ms = self.engagement_window_mins * 60_000;
        match last {
            Some((other, at)) if other != author && (0..=window_ms).contains(&(now_ms - at)) => self.max_xp,
            _ => self.base_xp,
        }
    }
}

/// What awarding XP for a message did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Award {
    pub old_xp: i64,
    pub new_xp: i64,
    /// Whether the engagement bonus (max XP) applied
    pub engaged: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub xp: i64,
    pub rep: i64,
    /// 1-based leaderboard position by XP
    pub position: i64,
}

/// Total XP needed to reach `level`.
pub fn xp_for(level: i64, rate: i64) -> i64 {
    (level * level - level) * rate / 2
}

/// The level a member with `xp` total XP is at.
pub fn level_for(xp: i64, rate: i64) -> i64 {
    // Inverse of xp_for, then corrected for floating-point rounding
    let mut level = ((1.0 + (1.0 + 8.0 * xp as f64 / rate as f64).sqrt()) / 2.0).floor() as i64;
    while xp_for(level + 1, rate) <= xp {
        level += 1;
    }
    while level > 1 && xp_for(level, rate) > xp {
        level -= 1;
    }
    level.max(1)
}

pub struct Store {
    conn: Connection,
    /// Latest human message per (guild, channel): author and Unix milliseconds.
    /// Kept in memory; after a restart the first message in a channel earns base XP.
    latest: HashMap<(u64, u64), (u64, i64)>,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/leveling.sql"))?;

        // Databases created before the level-up channel setting existed
        migrations::add_column(&conn, "settings", "levelup_channel_id", "INTEGER")?;
        // ...and before the engagement window and configurable cooldown
        let defaults = Settings::default();
        let window = format!("INTEGER NOT NULL DEFAULT {}", defaults.engagement_window_mins);
        migrations::add_column(&conn, "settings", "engagement_window_mins", &window)?;
        let cooldown = format!("INTEGER NOT NULL DEFAULT {}", defaults.xp_cooldown_secs);
        migrations::add_column(&conn, "settings", "xp_cooldown_secs", &cooldown)?;
        Ok(Self { conn, latest: HashMap::new() })
    }

    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        Ok(self
            .conn
            .query_row(
                // min_xp is the column base XP has always been stored in
                "SELECT rate, min_xp, max_xp, engagement_window_mins, xp_cooldown_secs, levelup_channel_id
                 FROM settings WHERE guild_id = ?",
                [guild_id],
                |row| {
                    Ok(Settings {
                        rate: row.get(0)?,
                        base_xp: row.get(1)?,
                        max_xp: row.get(2)?,
                        engagement_window_mins: row.get(3)?,
                        xp_cooldown_secs: row.get(4)?,
                        levelup_channel_id: row.get(5)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_settings(&self, guild_id: u64, settings: Settings) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, rate, min_xp, max_xp, engagement_window_mins, xp_cooldown_secs, levelup_channel_id)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                guild_id,
                settings.rate,
                settings.base_xp,
                settings.max_xp,
                settings.engagement_window_mins,
                settings.xp_cooldown_secs,
                settings.levelup_channel_id
            ],
        )?;
        Ok(())
    }

    /// Awards XP for a human message in a channel and records it as the channel's latest message,
    /// even when the member is on cooldown. Returns the award, or `None` if on cooldown.
    pub fn award_message_xp(
        &mut self,
        guild_id: u64,
        channel_id: u64,
        user_id: u64,
        now_ms: i64,
        settings: &Settings,
    ) -> rusqlite::Result<Option<Award>> {
        let last = self.latest.insert((guild_id, channel_id), (user_id, now_ms));
        let xp = settings.message_xp(last, user_id, now_ms);
        let awarded = self.award_xp(guild_id, user_id, xp, now_ms.div_euclid(1000), settings.xp_cooldown_secs)?;
        Ok(awarded.map(|(old_xp, new_xp)| Award { old_xp, new_xp, engaged: xp == settings.max_xp && xp != settings.base_xp }))
    }

    /// Gives `xp` to a member unless they were given XP less than `cooldown` seconds ago.
    /// Returns their (old, new) XP if it was awarded.
    pub fn award_xp(&mut self, guild_id: u64, user_id: u64, xp: i64, now: i64, cooldown: i64) -> rusqlite::Result<Option<(i64, i64)>> {
        let tx = self.conn.transaction()?;
        let current: Option<(i64, Option<i64>)> = tx
            .query_row("SELECT xp, last_xp FROM members WHERE guild_id = ? AND user_id = ?", [guild_id, user_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;

        let old = match current {
            Some((_, Some(last))) if now < last + cooldown => return Ok(None),
            Some((old, _)) => old,
            None => 0,
        };
        let new = old.saturating_add(xp);
        tx.execute(
            "INSERT INTO members (guild_id, user_id, xp, last_xp) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = ?3, last_xp = ?4",
            params![guild_id, user_id, new, now],
        )?;
        tx.commit()?;
        Ok(Some((old, new)))
    }

    /// A member's stats, or `None` if they've never earned XP or reputation.
    pub fn member(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Member>> {
        self.conn
            .query_row(
                "SELECT xp, rep, (SELECT COUNT(*) + 1 FROM members o WHERE o.guild_id = m.guild_id AND o.xp > m.xp)
                 FROM members m WHERE guild_id = ? AND user_id = ?",
                [guild_id, user_id],
                |row| Ok(Member { xp: row.get(0)?, rep: row.get(1)?, position: row.get(2)? }),
            )
            .optional()
    }

    /// The top members by XP: (user_id, xp).
    pub fn leaderboard(&self, guild_id: u64, limit: i64) -> rusqlite::Result<Vec<(u64, i64)>> {
        let mut stmt =
            self.conn.prepare("SELECT user_id, xp FROM members WHERE guild_id = ? AND xp > 0 ORDER BY xp DESC LIMIT ?")?;
        let rows = stmt.query_map(params![guild_id, limit], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Gives reputation from one member to another, limited to once per `cooldown` seconds
    /// per giver. Returns the receiver's new total, or `Err(next allowed time)`.
    pub fn give_rep(
        &mut self,
        guild_id: u64,
        giver: u64,
        receiver: u64,
        amount: i64,
        now: i64,
        cooldown: i64,
    ) -> rusqlite::Result<Result<i64, i64>> {
        let tx = self.conn.transaction()?;
        let last: Option<i64> = tx
            .query_row("SELECT last_rep_given FROM members WHERE guild_id = ? AND user_id = ?", [guild_id, giver], |row| {
                row.get(0)
            })
            .optional()?
            .flatten();
        if let Some(last) = last.filter(|last| now < last + cooldown) {
            return Ok(Err(last + cooldown));
        }

        tx.execute(
            "INSERT INTO members (guild_id, user_id, last_rep_given) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET last_rep_given = ?3",
            params![guild_id, giver, now],
        )?;
        tx.execute(
            "INSERT INTO members (guild_id, user_id, rep) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET rep = rep + ?3",
            params![guild_id, receiver, amount],
        )?;
        let total = tx.query_row("SELECT rep FROM members WHERE guild_id = ? AND user_id = ?", [guild_id, receiver], |row| row.get(0))?;
        tx.commit()?;
        Ok(Ok(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_curve_matches_original() {
        let rate = 2500;
        assert_eq!(xp_for(1, rate), 0);
        assert_eq!(xp_for(2, rate), 2500);
        assert_eq!(xp_for(3, rate), 7500);
        assert_eq!(level_for(0, rate), 1);
        assert_eq!(level_for(2499, rate), 1);
        assert_eq!(level_for(2500, rate), 2);
        assert_eq!(level_for(7499, rate), 2);
        assert_eq!(level_for(7500, rate), 3);
        for level in 1..500 {
            assert_eq!(level_for(xp_for(level, 250), 250), level);
            assert_eq!(level_for(xp_for(level, 250) - 1, 250), (level - 1).max(1));
        }
    }

    #[test]
    fn saves_settings_and_upgrades_old_databases() {
        let path = std::env::temp_dir().join(format!("levels-test-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        // A settings table from before levelup_channel_id existed
        connection::open(path)
            .unwrap()
            .execute_batch("CREATE TABLE settings (guild_id INTEGER PRIMARY KEY, rate INTEGER NOT NULL, min_xp INTEGER NOT NULL, max_xp INTEGER NOT NULL)")
            .unwrap();

        let s = Store::open_path(path).unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
        let settings = Settings { rate: 500, base_xp: 30, max_xp: 60, engagement_window_mins: 2, xp_cooldown_secs: 0, levelup_channel_id: Some(42) };
        s.save_settings(1, settings).unwrap();
        assert_eq!(s.settings(1).unwrap(), settings);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn xp_award_respects_cooldown() {
        let mut s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.award_xp(1, 10, 100, 1000, 300).unwrap(), Some((0, 100)));
        assert_eq!(s.award_xp(1, 10, 100, 1299, 300).unwrap(), None);
        assert_eq!(s.award_xp(1, 10, 50, 1300, 300).unwrap(), Some((100, 150)));
        assert_eq!(s.member(1, 10).unwrap(), Some(Member { xp: 150, rep: 0, position: 1 }));
    }

    const ALICE: u64 = 10;
    const BOB: u64 = 20;
    const MIN: i64 = 60_000;

    /// Base 25, max 300, 5 minute window, no cooldown unless a test sets one
    fn engagement_settings() -> Settings {
        Settings { xp_cooldown_secs: 0, ..Settings::default() }
    }

    /// XP a message earned (0 on cooldown).
    fn earned(s: &mut Store, channel: u64, user: u64, at_ms: i64, settings: &Settings) -> i64 {
        s.award_message_xp(1, channel, user, at_ms, settings).unwrap().map_or(0, |a| a.new_xp - a.old_xp)
    }

    #[test]
    fn conversation_example_from_the_spec() {
        let (mut s, cfg) = (Store::open_path(":memory:").unwrap(), engagement_settings());
        let t = 100 * MIN;
        assert_eq!(earned(&mut s, 1, ALICE, t, &cfg), 25, "quiet channel: base");
        assert_eq!(earned(&mut s, 1, BOB, t + 2 * MIN, &cfg), 300, "replying to Alice: max");
        assert_eq!(earned(&mut s, 1, BOB, t + 2 * MIN + 10_000, &cfg), 25, "own message doesn't count");
        assert_eq!(earned(&mut s, 1, ALICE, t + 3 * MIN + 10_000, &cfg), 300, "replying to Bob: max");
    }

    #[test]
    fn engagement_window_boundaries() {
        let cfg = engagement_settings();
        let window = 5 * MIN;
        assert_eq!(cfg.message_xp(Some((BOB, 0)), ALICE, window), 300, "exactly at the window edge");
        assert_eq!(cfg.message_xp(Some((BOB, 0)), ALICE, window + 1), 25, "just past the window");
        assert_eq!(cfg.message_xp(Some((BOB, 0)), ALICE, 0), 300, "same instant");
        assert_eq!(cfg.message_xp(Some((BOB, 10)), ALICE, 0), 25, "out-of-order timestamp");
        assert_eq!(cfg.message_xp(None, ALICE, 0), 25, "nobody has spoken");
        assert_eq!(cfg.message_xp(Some((ALICE, 0)), ALICE, 1), 25, "only yourself");
    }

    #[test]
    fn engagement_is_per_channel_and_guild() {
        let (mut s, cfg) = (Store::open_path(":memory:").unwrap(), engagement_settings());
        earned(&mut s, 1, ALICE, 0, &cfg);
        assert_eq!(earned(&mut s, 2, BOB, MIN, &cfg), 25, "Alice spoke in another channel");
        assert_eq!(s.award_message_xp(2, 1, BOB, MIN, &cfg).unwrap().unwrap().engaged, false, "another guild");
        assert_eq!(earned(&mut s, 1, BOB, MIN, &cfg), 300, "same channel and guild");
    }

    #[test]
    fn cooldown_blocks_xp_but_still_updates_the_latest_message() {
        let mut s = Store::open_path(":memory:").unwrap();
        let cfg = Settings { xp_cooldown_secs: 60, ..engagement_settings() };
        assert_eq!(earned(&mut s, 1, ALICE, 0, &cfg), 25);
        assert_eq!(earned(&mut s, 1, BOB, 10_000, &cfg), 300);
        // Alice is on cooldown and earns nothing, but she's now the latest speaker
        assert_eq!(earned(&mut s, 1, ALICE, 20_000, &cfg), 0);
        assert_eq!(earned(&mut s, 1, BOB, 80_000, &cfg), 300, "Bob replies to Alice");
        assert_eq!(earned(&mut s, 1, ALICE, 90_000, &cfg), 300, "cooldown over");
        assert_eq!(s.member(1, ALICE).unwrap().unwrap().xp, 325);
    }

    #[test]
    fn only_one_award_per_message() {
        let (mut s, cfg) = (Store::open_path(":memory:").unwrap(), engagement_settings());
        earned(&mut s, 1, ALICE, 0, &cfg);
        let award = s.award_message_xp(1, 1, BOB, MIN, &cfg).unwrap().unwrap();
        assert_eq!(award, Award { old_xp: 0, new_xp: 300, engaged: true }, "max XP replaces base, not added to it");
    }

    #[test]
    fn settings_are_validated() {
        assert_eq!(Settings::default().validate(), Ok(()));
        assert!(Settings { base_xp: 100, max_xp: 50, ..Settings::default() }.validate().is_err());
        assert_eq!(Settings { base_xp: 50, max_xp: 50, ..Settings::default() }.validate(), Ok(()));
        assert!(Settings { base_xp: 0, ..Settings::default() }.validate().is_err());
        assert!(Settings { engagement_window_mins: 0, ..Settings::default() }.validate().is_err());
        assert!(Settings { xp_cooldown_secs: -1, ..Settings::default() }.validate().is_err());
        assert!(Settings { rate: 100, ..Settings::default() }.validate().is_err());
    }

    #[test]
    fn leaderboard_and_positions() {
        let mut s = Store::open_path(":memory:").unwrap();
        s.award_xp(1, 10, 100, 0, 0).unwrap();
        s.award_xp(1, 20, 300, 0, 0).unwrap();
        s.award_xp(1, 30, 200, 0, 0).unwrap();
        s.award_xp(2, 40, 999, 0, 0).unwrap();
        assert_eq!(s.leaderboard(1, 10).unwrap(), vec![(20, 300), (30, 200), (10, 100)]);
        assert_eq!(s.member(1, 10).unwrap().unwrap().position, 3);
        assert_eq!(s.member(1, 20).unwrap().unwrap().position, 1);
    }

    #[test]
    fn reputation_has_per_giver_cooldown() {
        let mut s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.give_rep(1, 10, 20, 3, 1000, 7200).unwrap(), Ok(3));
        assert_eq!(s.give_rep(1, 10, 30, 3, 2000, 7200).unwrap(), Err(8200));
        assert_eq!(s.give_rep(1, 30, 20, 3, 2000, 7200).unwrap(), Ok(6));
        assert_eq!(s.give_rep(1, 10, 20, 3, 8200, 7200).unwrap(), Ok(9));
        // Giving reputation doesn't affect the giver's XP
        assert_eq!(s.member(1, 10).unwrap().unwrap().xp, 0);
    }
}
