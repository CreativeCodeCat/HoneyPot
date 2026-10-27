//! Pets cog: a Discord version of the PhantomBot tamagotchi. Members buy a pet with
//! economy money, keep it fed and happy, and play, hug, go on playdates and battle.

pub mod store;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use poise::serenity_prelude as serenity;
use rand::Rng;

use super::economy::money;
use super::economy::store::{EcoError, Pocket};
use super::{Context, Data, Error};
use crate::log;
use store::{Alert, Mood, Pet, FOOD_TTL_HOURS, HAPPY_FUN, LEVEL_CAP};

pub const COG_NAME: &str = "pets";

const PET_PRICE: i64 = 1000;
const TREAT_PRICE: i64 = 5;
/// Longest pet name, as in the original
const MAX_NAME_LEN: usize = 13;

const GREEN: u32 = 0x57f287;
const PURPLE: u32 = 0x9b59b6;
const RED: u32 = 0xed4245;

/// (key, display name, emoji)
const ANIMALS: &[(&str, &str, &str)] = &[
    ("cat", "Cat", "🐱"),
    ("dog", "Dog", "🐶"),
    ("pig", "Pig", "🐷"),
    ("horse", "Horse", "🐴"),
    ("panda", "Giant Panda", "🐼"),
    ("saber_tooth_tiger", "Saber Tooth Tiger", "🐯"),
    ("wolf", "Wolf", "🐺"),
    ("t_rex", "T Rex", "🦖"),
    ("bat", "Bat", "🦇"),
    ("mouse", "Mouse", "🐭"),
    ("phoenix", "Phoenix", "🔥"),
    ("fairy", "Fairy", "🧚"),
    ("house_elf", "House Elf", "🧝"),
    ("dragon", "Dragon", "🐉"),
    ("duck", "Duck", "🦆"),
    ("ant", "Ant", "🐜"),
    ("penguin", "Penguin", "🐧"),
    ("lizard", "Lizard", "🦎"),
    ("snake", "Snake", "🐍"),
    ("spider", "Spider", "🕷️"),
];

/// `{name}` is the pet
const PLAY_SPOTS: &[&str] = &[
    "{name} runs into the wild to have a bit of fun with the rabbits!",
    "{name} runs into the park to play with the other animals!",
    "{name} runs into the wild to have a bit of fun with the dogs!",
    "{name} runs into the wild to have a bit of fun with the pandas!",
    "{name} runs into the tall grass to roll around!",
    "{name} runs into the puddle of water to splash around!",
    "{name} waddles around in circles making noises!",
    "{name} jumps into a mud pit to roll around with the pigs!",
    "{name} runs down a big hill to spin around in circles!",
    "{name} scratches itself then runs off into the sunset!",
];

/// `{guest}` is the other member's pet, `{host}` is the caller's
const PLAYDATE_SPOTS: &[&str] = &[
    "{guest} suggests going to the park. {host} agrees and they're having fun!",
    "{guest} is coming over to {host} to play!",
    "{host} is coming over to {guest} to play!",
];

// Stat changes, from the original scripts
const PLAY: Effect = Effect { fun: 0.1, exp: 1.5, food: 1.0 };
const HUG: Effect = Effect { fun: 0.1, exp: 0.25, food: 0.1 };
const PLAYDATE: Effect = Effect { fun: 3.0, exp: 0.25, food: 0.5 };
const BATTLE_FOOD: f64 = 2.0;
const BATTLE_EXP: f64 = 0.75;
const BATTLE_WIN_FUN: f64 = 4.0;
const BATTLE_LOSE_FUN: f64 = 2.0;
const BATTLE_ACCEPT_TIME: Duration = Duration::from_secs(60);

struct Effect {
    fun: f64,
    exp: f64,
    food: f64,
}

impl Effect {
    fn apply(&self, pet: &mut Pet) {
        pet.add_fun(self.fun);
        pet.add_exp(self.exp);
        pet.food -= self.food;
    }
}

