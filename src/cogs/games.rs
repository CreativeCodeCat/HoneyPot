//! Games cog: small mini-games. `/hack` is based on the PhantomBot hack system: guess
//! the password the bot picks at random, and crack it to win money for your wallet.

use std::time::Duration;

use chrono::{DateTime, Utc};
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

/// Widest line of the terminal, so it doesn't wrap on phones
const MAX_LINE: usize = 54;
/// Inner width of the boxes drawn in the terminal
const BOX_WIDTH: usize = MAX_LINE - 4;
/// Delay between frames of the terminal animation
const FRAME_DELAY: Duration = Duration::from_millis(1500);
/// Room left in an embed description for the code fences
const EMBED_TEXT_LIMIT: usize = 4000;

const DENIED_QUIPS: &[&str] = &[
    "Impressive aesthetics. Questionable password.\n> Even the mainframe is disappointed.",
    "The firewall didn't even need to wake up for that one.",
    "Bold strategy. It did not pay off.",
    "The mainframe has logged this attempt under \"comedy\".",
];

const ACCESS_ART: &[&str] = &[
    " █████╗  ██████╗ ██████╗███████╗███████╗███████╗",
    "██╔══██╗██╔════╝██╔════╝██╔════╝██╔════╝██╔════╝",
    "███████║██║     ██║     █████╗  ███████╗███████╗",
    "██╔══██║██║     ██║     ██╔══╝  ╚════██║╚════██║",
    "██║  ██║╚██████╗╚██████╗███████╗███████║███████║",
    "╚═╝  ╚═╝ ╚═════╝ ╚═════╝╚══════╝╚══════╝╚══════╝",
];

fn pick<'a>(items: &[&'a str]) -> &'a str {
    items[rand::rng().random_range(0..items.len())]
}

/// Timestamps for the log lines: each line ticks the clock on by a fraction of a second, and
/// each frame starts where the animation will really be when it is shown.
struct Clock {
    start: DateTime<Utc>,
    now: DateTime<Utc>,
}

impl Clock {
    fn new() -> Self {
        let now = Utc::now();
        Self { start: now, now }
    }

    fn frame(&mut self, index: u32) {
        self.now = self.start + FRAME_DELAY * index;
    }

    /// A timestamped log line.
    fn log(&mut self, text: &str) -> String {
        self.now += chrono::Duration::milliseconds(rand::rng().random_range(180..260));
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
    let last = chunks.len() - 1;
    let mut reply = poise::CreateReply::default();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let mut embed = serenity::CreateEmbed::new().colour(colour).description(format!("```\n{chunk}```"));
        if i == 0 {
            embed = embed.title("💻 Hack");
        }
        if i == last {
            embed = embed.footer(serenity::CreateEmbedFooter::new("This is a fictional terminal animation, not a real system intrusion."));
        }
        reply = reply.embed(embed);
    }
    reply
}

/// What the player sees: the user, their guess and the password the bot picked.
struct Attempt<'a> {
    user: &'a str,
    guess: &'a str,
    answer: &'a str,
    session: [u16; 3],
}

