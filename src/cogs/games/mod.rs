//! Games cog: small mini-games. `/hack` is based on the PhantomBot hack system: guess
//! the password the bot picks at random, and crack it to win money for your wallet.

use std::time::Duration;

use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use rand::Rng;

use super::economy::money;
use super::economy::store::{EcoError, Pocket};
use super::{Context, Error};
use scripts::{Script, DENIED, GRANTED};

mod scripts;

/// Money paid into the wallet for a cracked password
const HACK_REWARD: i64 = 2500;
/// Default seconds between /hack attempts for each member; `/cooldown set` changes it
pub const HACK_COOLDOWN_SECS: i64 = 5 * 60;
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

/// Widest line of the terminal, so it doesn't wrap on phones
const MAX_LINE: usize = 54;
/// Inner width of the boxes drawn in the terminal
const BOX_WIDTH: usize = MAX_LINE - 4;
/// Lines printed at a time, and the delay between them
const LINES_PER_STEP: usize = 3;
const STEP_DELAY: Duration = Duration::from_millis(1000);
/// Extra pause between the stages of the hack
const FRAME_DELAY: Duration = Duration::from_millis(2000);
/// Room left in an embed description for the code fences
const EMBED_TEXT_LIMIT: usize = 4000;

const ACCESS_ART: &[&str] = &[
    " █████╗  ██████╗ ██████╗███████╗███████╗███████╗",
    "██╔══██╗██╔════╝██╔════╝██╔════╝██╔════╝██╔════╝",
    "███████║██║     ██║     █████╗  ███████╗███████╗",
    "██╔══██║██║     ██║     ██╔══╝  ╚════██║╚════██║",
    "██║  ██║╚██████╗╚██████╗███████╗███████║███████║",
    "╚═╝  ╚═╝ ╚═════╝ ╚═════╝╚══════╝╚══════╝╚══════╝",
];

/// Timestamps for the log lines, ticking on roughly as fast as the lines are printed.
struct Clock {
    now: DateTime<Utc>,
}

impl Clock {
    fn new() -> Self {
        Self { now: Utc::now() }
    }

    /// Moves the clock on by the pause between stages.
    fn pause(&mut self) {
        self.now += FRAME_DELAY;
    }

    /// A timestamped log line.
    fn log(&mut self, text: &str) -> String {
        self.now += chrono::Duration::milliseconds(rand::rng().random_range(250..400));
        format!("{} {text}", self.now.format("[%H:%M:%S]"))
    }

    /// A timestamped log line with a dotted leader and a status, like `Loading ... [ OK ]`.
    fn task(&mut self, text: &str, status: &str) -> String {
        let status = format!(" [ {status} ]");
        let width = MAX_LINE - 11 - status.chars().count();
        self.log(&format!("{:.<width$}{status}", format!("{text} ")))
    }
}

/// Pads or trims `text` to exactly `width` characters.
fn fit(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    out.extend(std::iter::repeat_n(' ', width - out.chars().count()));
    out
}

/// Breaks a line longer than `MAX_LINE` between words, indenting the rest.
fn wrap(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = line.to_string();
    while rest.chars().count() > MAX_LINE {
        let cut: String = rest.chars().take(MAX_LINE).collect();
        let at = cut.rfind(' ').filter(|&i| i > 12).unwrap_or(cut.len());
        out.push(rest[..at].trim_end().to_string());
        rest = format!("            {}", rest[at..].trim_start());
    }
    out.push(rest);
    out
}

fn centre(text: &str) -> String {
    format!("{}{text}", " ".repeat(BOX_WIDTH.saturating_sub(text.chars().count()) / 2))
}

/// A double-lined box around `rows`, with a divider after the first `header` rows.
fn frame(rows: &[String], header: usize) -> Vec<String> {
    let rule = "═".repeat(BOX_WIDTH + 2);
    let mut out = vec![format!("╔{rule}╗")];
    for (i, row) in rows.iter().enumerate() {
        if i == header && i > 0 {
            out.push(format!("╠{rule}╣"));
        }
        out.push(format!("║ {} ║", fit(row, BOX_WIDTH)));
    }
    out.push(format!("╚{rule}╝"));
    out
}