fn animal(kind: &str) -> (&'static str, &'static str) {
    ANIMALS.iter().find(|(k, _, _)| *k == kind).map(|(_, name, emoji)| (*name, *emoji)).unwrap_or(("Pet", "🐾"))
}

/// A random entry, never the same as the last one picked from this list.
fn pick(list: &[&'static str], last: &AtomicUsize) -> &'static str {
    let mut index = rand::rng().random_range(0..list.len());
    if list.len() > 1 && index == last.load(Ordering::Relaxed) {
        index = (index + 1) % list.len();
    }
    last.store(index, Ordering::Relaxed);
    list[index]
}

fn format_age(seconds: i64) -> String {
    let (days, hours, minutes) = (seconds / 86400, seconds % 86400 / 3600, seconds % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" }),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

fn guild_id(ctx: Context<'_>) -> u64 {
    ctx.guild_id().expect("pet commands are guild_only").get()
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

async fn say_embed(ctx: Context<'_>, colour: u32, text: impl Into<String>) -> Result<(), Error> {
    let embed = serenity::CreateEmbed::new().colour(colour).description(text);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

fn no_pet_message() -> String {
    format!("❌ You don't have a pet yet! Buy one with `/pet buy` for {}.", money(PET_PRICE))
}

/// The caller's pet, or a reply explaining they don't have one.
async fn own_pet(ctx: Context<'_>) -> Result<Option<Pet>, Error> {
    let pet = ctx.data().pets.lock().unwrap().get(guild_id(ctx), ctx.author().id.get())?;
    if pet.is_none() {
        say_private(ctx, no_pet_message()).await?;
    }
    Ok(pet)
}

/// Another member's pet, or a reply explaining why there isn't one to use.
async fn other_pet(ctx: Context<'_>, user: &serenity::User) -> Result<Option<Pet>, Error> {
    if user.id == ctx.author().id {
        say_private(ctx, "❌ Pick someone other than yourself.").await?;
        return Ok(None);
    }
    let pet = ctx.data().pets.lock().unwrap().get(guild_id(ctx), user.id.get())?;
    if pet.is_none() {
        say_private(ctx, format!("❌ <@{}> doesn't have a pet!", user.id)).await?;
    }
    Ok(pet)
}

/// Replies and returns false if the pet doesn't have more than `needed` treats.
async fn check_food(ctx: Context<'_>, pet: &Pet, needed: f64) -> Result<bool, Error> {
    if pet.food > needed {
        return Ok(true);
    }
    say_private(ctx, format!("❌ {} is too hungry for that. Give them some treats with `/pet feed`.", pet.name)).await?;
    Ok(false)
}

/// Takes `amount` from the caller's wallet, or replies with why it couldn't.
async fn charge(ctx: Context<'_>, amount: i64) -> Result<bool, Error> {
    let result = ctx.data().economy.lock().unwrap().adjust(guild_id(ctx), ctx.author().id.get(), Pocket::Wallet, -amount)?;
    let message = match result {
        Ok(_) => return Ok(true),
        Err(EcoError::NoAccount) => "❌ You need an economy account to pay for that. Use `/eco join` first.".to_string(),
        Err(EcoError::NotEnough(available)) => {
            format!("❌ That costs {}, but your wallet only has {}.", money(amount), money(available))
        }
        Err(_) => "❌ That payment didn't go through.".to_string(),
    };
    say_private(ctx, message).await?;
    Ok(false)
}

fn save(ctx: Context<'_>, pets: &[&Pet]) -> Result<(), Error> {
    let store = ctx.data().pets.lock().unwrap();
    for pet in pets {
        store.save(pet)?;
    }
    Ok(())
}

/// Pet commands
#[poise::command(
    slash_command,
    prefix_command,
    guild_only,
    category = "Pets",
    subcommands("buy", "status", "feed", "play", "hug", "playdate", "battle", "release"),
    subcommand_required
)]
pub async fn pet(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

async fn autocomplete_animal(_ctx: Context<'_>, partial: &str) -> Vec<serenity::AutocompleteChoice> {
    let partial = partial.to_lowercase();
    ANIMALS
        .iter()
        .filter(|(key, name, _)| key.contains(&partial) || name.to_lowercase().contains(&partial))
        .map(|(key, name, emoji)| serenity::AutocompleteChoice::new(format!("{emoji} {name}"), *key))
        .collect()
}

#[derive(Clone, Copy, poise::ChoiceParameter)]
enum Gender {
    Male,
    Female,
}

/// Buy a pet
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn buy(
    ctx: Context<'_>,
    #[description = "Kind of pet"]
    #[autocomplete = "autocomplete_animal"]
    kind: String,
    #[description = "Its name (up to 13 characters)"] name: String,
    #[description = "Male or female (random if not chosen)"] gender: Option<Gender>,
) -> Result<(), Error> {
    let kind = kind.trim().to_lowercase().replace(' ', "_");
    let Some((kind, animal_name, emoji)) = ANIMALS.iter().find(|(key, name, _)| *key == kind || name.to_lowercase().replace(' ', "_") == kind)
    else {
        let kinds: Vec<&str> = ANIMALS.iter().map(|(_, name, _)| *name).collect();
        return say_private(ctx, format!("❌ That isn't a kind of pet. Choose from: {}.", kinds.join(", "))).await;
    };
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return say_private(ctx, format!("❌ Pet names must be 1 to {MAX_NAME_LEN} characters long.")).await;
    }
    if ctx.data().pets.lock().unwrap().get(guild_id(ctx), ctx.author().id.get())?.is_some() {
        return say_private(ctx, "❌ You already have a pet. You can let it go with `/pet release` and buy a new one.").await;
    }
    if !charge(ctx, PET_PRICE).await? {
        return Ok(());
    }

    let male = match gender {
        Some(gender) => matches!(gender, Gender::Male),
        None => rand::rng().random_bool(0.5),
    };
    let pet = Pet::new(guild_id(ctx), ctx.author().id.get(), kind, name, Utc::now().timestamp(), male);
    ctx.data().pets.lock().unwrap().create(&pet)?;

    let sex = if male { "male" } else { "female" };
    say_embed(
        ctx,
        GREEN,
        format!(
            "🥚 <@{}> receives an egg in the mail. After a few minutes of rubbing to get it warm, a tiny {sex} {emoji} {animal_name} called **{name}** cracks the shell open and peers into the world.",
            ctx.author().id
        ),
    )
    .await
}

/// Check on your pet, or someone else's
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn status(
    ctx: Context<'_>,
    #[description = "Whose pet to check (defaults to yours)"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let user = user.as_ref().unwrap_or(ctx.author());
    let Some(pet) = ctx.data().pets.lock().unwrap().get(guild_id(ctx), user.id.get())? else {
        let text = if user.id == ctx.author().id { no_pet_message() } else { format!("❌ <@{}> doesn't have a pet!", user.id) };
        return say_private(ctx, text).await;
    };

    let (animal_name, emoji) = animal(&pet.kind);
    let mood = match pet.mood() {
        Mood::Happy => "😄 Happy".to_string(),
        Mood::Meh => format!("😐 A bit meh. Maybe you should play with {}.", pet.pronoun()),
        Mood::Sad => format!("😢 Sad! You should play with {}!", pet.pronoun()),
    };
    let hours_left = (pet.food * FOOD_TTL_HOURS * 3600.0) as i64;
    let embed = serenity::CreateEmbed::new()
        .title(format!("{emoji} {}", pet.name))
        .description(format!("<@{}>'s {} {animal_name}", user.id, if pet.male { "male" } else { "female" }))
        .colour(PURPLE)
        .field("Age", format_age(Utc::now().timestamp() - pet.born), true)
        .field("Level", pet.level().to_string(), true)
        .field("Treats", format!("{:.0} / {:.0}", pet.food.ceil(), pet.max_food()), true)
        .field("Fun", format!("{:.1} / {:.0}\n*happy at {HAPPY_FUN:.0}*", pet.fun, LEVEL_CAP / 2.0), true)
        .field("Experience", format!("{:.2} / {LEVEL_CAP:.0}", pet.exp), true)
        .field("\u{200b}", "\u{200b}", true)
        .field("Mood", mood, false)
        .field("Food lasts", format!("about {}", format_age(hours_left)), false);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Buy treats for your pet (each lasts 24 hours)
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn feed(
    ctx: Context<'_>,
    #[description = "How many treats to give"]
    #[min = 1]
    #[max = 200]
    amount: i64,
) -> Result<(), Error> {
    let Some(mut pet) = own_pet(ctx).await? else { return Ok(()) };
    if amount < 1 {
        return say_private(ctx, "❌ Give at least one treat.").await;
    }
    if pet.food + amount as f64 > pet.max_food() {
        let room = (pet.max_food() - pet.food).floor().max(0.0);
        return say_private(ctx, format!("❌ {} can't carry that many treats! There's room for {room:.0} more.", pet.name)).await;
    }
    if !charge(ctx, amount * TREAT_PRICE).await? {
        return Ok(());
    }

    pet.food += amount as f64;
    save(ctx, &[&pet])?;
    say_embed(
        ctx,
        GREEN,
        format!(
            "🍖 You gave **{}** {amount} treat{} for {}, enough for another {}.",
            pet.name,
            if amount == 1 { "" } else { "s" },
            money(amount * TREAT_PRICE),
            format_age(amount * FOOD_TTL_HOURS as i64 * 3600)
        ),
    )
    .await
}

/// Let your pet go and play
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn play(ctx: Context<'_>) -> Result<(), Error> {
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    let Some(mut pet) = own_pet(ctx).await? else { return Ok(()) };
    if !check_food(ctx, &pet, PLAY.food).await? {
        return Ok(());
    }

    PLAY.apply(&mut pet);
    save(ctx, &[&pet])?;
    say_embed(ctx, PURPLE, format!("{} {}", animal(&pet.kind).1, pick(PLAY_SPOTS, &LAST).replace("{name}", &format!("**{}**", pet.name)))).await
}

/// Send your pet to hug someone
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn hug(ctx: Context<'_>, #[description = "Who to hug"] user: serenity::User) -> Result<(), Error> {
    let Some(mut pet) = own_pet(ctx).await? else { return Ok(()) };
    if user.id == ctx.author().id {
        return say_private(ctx, "❌ Pick someone other than yourself.").await;
    }
    if !pet.is_happy() {
        return say_private(ctx, format!("❌ {} doesn't feel like hugging... Try `/pet play` first.", pet.name)).await;
    }
    if !check_food(ctx, &pet, HUG.food).await? {
        return Ok(());
    }

    HUG.apply(&mut pet);
    let target = ctx.data().pets.lock().unwrap().get(guild_id(ctx), user.id.get())?;
    let text = match target {
        Some(mut target) => {
            target.add_fun(HUG.fun);
            save(ctx, &[&pet, &target])?;
            format!("**{}** runs over to **{}** and hugs {}! ❤️", pet.name, target.name, target.pronoun())
        }
        None => {
            save(ctx, &[&pet])?;
            format!("**{}** runs over to <@{}> and hugs them! With love from <@{}> ❤️", pet.name, user.id, ctx.author().id)
        }
    };
    say_embed(ctx, PURPLE, text).await
}

/// Arrange a playdate with someone else's pet
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn playdate(ctx: Context<'_>, #[description = "Whose pet to play with"] user: serenity::User) -> Result<(), Error> {
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    let Some(mut host) = own_pet(ctx).await? else { return Ok(()) };
    let Some(mut guest) = other_pet(ctx, &user).await? else { return Ok(()) };
    if !check_food(ctx, &host, PLAYDATE.food).await? {
        return Ok(());
    }
    if guest.food < 1.0 + PLAYDATE.food {
        return say_private(ctx, format!("❌ {}'s food stash is too low...", guest.name)).await;
    }

    PLAYDATE.apply(&mut host);
    PLAYDATE.apply(&mut guest);
    save(ctx, &[&host, &guest])?;
    let text = pick(PLAYDATE_SPOTS, &LAST)
        .replace("{guest}", &format!("**{}**", guest.name))
        .replace("{host}", &format!("**{}**", host.name));
    say_embed(ctx, PURPLE, format!("🎈 {text}")).await
}

/// Challenge someone's pet to a battle
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn battle(ctx: Context<'_>, #[description = "Who to challenge"] user: serenity::User) -> Result<(), Error> {
    let Some(challenger) = own_pet(ctx).await? else { return Ok(()) };
    let Some(opponent) = other_pet(ctx, &user).await? else { return Ok(()) };
    for pet in [&challenger, &opponent] {
        if !pet.is_happy() {
            return say_private(ctx, format!("❌ {} is not in the mood to fight.", pet.name)).await;
        }
        if pet.food.floor() <= BATTLE_FOOD + 1.0 {
            return say_private(ctx, format!("❌ {} does not have enough food to survive this.", pet.name)).await;
        }
    }

    // Ask the opponent's owner to accept with a button
    let (accept_id, decline_id) = (format!("{}pet_accept", ctx.id()), format!("{}pet_decline", ctx.id()));
    let buttons = |disabled: bool| {
        vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&accept_id).label("Accept").style(serenity::ButtonStyle::Success).emoji('⚔').disabled(disabled),
            serenity::CreateButton::new(&decline_id).label("Decline").style(serenity::ButtonStyle::Secondary).disabled(disabled),
        ])]
    };
    let challenge = serenity::CreateEmbed::new().colour(RED).description(format!(
        "⚔️ **{}** challenges **{}** to a battle!\n<@{}>, do you accept? (expires <t:{}:R>)",
        challenger.name,
        opponent.name,
        user.id,
        Utc::now().timestamp() + BATTLE_ACCEPT_TIME.as_secs() as i64
    ));
    let reply = ctx
        .send(poise::CreateReply::default().content(format!("<@{}>", user.id)).embed(challenge.clone()).components(buttons(false)))
        .await?;

    let deadline = Instant::now() + BATTLE_ACCEPT_TIME;
    let accepted = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (accept, decline) = (accept_id.clone(), decline_id.clone());
        let Some(press) = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .filter(move |i| i.data.custom_id == accept || i.data.custom_id == decline)
            .timeout(remaining)
            .await
        else {
            break None;
        };
        if press.user.id != user.id {
            let not_yours = serenity::CreateInteractionResponseMessage::new().content("This battle isn't for you.").ephemeral(true);
            press.create_response(ctx, serenity::CreateInteractionResponse::Message(not_yours)).await?;
            continue;
        }
        press.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
        break Some(press.data.custom_id == accept_id);
    };

    let finish = |text: String, colour: u32| {
        poise::CreateReply::default().content("").embed(serenity::CreateEmbed::new().colour(colour).description(text)).components(buttons(true))
    };
    match accepted {
        None | Some(false) => {
            // Being stood up is a little disappointing
            if let Some(mut pet) = ctx.data().pets.lock().unwrap().get(guild_id(ctx), ctx.author().id.get())? {
                pet.add_fun(-1.0);
                save(ctx, &[&pet])?;
            }
            let why = if accepted.is_none() { "did not accept the battle" } else { "declined the battle" };
            reply.edit(ctx, finish(format!("<@{}> {why}... so it's cancelled.", user.id), GREY)).await?;
            return Ok(());
        }
        Some(true) => {}
    }

    reply.edit(ctx, finish(format!("⚔️ The battle today is **{}** vs **{}**! Ready?!", challenger.name, opponent.name), RED)).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Reload both pets: time may have passed while waiting for the accept
    let (Some(mut challenger), Some(mut opponent)) = ({
        let store = ctx.data().pets.lock().unwrap();
        (store.get(guild_id(ctx), ctx.author().id.get())?, store.get(guild_id(ctx), user.id.get())?)
    }) else {
        reply.edit(ctx, finish("The battle was called off: one of the pets is gone.".to_string(), GREY)).await?;
        return Ok(());
    };

    // Higher level gives up to a ±10 in 25 edge, as in the original
    let edge = (challenger.exp - opponent.exp).clamp(-10.0, 10.0);
    let challenger_wins = (rand::rng().random_range(0..=24) as f64) <= 10.0 + edge;
    let (winner, loser) = if challenger_wins { (&mut challenger, &mut opponent) } else { (&mut opponent, &mut challenger) };
    winner.add_exp(BATTLE_EXP * 2.0);
    winner.add_fun(BATTLE_WIN_FUN);
    loser.add_exp(BATTLE_EXP);
    loser.add_fun(-BATTLE_LOSE_FUN);
    winner.food -= BATTLE_FOOD;
    loser.food -= BATTLE_FOOD;
    let text = format!(
        "🏆 **{}** wiped the floor with **{}**... The pets return home to gloat about their battle scars.",
        winner.name, loser.name
    );
    save(ctx, &[&challenger, &opponent])?;
    reply.edit(ctx, finish(text, GREEN)).await?;
    Ok(())
}