/// Lines added in each frame of a failed hack.
fn denied(a: &Attempt, clock: &mut Clock) -> Vec<Vec<String>> {
    let [s1, s2, s3] = a.session;
    let mut frames = vec![];

    clock.frame(0);
    let mut lines = frame(
        &[
            centre("░▒▓  HONEYPOT :: CYBER OPERATIONS TERMINAL  ▓▒░"),
            format!("SESSION ID : 0x{s1:04X}-{s2:04X}-{s3:04X}"),
            "CLEARANCE  : UNAUTHORIZED".into(),
        ],
        1,
    );
    lines.push(String::new());
    lines.push(clock.log("INITIALIZING SECURE SHELL..."));
    lines.push(clock.task("Loading cryptographic modules", "OK"));
    lines.push(clock.task("Establishing encrypted tunnel", "OK"));
    frames.push(lines);

    clock.frame(1);
    let mut lines = vec![
        clock.task("Resolving MAINFRAME://UNKNOWN", "OK"),
        clock.task("Analyzing network perimeter", "DONE"),
    ];
    lines.extend(
        [
            "",
            "┌─[ SYSTEM RECONNAISSANCE ]",
            "│",
            "├─ Target Status      : ONLINE",
            "├─ Firewall           : ACTIVE",
            "├─ Encryption         : AES-256 (SIMULATED)",
            "├─ Authentication     : MULTI-FACTOR",
            "├─ Intrusion Detection: ENABLED",
            "└─ Security Rating    : ██████████ CRITICAL",
            "",
        ]
        .map(String::from),
    );
    frames.push(lines);

    clock.frame(2);
    frames.push(vec![
        clock.log(&format!("{} has entered the session.", a.user)),
        clock.log("Authentication challenge detected."),
        clock.log(&format!("Submitting candidate credential: \"{}\"", a.guess)),
        clock.log("Validating credential signature..."),
        clock.log("Comparing authentication hashes..."),
        clock.log("Verifying access token..."),
        clock.log("Cross-checking security clearance..."),
    ]);

    clock.frame(3);
    let mut lines: Vec<String> = [
        "",
        "    ACCESS TOKEN:  0x00000000",
        "    HASH MATCH:    FALSE",
        "    SESSION KEY:   INVALID",
        "    AUTH STATUS:   DENIED",
        "",
    ]
    .map(String::from)
    .into();
    lines.push(clock.log("WARNING: Authentication mismatch."));
    lines.push(clock.log("WARNING: Privilege exceeds clearance."));
    lines.push(clock.log("Starting countermeasure sequence..."));
    lines.extend(
        [
            "",
            "  [##########----------] 50%  Analyzing request",
            "  [###############-----] 75%  Auditing credentials",
            "  [####################] 100% Security audit complete",
            "",
        ]
        .map(String::from),
    );
    frames.push(lines);

    clock.frame(4);
    let mut lines = frame(
        &[
            centre("ACCESS DENIED"),
            format!("USER       : {}", a.user),
            format!("INPUT      : {}", a.guess),
            "RESULT     : AUTHENTICATION FAILURE".into(),
            format!("ERROR CODE : AUTH_ERR_0x{:04X}", rand::rng().random_range(1..0x100u32)),
            "CLEARANCE  : INSUFFICIENT".into(),
        ],
        1,
    );
    lines.push(String::new());
    lines.push(clock.log("Performing final diagnostic..."));
    lines.push(clock.log("Diagnostic complete."));
    lines.extend([
        String::new(),
        "> SECURITY REPORT:".into(),
        format!("> The password \"{}\" was incorrect.", a.guess),
        format!("> The correct password was: \"{}\"", a.answer),
        String::new(),
        "> SYSTEM MESSAGE:".into(),
        format!("> {}", pick(DENIED_QUIPS)),
        String::new(),
        "[SESSION TERMINATED]".into(),
        "[CONNECTION CLOSED] ░▒▓█".into(),
    ]);
    frames.push(lines);
    frames
}