/// The terminal as code-block embeds, split between lines when it is too long for one.
fn terminal(lines: &[String], colour: u32) -> poise::CreateReply {
    let mut chunks = vec![String::new()];
    for line in lines {
        let chunk = chunks.last_mut().unwrap();
        if !chunk.is_empty() && chunk.chars().count() + line.chars().count() + 1 > EMBED_TEXT_LIMIT {
            chunks.push(String::new());
        }
        let chunk = chunks.last_mut().unwrap();
        chunk.push_str(line);
        chunk.push('\n');
    }
    chunks.into_iter().fold(poise::CreateReply::default(), |reply, chunk| {
        reply.embed(serenity::CreateEmbed::new().colour(colour).description(format!("```\n{chunk}```")))
    })
}

/// The values filled into a script's placeholders.
struct Vars(Vec<(&'static str, String)>);

impl Vars {
    fn fill(&self, text: &str) -> String {
        self.0.iter().fold(text.to_string(), |text, (key, value)| text.replace(key, value))
    }

    /// `key : value` rows with the keys lined up.
    fn pairs(&self, prefix: &str, pairs: &[(&str, &str)]) -> Vec<String> {
        let width = pairs.iter().map(|(key, _)| key.chars().count()).max().unwrap_or(0);
        pairs.iter().map(|(key, value)| format!("{prefix}{key:<width$} : {}", self.fill(value))).collect()
    }
}

/// The lines added in each stage of `script`. Granted scripts end with the big ACCESS banner.
fn play(script: &Script, granted: bool, vars: &Vars, clock: &mut Clock) -> Vec<Vec<String>> {
    let mut frames = vec![];
    let logs = |clock: &mut Clock, lines: &[&str]| lines.iter().map(|line| clock.log(&vars.fill(line))).collect::<Vec<_>>();
    let tasks = |clock: &mut Clock, tasks: &[(&str, &str)]| {
        tasks
            .iter()
            .map(|(text, status)| match *status {
                "" => clock.log(&vars.fill(text)),
                status => clock.task(&vars.fill(text), status),
            })
            .collect::<Vec<_>>()
    };

    let mut header = vec![centre(script.title)];
    header.extend(script.header.iter().map(|row| vars.fill(row)));
    let mut lines = frame(&header, 1);
    lines.push(String::new());
    lines.extend(tasks(clock, script.boot));
    frames.push(lines);

    clock.pause();
    let (heading, intel) = script.intel;
    let mut lines = vec![String::new(), format!("┌─[ {heading} ]"), "│".into()];
    let rows = vars.pairs("", intel);
    let last = rows.len().saturating_sub(1);
    lines.extend(rows.into_iter().enumerate().map(|(i, row)| format!("{}─ {row}", if i == last { "└" } else { "├" })));
    lines.push(String::new());
    frames.push(lines);

    clock.pause();
    let mut lines = logs(clock, script.attempt);
    lines.extend(tasks(clock, script.checks));
    frames.push(lines);

    clock.pause();
    let mut lines = vec![String::new()];
    lines.extend(vars.pairs("    ", script.verdict));
    lines.push(String::new());
    lines.extend(logs(clock, script.notices));
    lines.push(String::new());
    lines.extend(script.progress.iter().map(|(percent, label)| {
        let filled = *percent as usize / 5;
        format!("  [{}{}] {:<4} {label}", "#".repeat(filled), "-".repeat(20 - filled), format!("{percent}%"))
    }));
    lines.push(String::new());
    frames.push(lines);

    clock.pause();
    let rows = vars.pairs("", script.result);
    let mut lines = if granted {
        let mut body = vec![String::new()];
        body.extend(ACCESS_ART.iter().map(|line| centre(line)));
        body.extend([String::new(), centre(script.banner), String::new()]);
        body.extend(rows);
        body.push(String::new());
        frame(&body, 0)
    } else {
        let mut body = vec![centre(script.banner)];
        body.extend(rows);
        frame(&body, 1)
    };
    lines.push(String::new());
    lines.extend(logs(clock, script.closing));
    lines.push(String::new());
    lines.extend(script.message.iter().map(|line| match *line {
        "" => String::new(),
        line => format!("> {}", vars.fill(line)),
    }));
    lines.push(String::new());
    lines.extend(script.end.iter().map(|line| line.to_string()));
    frames.push(lines);
    frames
}

/// Placeholder values for one hack attempt.
fn vars(user: &str, guess: &str, answer: &str, payout: &str) -> Vars {
    let mut rng = rand::rng();
    let [s1, s2, s3]: [u16; 3] = rng.random();
    let hash: u32 = rng.random();
    // The player's own text goes in last, so anything in it that looks like a placeholder stays as typed
    Vars(vec![
        ("{answer}", answer.to_string()),
        ("{stars}", "*".repeat(answer.len())),
        ("{session}", format!("0x{s1:04X}-{s2:04X}-{s3:04X}")),
        ("{short}", format!("{s1:04X}")),
        ("{hash}", format!("0x{:04X}••••••{:04X}", hash >> 16, hash & 0xffff)),
        ("{id}", format!("0x{:08X}", rng.random::<u32>())),
        ("{code}", format!("0x{:04X}", rng.random_range(1..0x100u32))),
        ("{payout}", payout.to_string()),
        ("{guess}", guess.to_string()),
        ("{user}", user.to_string()),
    ])
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
    let cooldown = ctx.data().cooldowns.lock().unwrap().start(guild_id, user_id.get(), "hack", Utc::now().timestamp())?;
    if let Err(ready) = cooldown {
        let reply = format!("❌ The mainframe is on alert. You can hack again <t:{ready}:R>.");
        ctx.send(poise::CreateReply::default().content(reply).ephemeral(true)).await?;
        return Ok(());
    }

    let answer = PASSWORDS[rand::rng().random_range(0..PASSWORDS.len())];
    let cracked = guess.eq_ignore_ascii_case(answer);
    // Backticks would end the code block early
    let guess_shown = guess.replace('`', "'");
    let user = format!("@{}", ctx.author().name.replace('`', "'"));

    let (scripts, payout, colour) = if cracked {
        let paid = ctx.data().economy.lock().unwrap().adjust(guild_id, user_id.get(), Pocket::Wallet, HACK_REWARD)?;
        let payout = match paid {
            Ok(_) => format!("{} has been transferred to your wallet.", money(HACK_REWARD)),
            Err(EcoError::NoAccount) => format!("Join with /eco join to earn {} next time.", money(HACK_REWARD)),
            Err(_) => "Your wallet is full, so the loot stays put.".to_string(),
        };
        (GRANTED, payout, GREEN)
    } else {
        (DENIED, String::new(), RED)
    };
    let script = &scripts[rand::rng().random_range(0..scripts.len())];
    let frames = play(script, cracked, &vars(&user, &guess_shown, answer, &payout), &mut Clock::new());

    // Print a few lines at a time, pausing a little longer between stages
    let mut lines = vec![];
    let mut message = None;
    let last = frames.len() - 1;
    for (i, frame) in frames.into_iter().enumerate() {
        let frame: Vec<String> = frame.iter().flat_map(|line| wrap(line)).collect();
        let steps = frame.chunks(LINES_PER_STEP).count();
        for (j, step) in frame.chunks(LINES_PER_STEP).enumerate() {
            lines.extend_from_slice(step);
            let done = i == last && j == steps - 1;
            let reply = terminal(&lines, if done { colour } else { TERMINAL });
            match &message {
                None => message = Some(ctx.send(reply).await?),
                Some(message) => {
                    tokio::time::sleep(if j == 0 { FRAME_DELAY } else { STEP_DELAY }).await;
                    message.edit(ctx, reply).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_line_up() {
        let lines = frame(&[centre("ACCESS DENIED"), "USER       : @someone".into(), "x".repeat(200)], 1);
        assert!(lines.iter().all(|l| l.chars().count() == BOX_WIDTH + 4));
    }

    #[test]
    fn every_script_fits_and_reveals_the_answer() {
        let vars = vars(&format!("@{}", "u".repeat(32)), &"x".repeat(MAX_GUESS_LEN), "changeme", "paid");
        for (scripts, granted) in [(DENIED, false), (GRANTED, true)] {
            for script in scripts {
                let frames = play(script, granted, &vars, &mut Clock::new());
                let lines: Vec<String> = frames.concat().iter().flat_map(|l| wrap(l)).collect();
                assert!(lines.iter().map(|l| l.chars().count() + 1).sum::<usize>() + 20 < 6000, "{}", script.title);
                assert!(lines.iter().all(|l| l.chars().count() <= MAX_LINE), "{}", script.title);
                let text = lines.join("\n");
                assert!(text.contains("changeme"), "{}", script.title);
                assert!(!text.contains('{'), "{}", script.title);
                assert_eq!(text.contains("paid"), granted, "{}", script.title);
            }
        }
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