/// Let your pet go
#[poise::command(slash_command, prefix_command, guild_only, category = "Pets")]
async fn release(ctx: Context<'_>) -> Result<(), Error> {
    let Some(pet) = own_pet(ctx).await? else { return Ok(()) };
    ctx.data().pets.lock().unwrap().remove(guild_id(ctx), ctx.author().id.get())?;
    say_embed(ctx, GREY, format!("👋 You let **{}** go. You can buy a new pet with `/pet buy`.", pet.name)).await
}

const GREY: u32 = 0x99aab5;

/// Applies hunger and boredom every 10 minutes (paused in servers with the cog
/// disabled), and DMs owners when their pet is hungry or has died.
pub async fn needs_loop(ctx: serenity::Context, data: Data) {
    let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
    loop {
        interval.tick().await;
        let alerts = data.pets.lock().unwrap().decay_all(Utc::now().timestamp(), |guild| data.is_cog_enabled(guild, COG_NAME));
        let alerts = match alerts {
            Ok(alerts) => alerts,
            Err(e) => {
                log::error(format!("[PETS] Error updating pets: {e}"));
                continue;
            }
        };

        for alert in alerts {
            let (pet, text) = match alert {
                Alert::Hungry(pet) => {
                    let text = format!("🍖 **{}** needs food! Use `/pet feed` to give {} some treats.", pet.name, pet.pronoun());
                    (pet, text)
                }
                Alert::Died(pet) => {
                    let text = format!(
                        "💀 **{}** has died of hunger. You can buy a new pet with `/pet buy` for {}.",
                        pet.name,
                        money(PET_PRICE)
                    );
                    (pet, text)
                }
            };
            let guild_name = ctx.cache.guild(pet.guild_id).map(|g| g.name.clone()).unwrap_or_default();
            let dm = serenity::CreateMessage::new().content(format!("{text}\n*({guild_name})*"));
            // Owners with DMs closed just don't get the message
            let _ = serenity::UserId::new(pet.owner_id).direct_message(&ctx, dm).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_ages() {
        assert_eq!(format_age(59), "0 minutes");
        assert_eq!(format_age(60), "1 minute");
        assert_eq!(format_age(3 * 3600 + 120), "3h 2m");
        assert_eq!(format_age(2 * 86400 + 5 * 3600), "2d 5h");
    }

    #[test]
    fn picks_never_repeat() {
        let last = AtomicUsize::new(usize::MAX);
        let mut previous = "";
        for _ in 0..200 {
            let next = pick(PLAY_SPOTS, &last);
            assert_ne!(next, previous);
            previous = next;
        }
    }
}
