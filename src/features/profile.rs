//! Profile cog: members make an "about me" profile with `/profile create`, fill it in with
//! `/profile set ...`, and anyone can see it with `/profile view` or the "Profile" user
//! context menu. Profiles belong to the user, so they're the same in every server.

use ::serenity::all as serenity;

use crate::core::framework::{command, CreateReply};
use super::{Context, Error};

pub use crate::database::repositories::profiles::*;

const COLOUR: u32 = 0x2b2d31;

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
    ctx.send(CreateReply::default().embed(embed).ephemeral(ephemeral)).await?;
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
#[command(
    slash_command,
    category = "Profile",
    subcommands("view", "create", "delete", "set", "help"),
    subcommand_required
)]
pub async fn profile(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// View a user's profile
#[command(slash_command)]
async fn view(
    ctx: Context<'_>,
    #[description = "The user whose profile you want to see (default: you)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    show(ctx, user.unwrap_or_else(|| ctx.author().clone())).await
}

/// View this user's profile
#[command(context_menu_command = "Profile", category = "Profile")]
pub async fn profile_menu(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    show(ctx, user).await
}

/// Create your profile
#[command(slash_command)]
async fn create(ctx: Context<'_>) -> Result<(), Error> {
    if !ctx.data().profiles.lock().unwrap().create(ctx.author().id.get())? {
        return reply(ctx, notice("❌ Profile Already Exists", "You already have a profile. Fill it in with `/profile set`."), true).await;
    }
    reply(ctx, notice("✅ Profile Created", "Your profile has been created. Fill it in with `/profile set`."), true).await
}

/// Delete your profile
#[command(slash_command)]
async fn delete(ctx: Context<'_>) -> Result<(), Error> {
    if !ctx.data().profiles.lock().unwrap().delete(ctx.author().id.get())? {
        return reply(ctx, notice("❌ No Profile Found", "You don't have a profile to delete."), true).await;
    }
    reply(ctx, notice("✅ Profile Deleted", "Your profile has been deleted."), true).await
}

/// Fill in your profile
#[command(
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
        #[command(slash_command, rename = $name)]
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
#[command(slash_command)]
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
#[command(slash_command)]
async fn birthday(
    ctx: Context<'_>,
    #[description = "Your birthday, as DD-MM-YYYY"]
    #[max_length = 100]
    birthday: String,
) -> Result<(), Error> {
    set_field(ctx, Field::Birthday, format_birthday(&birthday)).await
}

/// Get help with the command
#[command(slash_command)]
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
