//! Member profiles: the fields people fill in about themselves.

use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;
use crate::shared::utils::now_iso;

/// A profile entry: the key it's stored under, its label and emoji, and whether it sits
/// beside others in the embed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Gender,
    Age,
    Birthday,
    Languages,
    FavoriteColor,
    FavoriteAnimals,
    FavoriteFoods,
    FavoriteSongs,
    FavoriteArtists,
    FavoriteMovies,
    FavoriteActors,
    Origin,
    Status,
    Hobbies,
    AboutMe,
}

impl Field {
    /// In the order they're shown
    pub const ALL: [Self; 15] = [
        Self::Gender,
        Self::Age,
        Self::Birthday,
        Self::Languages,
        Self::FavoriteColor,
        Self::FavoriteAnimals,
        Self::FavoriteFoods,
        Self::FavoriteSongs,
        Self::FavoriteArtists,
        Self::FavoriteMovies,
        Self::FavoriteActors,
        Self::Origin,
        Self::Status,
        Self::Hobbies,
        Self::AboutMe,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Gender => "gender",
            Self::Age => "age",
            Self::Birthday => "birthday",
            Self::Languages => "languages",
            Self::FavoriteColor => "favorite-color",
            Self::FavoriteAnimals => "favorite-animals",
            Self::FavoriteFoods => "favorite-foods",
            Self::FavoriteSongs => "favorite-songs",
            Self::FavoriteArtists => "favorite-artists",
            Self::FavoriteMovies => "favorite-movies",
            Self::FavoriteActors => "favorite-actors",
            Self::Origin => "origin",
            Self::Status => "status",
            Self::Hobbies => "hobbies",
            Self::AboutMe => "aboutme",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AboutMe => "About Me",
            Self::FavoriteColor => "Favorite Color",
            Self::FavoriteAnimals => "Favorite Animals",
            Self::FavoriteFoods => "Favorite Foods",
            Self::FavoriteSongs => "Favorite Songs",
            Self::FavoriteArtists => "Favorite Artists",
            Self::FavoriteMovies => "Favorite Movies",
            Self::FavoriteActors => "Favorite Actors",
            Self::Gender => "Gender",
            Self::Age => "Age",
            Self::Birthday => "Birthday",
            Self::Languages => "Languages",
            Self::Origin => "Origin",
            Self::Status => "Status",
            Self::Hobbies => "Hobbies",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Self::Gender => "⚧️",
            Self::Age => "🎂",
            Self::Birthday => "📅",
            Self::Languages => "🌐",
            Self::FavoriteColor => "🌈",
            Self::FavoriteAnimals => "🐱",
            Self::FavoriteFoods => "🍕",
            Self::FavoriteSongs => "🎵",
            Self::FavoriteArtists => "🎤",
            Self::FavoriteMovies => "🎬",
            Self::FavoriteActors => "🎭",
            Self::Origin => "🌍",
            Self::Status => "🟢",
            Self::Hobbies => "🎮",
            Self::AboutMe => "ℹ️",
        }
    }

    pub fn inline(self) -> bool {
        !matches!(self, Self::Hobbies | Self::AboutMe)
    }
}

pub const DB_FILE: &str = "database/profiles.db";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/profiles.sql"))?;
        Ok(Self { conn })
    }

    /// Returns false if the user already had a profile.
    pub fn create(&self, user_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("INSERT OR IGNORE INTO profiles (user_id, updated) VALUES (?, ?)", params![user_id, now_iso()])? > 0)
    }

    /// Returns false if the user had no profile.
    pub fn delete(&self, user_id: u64) -> rusqlite::Result<bool> {
        self.conn.execute("DELETE FROM fields WHERE user_id = ?", [user_id])?;
        Ok(self.conn.execute("DELETE FROM profiles WHERE user_id = ?", [user_id])? > 0)
    }

    /// The profile's fields that are set, or `None` if the user has no profile.
    pub fn get(&self, user_id: u64) -> rusqlite::Result<Option<Vec<(Field, String)>>> {
        let exists = self.conn.query_row("SELECT 1 FROM profiles WHERE user_id = ?", [user_id], |_| Ok(())).optional()?;
        if exists.is_none() {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare("SELECT field, value FROM fields WHERE user_id = ?")?;
        let rows = stmt.query_map([user_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        let mut fields = Vec::new();
        for row in rows {
            let (key, value) = row?;
            if let Some(field) = Field::ALL.into_iter().find(|f| f.key() == key) {
                fields.push((field, value));
            }
        }
        Ok(Some(fields))
    }

    /// Returns false if the user has no profile.
    pub fn set(&self, user_id: u64, field: Field, value: &str) -> rusqlite::Result<bool> {
        let updated = self.conn.execute("UPDATE profiles SET updated = ? WHERE user_id = ?", params![now_iso(), user_id])?;
        if updated == 0 {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO fields (user_id, field, value) VALUES (?, ?, ?)",
            params![user_id, field.key(), value],
        )?;
        Ok(true)
    }
}
