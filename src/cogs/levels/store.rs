//! Per-server XP, levels and reputation, persisted to SQLite.
//!
//! Levels follow the original bot's curve: reaching level `L` takes
//! `(L² - L) × rate / 2` total XP, so each level needs `rate` more XP than the last.
//! Everyone starts at level 1. Levels are always computed from XP, never stored.

use rusqlite::{params, Connection, OptionalExtension};

const DB_FILE: &str = "database/levels.db";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// XP difference between consecutive levels
    pub rate: i64,
    pub min_xp: i64,
    pub max_xp: i64,
    /// Where level-ups are announced; `None` uses the system channel
    pub levelup_channel_id: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { rate: 2500, min_xp: 25, max_xp: 300, levelup_channel_id: None }
    }
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
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (
                guild_id INTEGER PRIMARY KEY,
                rate INTEGER NOT NULL,
                min_xp INTEGER NOT NULL,
                max_xp INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS members (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                xp INTEGER NOT NULL DEFAULT 0,
                rep INTEGER NOT NULL DEFAULT 0,
                -- Unix times of the last XP award and the last reputation this member gave
                last_xp INTEGER,
                last_rep_given INTEGER,
                PRIMARY KEY (guild_id, user_id)
            );
            CREATE INDEX IF NOT EXISTS idx_members_xp ON members(guild_id, xp DESC);",
        )?;

        // Databases created before the level-up channel setting existed
        let has_channel_column: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('settings') WHERE name = 'levelup_channel_id'",
            [],
            |row| row.get(0),
        )?;
        if !has_channel_column {
            conn.execute_batch("ALTER TABLE settings ADD COLUMN levelup_channel_id INTEGER")?;
        }
        Ok(Self { conn })
    }

    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        Ok(self
            .conn
            .query_row(
                "SELECT rate, min_xp, max_xp, levelup_channel_id FROM settings WHERE guild_id = ?",
                [guild_id],
                |row| {
                    Ok(Settings {
                        rate: row.get(0)?,
                        min_xp: row.get(1)?,
                        max_xp: row.get(2)?,
                        levelup_channel_id: row.get(3)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_settings(&self, guild_id: u64, settings: Settings) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, rate, min_xp, max_xp, levelup_channel_id) VALUES (?, ?, ?, ?, ?)",
            params![guild_id, settings.rate, settings.min_xp, settings.max_xp, settings.levelup_channel_id],
        )?;
        Ok(())
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
        Connection::open(path)
            .unwrap()
            .execute_batch("CREATE TABLE settings (guild_id INTEGER PRIMARY KEY, rate INTEGER NOT NULL, min_xp INTEGER NOT NULL, max_xp INTEGER NOT NULL)")
            .unwrap();

        let s = Store::open_path(path).unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
        let settings = Settings { rate: 500, min_xp: 30, max_xp: 60, levelup_channel_id: Some(42) };
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
