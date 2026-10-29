//! Profile cog: members make an "about me" profile with `/profile create`, fill it in with
//! `/profile set ...`, and anyone can see it with `/profile view` or the "Profile" user
//! context menu. Profiles belong to the user, so they're the same in every server.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use super::{now_iso, Context, Error};

const DB_FILE: &str = "database/profiles.db";
const COLOUR: u32 = 0x2b2d31;

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
    const ALL: [Self; 15] = [
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

    fn key(self) -> &'static str {
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

    fn label(self) -> &'static str {
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

    fn emoji(self) -> &'static str {
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

    fn inline(self) -> bool {
        !matches!(self, Self::Hobbies | Self::AboutMe)
    }
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
            "CREATE TABLE IF NOT EXISTS profiles (
                user_id INTEGER PRIMARY KEY,
                updated TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS fields (
                user_id INTEGER NOT NULL,
                -- A `Field::key`
                field TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (user_id, field)
            )",
        )?;
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

/// "1st", "2nd", "23rd"...
fn ordinal(day: u32) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

/// Formats a DD-MM-YYYY birthday like "Wednesday, 23rd February 2022"; anything else is kept as given.
fn format_birthday(text: &str) -> String {
    match chrono::NaiveDate::parse_from_str(text.trim(), "%d-%m-%Y") {
        Ok(date) => {
            use chrono::Datelike;
            format!("{}, {} {}", date.format("%A"), ordinal(date.day()), date.format("%B %Y"))
        }
        Err(_) => text.trim().to_string(),
    }
}

fn notice(title: &str, text: &str) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new().colour(COLOUR).title(title).description(format!("> {text}")).timestamp(serenity::Timestamp::now())
}

async fn reply(ctx: Context<'_>, embed: serenity::CreateEmbed, ephemeral: bool) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(ephemeral)).await?;
    Ok(())
}

fn block(text: &str) -> String {
    format!("```{}```", super::truncate(&text.replace("```", "'''"), 1000))
}

/// Shows `user`'s profile, or says they don't have one.
async fn show(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    ctx.defer().await?;
    let Some(fields) = ctx.data().profiles.lock().unwrap().get(user.id.get())? else {
        let text = if user.id == ctx.author().id {
            "You don't have a profile yet. Make one with `/profile create`.".to_string()
        } else {
            format!("{} doesn't have a profile yet.", user.display_name())
        };
        return reply(ctx, notice("❌ No Profile Found", &text), false).await;
    };

    let mut embed = serenity::CreateEmbed::new()
        .colour(COLOUR)
        .title(format!("{}'s Profile", user.display_name()))
        .thumbnail(user.face())
        .field("👤 Name", block(user.display_name()), true)
        .field("🔖 Username", block(&user.name), true)
        .field("🆔 ID", block(&user.id.to_string()), true);
    // Fields that aren't set are left out
    for field in Field::ALL {
        if let Some((_, value)) = fields.iter().find(|(f, _)| *f == field) {
            embed = embed.field(format!("{} {}", field.emoji(), field.label()), block(value), field.inline());
        }
    }

    // Their money in this server, if the Economy cog is on and they've joined it
    if let Some(guild_id) = ctx.guild_id() {
        if ctx.data().is_cog_enabled(guild_id.get(), "economy") {
            let balance = ctx.data().economy.lock().unwrap().balance(guild_id.get(), user.id.get())?;
            if let Some(b) = balance {
                let money = format!(
                    "Wallet: {} • Bank: {}",
                    super::economy::money(b.wallet),
                    super::economy::money(b.bank)
                );
                embed = embed.field("💰 Money", block(&money), false);
            }
        }
    }

    embed = embed.footer(serenity::CreateEmbedFooter::new("About Me").icon_url(ctx.author().face())).timestamp(serenity::Timestamp::now());
    reply(ctx, embed, false).await
}

/// Saves one profile field for the author.
async fn set_field(ctx: Context<'_>, field: Field, value: String) -> Result<(), Error> {
    let value = value.trim().to_string();
    if !ctx.data().profiles.lock().unwrap().set(ctx.author().id.get(), field, &value)? {
        return reply(ctx, notice("❌ No Profile Found", "You don't have a profile yet. Make one with `/profile create`."), true).await;
    }
    let text = format!("{} set their {} to **{value}**.", ctx.author().display_name(), field.label().to_lowercase());
    reply(ctx, notice("✅ Profile Updated", &text), true).await
}