/// Lines added in each frame of a successful hack. `payout` says what happened to the reward.
fn granted(a: &Attempt, payout: &str, clock: &mut Clock) -> Vec<Vec<String>> {
    let [s1, s2, s3] = a.session;
    let mut frames = vec![];

    clock.frame(0);
    let mut lines = frame(
        &[
            centre("░▒▓█  HONEYPOT :: CYBER OPERATIONS TERMINAL  █▓▒░"),
            format!("SESSION ID     : 0x{s1:04X}-{s2:04X}-{s3:04X}"),
            "SECURITY LEVEL : ROOT".into(),
            "NETWORK        : MAINFRAME://CLASSIFIED".into(),
            "STATUS         : AUTHENTICATED".into(),
        ],
        1,
    );
    lines.push(String::new());
    lines.push(clock.log("BOOTSTRAPPING CYBER OPS ENVIRONMENT..."));
    lines.push(clock.task("Loading cryptographic libraries", "OK"));
    lines.push(clock.task("Initializing secure comms", "OK"));
    lines.push(clock.task("Establishing encrypted session", "OK"));
    lines.push(clock.task("Connecting to auth gateway", "OK"));
    frames.push(lines);

    clock.frame(1);
    frames.push(
        [
            "",
            "┌─[ TARGET INTELLIGENCE ]",
            "│",
            "├─ HOSTNAME       : MAINFRAME-01",
            "├─ FIREWALL       : ACTIVE",
            "├─ ENCRYPTION     : AES-256",
            "├─ AUTHENTICATION : PASSWORD-BASED",
            "├─ SECURITY LEVEL : MAXIMUM",
            "└─ SESSION STATUS : AWAITING CREDENTIALS",
            "",
        ]
        .map(String::from)
        .into(),
    );

    clock.frame(2);
    frames.push(vec![
        clock.log(&format!("{} has entered the session.", a.user)),
        clock.log("Authentication challenge received."),
        clock.log(&format!("Submitting credential: {}", "*".repeat(a.answer.len()))),
        clock.task("Validating signature", "OK"),
        clock.task("Comparing credential hash", "MATCH"),
        clock.task("Verifying auth token", "OK"),
        clock.task("Checking permissions", "OK"),
    ]);

    clock.frame(3);
    let (hash, id): (u32, u32) = (rand::rng().random(), rand::rng().random());
    let mut lines = vec![
        String::new(),
        format!("    PASSWORD HASH : 0x{:04X}••••••{:04X}", hash >> 16, hash & 0xffff),
        "    AUTH TOKEN    : VALID".into(),
        format!("    USER ID       : 0x{id:08X}"),
        "    ACCESS LEVEL  : ROOT".into(),
        "    TRUST STATUS  : VERIFIED".into(),
        String::new(),
        clock.log("Authentication successful."),
        clock.log("Generating secure session identifier..."),
        clock.log(&format!("Session identifier assigned: 0xROOT-{s1:04X}")),
        clock.log("Verifying authorized permissions..."),
        clock.log("All authentication checks passed."),
    ];
    lines.extend(
        [
            "",
            "  [#####---------------] 25%  Identity verified",
            "  [##########----------] 50%  Credentials accepted",
            "  [###############-----] 75%  Permissions validated",
            "  [####################] 100% Authentication complete",
            "",
        ]
        .map(String::from),
    );
    frames.push(lines);

    clock.frame(4);
    let mut rows = vec![String::new()];
    rows.extend(ACCESS_ART.iter().map(|line| centre(line)));
    rows.extend([
        String::new(),
        centre(">>> ACCESS GRANTED <<<"),
        String::new(),
        format!("OPERATOR   : {}", a.user),
        format!("PASSWORD   : {}", a.answer),
        "CLEARANCE  : ROOT ACCESS".into(),
        "SESSION    : FULLY AUTHENTICATED".into(),
        "STATUS     : ALL SYSTEMS OPERATIONAL".into(),
        String::new(),
    ]);
    let mut lines = frame(&rows, 0);
    lines.push(String::new());
    lines.push(clock.log("Welcome back, operator."));
    lines.push(clock.log("Secure session established."));
    lines.push(clock.log("All checks completed successfully."));
    lines.extend([
        String::new(),
        "> MAINFRAME MESSAGE:".into(),
        "> Identity confirmed.".into(),
        "> Credentials accepted.".into(),
        "> Authorization granted.".into(),
        format!("> Welcome to the system, {}.", a.user),
        format!("> {payout}"),
        String::new(),
        "> STATUS: ████████████████████ 100%".into(),
        "> CONNECTION: SECURE".into(),
        "> AUTHENTICATION: SUCCESSFUL".into(),
        "> SESSION: ACTIVE".into(),
        String::new(),
        "[SYSTEM READY] :: [AWAITING COMMANDS]".into(),
        "[CONNECTION ESTABLISHED] ░▒▓█".into(),
    ]);
    frames.push(lines);
    frames
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
    let cracked = guess.eq_ignore_ascii_case(answer);
    // Backticks would end the code block early
    let guess_shown = guess.replace('`', "'");
    let user = format!("@{}", ctx.author().name.replace('`', "'"));
    let session: [u16; 3] = rand::rng().random();
    let attempt = Attempt { user: &user, guess: &guess_shown, answer, session };

    let mut clock = Clock::new();
    let (frames, colour) = if cracked {
        let paid = ctx.data().economy.lock().unwrap().adjust(guild_id, user_id.get(), Pocket::Wallet, HACK_REWARD)?;
        let payout = match paid {
            Ok(_) => format!("{} has been transferred to your wallet.", money(HACK_REWARD)),
            Err(EcoError::NoAccount) => format!("Join with /eco join to earn {} next time.", money(HACK_REWARD)),
            Err(_) => "Your wallet is full, so the loot stays put.".to_string(),
        };
        (granted(&attempt, &payout, &mut clock), GREEN)
    } else {
        (denied(&attempt, &mut clock), RED)
    };

    let mut lines = vec![];
    let mut message = None;
    let last = frames.len() - 1;
    for (i, frame) in frames.into_iter().enumerate() {
        lines.extend(frame.iter().flat_map(|line| wrap(line)));
        let reply = terminal(&lines, if i == last { colour } else { TERMINAL });
        match &message {
            None => message = Some(ctx.send(reply).await?),
            Some(message) => {
                tokio::time::sleep(FRAME_DELAY).await;
                message.edit(ctx, reply).await?;
            }
        }
    }
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
    fn boxes_line_up() {
        let lines = frame(&[centre("ACCESS DENIED"), "USER       : @someone".into(), "x".repeat(200)], 1);
        assert!(lines.iter().all(|l| l.chars().count() == BOX_WIDTH + 4));
    }

    #[test]
    fn both_endings_fit_in_one_message() {
        let attempt = Attempt { user: &"@".repeat(33), guess: &"x".repeat(MAX_GUESS_LEN), answer: "changeme", session: [0; 3] };
        for frames in [denied(&attempt, &mut Clock::new()), granted(&attempt, "paid", &mut Clock::new())] {
            let lines: Vec<String> = frames.concat().iter().flat_map(|l| wrap(l)).collect();
            assert!(lines.iter().map(|l| l.chars().count() + 1).sum::<usize>() + 20 < 6000);
            assert!(lines.iter().all(|l| l.chars().count() <= MAX_LINE));
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
