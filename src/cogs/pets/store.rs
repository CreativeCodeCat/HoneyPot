//! Pets and their needs, persisted to SQLite.
//!
//! Stats follow the original PhantomBot tamagotchi: one treat of food lasts 24 hours,
//! fun drains twice as fast, and a pet whose food reaches zero dies.

use rusqlite::{params, Connection, OptionalExtension, Row};

const DB_FILE: &str = "database/pets.db";

/// Hours one treat lasts
pub const FOOD_TTL_HOURS: f64 = 24.0;
/// Treats a pet can hold, plus one per level
pub const BASE_FOOD_MAX: f64 = 30.0;
/// Highest experience; fun is capped at half of this
pub const LEVEL_CAP: f64 = 100.0;
/// Fun at or above this counts as happy
pub const HAPPY_FUN: f64 = 7.0;
/// Owners are warned when food drops to this many treats (about a day left)
pub const HUNGRY_FOOD: f64 = 1.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Pet {
    pub guild_id: u64,
    pub owner_id: u64,
    /// Key from `ANIMALS`, e.g. "saber_tooth_tiger"
    pub kind: String,
    pub name: String,
    /// Unix time it hatched
    pub born: i64,
    /// Treats left; each lasts `FOOD_TTL_HOURS`
    pub food: f64,
    pub fun: f64,
    pub exp: f64,
    pub male: bool,
}

pub enum Mood {
    Happy,
    Meh,
    Sad,
}

impl Pet {
    pub fn new(guild_id: u64, owner_id: u64, kind: &str, name: &str, born: i64, male: bool) -> Self {
        Self { guild_id, owner_id, kind: kind.to_string(), name: name.to_string(), born, food: 7.0, fun: 3.0, exp: 0.0, male }
    }

    pub fn level(&self) -> i64 {
        self.exp.floor() as i64
    }

    pub fn max_food(&self) -> f64 {
        BASE_FOOD_MAX + self.exp.floor()
    }

    pub fn is_happy(&self) -> bool {
        self.fun >= HAPPY_FUN
    }

    pub fn mood(&self) -> Mood {
        if self.fun > HAPPY_FUN {
            Mood::Happy
        } else if self.fun > 0.0 {
            Mood::Meh
        } else {
            Mood::Sad
        }
    }

    /// "him" or "her"
    pub fn pronoun(&self) -> &'static str {
        if self.male { "him" } else { "her" }
    }

    pub fn add_exp(&mut self, amount: f64) {
        self.exp = (self.exp + amount).clamp(0.0, LEVEL_CAP);
    }

    pub fn add_fun(&mut self, amount: f64) {
        self.fun = (self.fun + amount).clamp(0.0, LEVEL_CAP / 2.0);
    }

    /// Applies `hours` of hunger and boredom.
    fn decay(&mut self, hours: f64) {
        self.food -= hours / FOOD_TTL_HOURS;
        self.add_fun(-hours / (FOOD_TTL_HOURS / 2.0));
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            guild_id: row.get("guild_id")?,
            owner_id: row.get("owner_id")?,
            kind: row.get("kind")?,
            name: row.get("name")?,
            born: row.get("born")?,
            food: row.get("food")?,
            fun: row.get("fun")?,
            exp: row.get("exp")?,
            male: row.get("male")?,
        })
    }
}

