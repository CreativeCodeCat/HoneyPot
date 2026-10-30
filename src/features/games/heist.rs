//! `/heist`: the first member to bet starts a heist and everyone else has `JOIN_TIME`
//! to bet in too. Then a random story plays out, the story's difficulty decides who
//! gets caught, and the survivors get their bet back plus `GAIN_PERCENT`. Once it
//! ends the whole server waits out the cooldown.
//!
//! Bets are held in the economy database until the heist pays out, and refunded if
//! the bot stops before then.
//!
//! A happy pet joins its owner with a free entry of half their bet, and anything it
//! wins goes to its owner.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use chrono::Utc;
use ::serenity::all as serenity;
use rand::Rng;

use crate::core::framework::{command, CreateReply};
use super::super::economy::money;
use super::super::economy::store::EcoError;
use super::super::pets::{self, animal};
use super::super::{Context, Data, Error};
use crate::log;

/// Default seconds the server waits after a heist ends; `/cooldown set heist` changes it
pub const HEIST_COOLDOWN_SECS: i64 = 15 * 60;
const JOIN_TIME: Duration = Duration::from_secs(60);
const CHAPTER_DELAY: Duration = Duration::from_secs(7);
const GAIN_PERCENT: i64 = 30;
const MIN_BET: i64 = 10;
const MAX_BET: i64 = 1000;

// Pet stat changes for joining
const PET_FUN: f64 = 1.0;
const PET_EXP: f64 = 0.5;
const PET_FOOD: f64 = 0.25;

const GOLD: u32 = 0xf1c40f;
const GREEN: u32 = 0x57f287;
const RED: u32 = 0xed4245;

/// Cooldowns are per member, so the server-wide one is kept under this user id.
const SERVER: u64 = 0;

struct Story {
    title: &'static str,
    /// Percent chance each member gets caught
    difficulty: u32,
    /// `(caught)` and `(survivors)` are replaced with the crew; a line naming a
    /// group nobody is in is skipped.
    chapters: &'static [&'static str],
}

const STORIES: &[Story] = &[
    Story {
        title: "Time Heist",
        difficulty: 10,
        chapters: &[
            "Your memory is vague, on the table a small laptop is playing a video: \"My name is The Architect. The bank of Karabraxos is the most secure bank in the universe. You will rob the bank of Karabraxos!\"",
            "Unable to leave their minds blank, (caught) slowly feel their mind being drained as The Teller feeds on their thoughts.",
            "We find ourselves back in the room we started in as consciousness of (survivors) slowly fades again, only to wake up in our beds like nothing at all has happened.",
        ],
    },
    Story {
        title: "Beartraps",
        difficulty: 20,
        chapters: &[
            "Friends! I've got coordinates for a secret stash of bolts, hidden away within the bowels of the elven forest. We should shoe up and give this a go!",
            "Look out, bear traps! (caught) got their legs ripped off!",
            "Dayum, that was a close call for losing a leg. But you've deserved this (survivors)!",
        ],
    },
    Story {
        title: "Vampires?!",
        difficulty: 30,
        chapters: &[
            "Ah, my dear friends! I may have found the heist of a lifetime. Namely the house of count Dracula is believed to be the bolts master! I'm for going now!",
            "It's him! (caught) got slaughtered violently!",
            "That was a close call, I don't think I've been bitten. You? Oh well, (survivors), here's your share! ~Transforms into a bat and flutters off~",
        ],
    },
    Story {
        title: "Cereal",
        difficulty: 40,
        chapters: &[
            "I think we have a much bigger threat on our hands than the cave in... It is half man, half bear, half pig... Don't laugh, I'M SUPER CEREAL!",
            "As the heisters work their way through the tunnels they hear a soft noise from behind them...",
            "Look out! It's ManBearPig! (caught) get dragged off into the darkness.",
            "(survivors) run away. Let's get out of here guys! We can't deal with this alone.",
        ],
    },
];

/// A member, or a member's pet, in the crew.
struct Crew {
    owner_id: u64,
    /// The pet's emoji and name, for pets
    pet: Option<String>,
    bet: i64,
}

impl Crew {
    fn name(&self) -> String {
        match &self.pet {
            Some(pet) => pet.clone(),
            None => format!("<@{}>", self.owner_id),
        }
    }
}

#[derive(Default)]
struct Heist {
    crew: Vec<Crew>,
    /// Under way, so nobody else can join
    running: bool,
}

/// Heists gathering a crew or under way, by guild.
static HEISTS: LazyLock<Mutex<HashMap<u64, Heist>>> = LazyLock::new(Default::default);

fn names(crew: &[&Crew]) -> String {
    crew.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ")
}