/// User profiles
#[poise::command(
    slash_command,
    category = "Profile",
    subcommands("view", "create", "delete", "set", "help"),
    subcommand_required
)]
pub async fn profile(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// View a user's profile
#[poise::command(slash_command)]
async fn view(
    ctx: Context<'_>,
    #[description = "The user whose profile you want to see (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    show(ctx, user.unwrap_or_else(|| ctx.author().clone())).await
}

/// View this user's profile
#[poise::command(context_menu_command = "Profile", category = "Profile")]
pub async fn profile_menu(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    show(ctx, user).await
}

/// Create your profile
#[poise::command(slash_command)]
async fn create(ctx: Context<'_>) -> Result<(), Error> {
    if !ctx.data().profiles.lock().unwrap().create(ctx.author().id.get())? {
        return reply(ctx, notice("❌ Profile Already Exists", "You already have a profile. Fill it in with `/profile set`."), true).await;
    }
    reply(ctx, notice("✅ Profile Created", "Your profile has been created. Fill it in with `/profile set`."), true).await
}

/// Delete your profile
#[poise::command(slash_command)]
async fn delete(ctx: Context<'_>) -> Result<(), Error> {
    if !ctx.data().profiles.lock().unwrap().delete(ctx.author().id.get())? {
        return reply(ctx, notice("❌ No Profile Found", "You don't have a profile to delete."), true).await;
    }
    reply(ctx, notice("✅ Profile Deleted", "Your profile has been deleted."), true).await
}

/// Fill in your profile
#[poise::command(
    slash_command,
    subcommands(
        "gender", "age", "birthday", "languages", "favorite_color", "favorite_animals", "favorite_foods",
        "favorite_songs", "favorite_artists", "favorite_movies", "favorite_actors", "origin", "status", "hobbies", "aboutme"
    ),
    subcommand_required
)]
async fn set(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// A `/profile set` subcommand that saves its one text option to `$field`.
macro_rules! text_setter {
    ($fn_name:ident, $name:literal, $field:expr, $doc:literal, $option:literal, $max:literal) => {
        #[doc = $doc]
        #[poise::command(slash_command, rename = $name)]
        async fn $fn_name(
            ctx: Context<'_>,
            #[description = $option]
            #[max_length = $max]
            value: String,
        ) -> Result<(), Error> {
            set_field(ctx, $field, value).await
        }
    };
}

text_setter!(gender, "gender", Field::Gender, "Set your gender", "Your gender", 100);
text_setter!(languages, "languages", Field::Languages, "Set the languages you speak", "Your languages", 200);
text_setter!(favorite_color, "favorite-color", Field::FavoriteColor, "Set your favorite color", "Your favorite color", 100);
text_setter!(favorite_animals, "favorite-animals", Field::FavoriteAnimals, "Set your favorite animals", "Your favorite animals", 200);
text_setter!(favorite_foods, "favorite-foods", Field::FavoriteFoods, "Set your favorite foods", "Your favorite foods", 200);
text_setter!(favorite_songs, "favorite-songs", Field::FavoriteSongs, "Set your favorite songs", "Your favorite songs", 200);
text_setter!(favorite_artists, "favorite-artists", Field::FavoriteArtists, "Set your favorite artists", "Your favorite artists", 200);
text_setter!(favorite_movies, "favorite-movies", Field::FavoriteMovies, "Set your favorite movies", "Your favorite movies", 200);
text_setter!(favorite_actors, "favorite-actors", Field::FavoriteActors, "Set your favorite actors", "Your favorite actors", 200);
text_setter!(origin, "origin", Field::Origin, "Set where you're from", "Where you're from", 100);
text_setter!(status, "status", Field::Status, "Set your status", "Your status", 100);
text_setter!(hobbies, "hobbies", Field::Hobbies, "Set your hobbies", "Your hobbies", 500);
text_setter!(aboutme, "aboutme", Field::AboutMe, "Set your about me", "A bit about you", 1000);

/// Set your age
#[poise::command(slash_command)]
async fn age(
    ctx: Context<'_>,
    #[description = "Your age"]
    #[min = 13]
    #[max = 150]
    age: u8,
) -> Result<(), Error> {
    set_field(ctx, Field::Age, age.to_string()).await
}

/// Set your birthday
#[poise::command(slash_command)]
async fn birthday(
    ctx: Context<'_>,
    #[description = "Your birthday, as DD-MM-YYYY"]
    #[max_length = 100]
    birthday: String,
) -> Result<(), Error> {
    set_field(ctx, Field::Birthday, format_birthday(&birthday)).await
}

/// Get help with the command
#[poise::command(slash_command)]
async fn help(ctx: Context<'_>) -> Result<(), Error> {
    let setters: Vec<String> = Field::ALL.iter().map(|f| format!("`{}`", f.key())).collect();
    let text = format!(
        "> `/profile create` • Create your profile.\n\
         > `/profile view [user]` • View your profile or someone else's. You can also right-click a user → Apps → **Profile**.\n\
         > `/profile set <field>` • Fill in part of your profile: {}.\n\
         > `/profile delete` • Delete your profile.\n\
         > `/profile help` • Show this message.",
        setters.join(", ")
    );
    let embed = serenity::CreateEmbed::new().colour(COLOUR).title("Profile Help").description(text);
    reply(ctx, embed, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_created_set_and_deleted() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.get(1).unwrap(), None);
        assert!(!s.set(1, Field::Age, "20").unwrap());
        assert!(s.create(1).unwrap());
        assert!(!s.create(1).unwrap());
        assert_eq!(s.get(1).unwrap(), Some(vec![]));

        assert!(s.set(1, Field::Age, "20").unwrap());
        assert!(s.set(1, Field::Age, "21").unwrap());
        assert_eq!(s.get(1).unwrap(), Some(vec![(Field::Age, "21".to_string())]));

        assert!(s.delete(1).unwrap());
        assert!(!s.delete(1).unwrap());
        assert_eq!(s.get(1).unwrap(), None);
    }

    #[test]
    fn birthdays_are_formatted() {
        assert_eq!(format_birthday("23-02-2022"), "Wednesday, 23rd February 2022");
        assert_eq!(format_birthday("01-01-2000"), "Saturday, 1st January 2000");
        assert_eq!(format_birthday("11-11-2011"), "Friday, 11th November 2011");
        assert_eq!(format_birthday("sometime in June"), "sometime in June");
    }
}