/// Something owners should hear about after time passes.
pub enum Alert {
    Hungry(Pet),
    Died(Pet),
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
            "CREATE TABLE IF NOT EXISTS pets (
                guild_id INTEGER NOT NULL,
                owner_id INTEGER NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                born INTEGER NOT NULL,
                food REAL NOT NULL,
                fun REAL NOT NULL,
                exp REAL NOT NULL,
                male BOOLEAN NOT NULL,
                -- Unix time hunger and boredom were last applied
                last_decay INTEGER NOT NULL,
                PRIMARY KEY (guild_id, owner_id)
            )",
        )?;
        Ok(Self { conn })
    }

    pub fn get(&self, guild_id: u64, owner_id: u64) -> rusqlite::Result<Option<Pet>> {
        self.conn
            .query_row("SELECT * FROM pets WHERE guild_id = ? AND owner_id = ?", [guild_id, owner_id], Pet::from_row)
            .optional()
    }

    /// Adds a new pet. Returns false if the owner already has one.
    pub fn create(&self, pet: &Pet) -> rusqlite::Result<bool> {
        let created = self.conn.execute(
            "INSERT OR IGNORE INTO pets (guild_id, owner_id, kind, name, born, food, fun, exp, male, last_decay)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?5)",
            params![pet.guild_id, pet.owner_id, pet.kind, pet.name, pet.born, pet.food, pet.fun, pet.exp, pet.male],
        )?;
        Ok(created > 0)
    }

    /// Saves a pet's food, fun and experience.
    pub fn save(&self, pet: &Pet) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE pets SET food = ?, fun = ?, exp = ? WHERE guild_id = ? AND owner_id = ?",
            params![pet.food, pet.fun, pet.exp, pet.guild_id, pet.owner_id],
        )?;
        Ok(())
    }

    /// Returns false if the owner had no pet.
    pub fn remove(&self, guild_id: u64, owner_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM pets WHERE guild_id = ? AND owner_id = ?", [guild_id, owner_id])? > 0)
    }

    /// Applies hunger and boredom for the time since each pet was last updated,
    /// removes pets that starved, and reports what owners should be told.
    /// Pets in servers where `active` is false are paused instead.
    pub fn decay_all(&mut self, now: i64, active: impl Fn(u64) -> bool) -> rusqlite::Result<Vec<Alert>> {
        let tx = self.conn.transaction()?;
        let pets: Vec<(Pet, i64)> = {
            let mut stmt = tx.prepare("SELECT *, last_decay FROM pets")?;
            let rows = stmt.query_map([], |row| Ok((Pet::from_row(row)?, row.get("last_decay")?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        let mut alerts = vec![];
        for (mut pet, last_decay) in pets {
            let hours = if active(pet.guild_id) { (now - last_decay).max(0) as f64 / 3600.0 } else { 0.0 };
            let food_before = pet.food;
            pet.decay(hours);

            if pet.food <= 0.0 {
                tx.execute("DELETE FROM pets WHERE guild_id = ? AND owner_id = ?", [pet.guild_id, pet.owner_id])?;
                alerts.push(Alert::Died(pet));
                continue;
            }
            tx.execute(
                "UPDATE pets SET food = ?, fun = ?, last_decay = ? WHERE guild_id = ? AND owner_id = ?",
                params![pet.food, pet.fun, now, pet.guild_id, pet.owner_id],
            )?;
            if food_before > HUNGRY_FOOD && pet.food <= HUNGRY_FOOD {
                alerts.push(Alert::Hungry(pet));
            }
        }
        tx.commit()?;
        Ok(alerts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_path(":memory:").unwrap()
    }

    #[test]
    fn one_pet_per_owner_per_server() {
        let s = store();
        assert!(s.create(&Pet::new(1, 10, "cat", "Tom", 0, true)).unwrap());
        assert!(!s.create(&Pet::new(1, 10, "dog", "Rex", 0, true)).unwrap());
        assert!(s.create(&Pet::new(2, 10, "dog", "Rex", 0, true)).unwrap());
        assert_eq!(s.get(1, 10).unwrap().unwrap().name, "Tom");
        assert!(s.remove(1, 10).unwrap());
        assert!(s.get(1, 10).unwrap().is_none());
    }

    #[test]
    fn stats_are_capped() {
        let mut pet = Pet::new(1, 10, "cat", "Tom", 0, true);
        pet.add_exp(500.0);
        pet.add_fun(500.0);
        assert_eq!((pet.exp, pet.fun), (LEVEL_CAP, LEVEL_CAP / 2.0));
        pet.add_fun(-500.0);
        assert_eq!(pet.fun, 0.0);
        assert_eq!(pet.max_food(), BASE_FOOD_MAX + LEVEL_CAP);
    }

    #[test]
    fn pets_get_hungry_then_starve() {
        let mut s = store();
        s.create(&Pet::new(1, 10, "cat", "Tom", 0, true)).unwrap(); // 7 treats

        // Six days later: one treat left, so the owner is warned
        let alerts = s.decay_all(6 * 86400, |_| true).unwrap();
        assert!(matches!(alerts.as_slice(), [Alert::Hungry(p)] if p.name == "Tom"));
        let pet = s.get(1, 10).unwrap().unwrap();
        assert!((pet.food - 1.0).abs() < 1e-9);
        assert_eq!(pet.fun, 0.0);

        // No repeat warning an hour later
        assert!(s.decay_all(6 * 86400 + 3600, |_| true).unwrap().is_empty());

        // The seventh day it starves
        let alerts = s.decay_all(7 * 86400, |_| true).unwrap();
        assert!(matches!(alerts.as_slice(), [Alert::Died(p)] if p.name == "Tom"));
        assert!(s.get(1, 10).unwrap().is_none());
    }

    #[test]
    fn paused_servers_dont_decay() {
        let mut s = store();
        s.create(&Pet::new(1, 10, "cat", "Tom", 0, true)).unwrap();
        assert!(s.decay_all(30 * 86400, |_| false).unwrap().is_empty());
        assert_eq!(s.get(1, 10).unwrap().unwrap().food, 7.0);
        // Resuming only counts time since the pause ended
        s.decay_all(30 * 86400 + 86400, |_| true).unwrap();
        assert!((s.get(1, 10).unwrap().unwrap().food - 6.0).abs() < 1e-9);
    }

    #[test]
    fn saving_keeps_stats() {
        let s = store();
        let mut pet = Pet::new(1, 10, "cat", "Tom", 0, false);
        s.create(&pet).unwrap();
        pet.food = 12.5;
        pet.add_exp(3.25);
        pet.add_fun(4.0);
        s.save(&pet).unwrap();
        assert_eq!(s.get(1, 10).unwrap().unwrap(), pet);
    }
}