/// Fills in the crew, or `None` if the line names a group nobody is in.
fn chapter(line: &str, caught: &[&Crew], survivors: &[&Crew]) -> Option<String> {
    let mut line = line.to_string();
    for (tag, group) in [("(caught)", caught), ("(survivors)", survivors)] {
        if line.contains(tag) {
            if group.is_empty() {
                return None;
            }
            line = line.replace(tag, &names(group));
        }
    }
    Some(line)
}

async fn say_private(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Start or join a heist
#[command(slash_command, guild_only, category = "Games")]
pub async fn heist(
    ctx: Context<'_>,
    #[description = "How much of your wallet to bet"]
    #[min = 10]
    #[max = 1000]
    amount: i64,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get();
    let user_id = ctx.author().id.get();
    let data = ctx.data();

    if !(MIN_BET..=MAX_BET).contains(&amount) {
        return say_private(ctx, format!("❌ Bets must be between {} and {}.", money(MIN_BET), money(MAX_BET))).await;
    }
    let starting = match join(data, guild_id, user_id, amount)? {
        Ok(starting) => starting,
        Err(message) => return say_private(ctx, message).await,
    };

    let mut text = if starting {
        format!(
            "💰 <@{user_id}> is trying to get a team together for some serious heist business! \
             Use `/heist` within {} seconds to join in!",
            JOIN_TIME.as_secs()
        )
    } else {
        format!("💰 <@{user_id}> has joined the heist with {}!", money(amount))
    };
    if let Some(line) = invite_pet(data, guild_id, user_id, amount)? {
        text = format!("{text}\n{line}");
    }
    let embed = serenity::CreateEmbed::new().colour(GOLD).description(text);
    ctx.send(CreateReply::default().embed(embed)).await?;

    if starting {
        let (data, http, channel_id) = (data.clone(), ctx.serenity_context().http.clone(), ctx.channel_id());
        tokio::spawn(async move {
            tokio::time::sleep(JOIN_TIME).await;
            let crew = {
                let mut heists = HEISTS.lock().unwrap();
                let heist = heists.entry(guild_id).or_default();
                heist.running = true;
                std::mem::take(&mut heist.crew)
            };
            if let Err(e) = run(&data, &http, channel_id, guild_id, crew).await {
                log::error(format!("Error running heist in guild {guild_id}: {e}"));
                // Give back any bets that weren't settled
                if let Err(e) = data.economy.lock().unwrap().refund_held(Some((guild_id, "heist"))) {
                    log::error(format!("Error refunding heist bets in guild {guild_id}: {e}"));
                }
            }
            HEISTS.lock().unwrap().remove(&guild_id);
        });
    }
    Ok(())
}

/// Takes the member's bet and adds them to the crew, returning whether they started
/// the heist, or why they can't join. Done in one go so two members can't both start one.
fn join(data: &Data, guild_id: u64, user_id: u64, amount: i64) -> Result<Result<bool, String>, Error> {
    let mut heists = HEISTS.lock().unwrap();
    let heist = heists.get(&guild_id);
    if heist.is_some_and(|heist| heist.running) {
        return Ok(Err("❌ The heist is already under way. Wait for the next one!".into()));
    }
    if heist.is_some_and(|heist| heist.crew.iter().any(|c| c.owner_id == user_id && c.pet.is_none())) {
        return Ok(Err("❌ You have already joined the heist!".into()));
    }
    let starting = heist.is_none();
    if starting {
        let ready = data.cooldowns.lock().unwrap().ready_at(guild_id, SERVER, "heist", Utc::now().timestamp())?;
        if let Some(ready) = ready {
            return Ok(Err(format!("❌ The heat is still on from the last heist. Try again <t:{ready}:R>.")));
        }
    }
    let payment = data.economy.lock().unwrap().hold(guild_id, user_id, "heist", amount)?;
    if let Err(e) = payment {
        return Ok(Err(match e {
            EcoError::NoAccount => "❌ You need an economy account to join. Use `/eco join` first.".to_string(),
            EcoError::NotEnough(available) => {
                format!("❌ You can't bet {}, your wallet only has {}.", money(amount), money(available))
            }
            _ => "❌ Your bet didn't go through.".to_string(),
        }));
    }
    heists.entry(guild_id).or_default().crew.push(Crew { owner_id: user_id, pet: None, bet: amount });
    Ok(Ok(starting))
}

/// Brings the member's pet along if it's happy, and says how that went.
fn invite_pet(data: &Data, guild_id: u64, user_id: u64, bet: i64) -> Result<Option<String>, Error> {
    if !data.is_cog_enabled(guild_id, pets::COG_NAME) {
        return Ok(None);
    }
    let pets = data.pets.lock().unwrap();
    let Some(mut pet) = pets.get(guild_id, user_id)? else { return Ok(None) };
    let name = format!("{} **{}**", animal(&pet.kind).1, pet.name);
    if !pet.is_happy() || pet.food <= PET_FOOD {
        return Ok(Some(format!("{name} doesn't feel like joining the heist... Try `/pet play` or `/pet feed`.")));
    }

    pet.add_fun(PET_FUN);
    pet.add_exp(PET_EXP);
    pet.food -= PET_FOOD;
    pets.save(&pet)?;
    let crew = Crew { owner_id: user_id, pet: Some(name.clone()), bet: bet / 2 };
    HEISTS.lock().unwrap().entry(guild_id).or_default().crew.push(crew);
    Ok(Some(format!("{name} is also joining the heist!")))
}

/// Plays out the story a chapter at a time, then pays the survivors.
async fn run(
    data: &Data,
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    guild_id: u64,
    crew: Vec<Crew>,
) -> Result<(), Error> {
    let story = &STORIES[rand::rng().random_range(0..STORIES.len())];
    let (caught, survivors): (Vec<&Crew>, Vec<&Crew>) =
        crew.iter().partition(|_| rand::rng().random_range(0..100) < story.difficulty);

    let members = crew.iter().filter(|c| c.pet.is_none()).count();
    let mut text = format!("Starting heist **{}** with {members} player{}.", story.title, if members == 1 { "" } else { "s" });
    let embed = |text: &str, colour| serenity::CreateMessage::new().embed(serenity::CreateEmbed::new().colour(colour).description(text));
    let mut message = channel_id.send_message(http, embed(&text, GOLD)).await?;
    for line in story.chapters.iter().filter_map(|line| chapter(line, &caught, &survivors)) {
        tokio::time::sleep(CHAPTER_DELAY).await;
        text = format!("{text}\n\n{line}");
        let edit = serenity::EditMessage::new().embed(serenity::CreateEmbed::new().colour(GOLD).description(&text));
        message.edit(http, edit).await?;
    }

    // Survivors get their bet back plus their winnings; a pet's go to its owner
    let mut winnings: Vec<(u64, i64)> = vec![];
    for survivor in &survivors {
        let payout = survivor.bet + survivor.bet * GAIN_PERCENT / 100;
        match winnings.iter_mut().find(|(id, _)| *id == survivor.owner_id) {
            Some((_, total)) => *total += payout,
            None => winnings.push((survivor.owner_id, payout)),
        }
    }
    // Every member's held bet is settled, with nothing paid to those who lost
    let mut paid = vec![];
    let mut full = vec![];
    for member in crew.iter().filter(|c| c.pet.is_none()) {
        let payout = winnings.iter().find(|(id, _)| *id == member.owner_id).map_or(0, |(_, total)| *total);
        match data.economy.lock().unwrap().settle(guild_id, member.owner_id, "heist", payout)? {
            Ok(()) if payout > 0 => paid.push((member.owner_id, payout)),
            Ok(()) => {}
            Err(_) => full.push(member.owner_id),
        }
    }
    // Can't still be cooling down: that was checked when the heist started
    let _ = data.cooldowns.lock().unwrap().start(guild_id, SERVER, "heist", Utc::now().timestamp())?;

    let (result, colour) = if paid.is_empty() && full.is_empty() {
        ("The heist ended! There are no survivors.".to_string(), RED)
    } else {
        let mut result = format!(
            "The heist ended! Survivors are: {}.",
            paid.iter().map(|(id, total)| format!("<@{id}> (+{})", money(*total))).collect::<Vec<_>>().join(", ")
        );
        for id in full {
            result = format!("{result}\n<@{id}>'s wallet is full, so their share of the loot stays put.");
        }
        (result, GREEN)
    };
    tokio::time::sleep(CHAPTER_DELAY).await;
    channel_id.send_message(http, embed(&result, colour)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_skip_empty_groups() {
        let member = Crew { owner_id: 1, pet: None, bet: 10 };
        let pet = Crew { owner_id: 1, pet: Some("🐱 **Tom**".into()), bet: 5 };
        assert_eq!(chapter("(caught) fell.", &[], &[&member]), None);
        assert_eq!(chapter("(survivors) ran.", &[], &[&member, &pet]).unwrap(), "<@1>, 🐱 **Tom** ran.");
        assert_eq!(chapter("Plain.", &[], &[]).unwrap(), "Plain.");
    }

    #[test]
    fn every_story_names_both_groups() {
        for story in STORIES {
            let text = story.chapters.concat();
            assert!(text.contains("(caught)") && text.contains("(survivors)"), "{}", story.title);
            assert!(story.difficulty < 100, "{}", story.title);
        }
    }
}
