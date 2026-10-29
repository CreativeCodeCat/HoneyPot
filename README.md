# Discord Honeypot Bot

A Discord bot written in Rust ([serenity](https://github.com/serenity-rs/serenity) + [poise](https://github.com/serenity-rs/poise)) with a cog-style module system, featuring a comprehensive honeypot system for multi-server monitoring and security.

## Features

### Honeypot System
- **Multi-server support**: Works across multiple Discord servers
- **Multiple honeypot channels**: Set up multiple honeypot channels per server
- **Message content analysis**: Detects URLs, Discord invites, mentions, excessive caps, and repeated characters
- **Configurable punishments**: Ban, kick, or timeout with customizable duration
- **Template system**: 6 pre-configured templates (strict, lenient, stealth, moderate, newbie, investigation)
- **Account age checks**: Filter users based on account age
- **Whitelist system**: Exempt specific users and roles from detection
- **Auto-ban functionality**: Automatically ban users who send messages in honeypot channels
- **Activity logging**: Tracks all suspicious activity in SQLite database
- **Cross-server intelligence**: Share offender data across all servers
- **Admin notifications**: DM and channel notifications for honeypot triggers
- **CSV export**: Export activity logs to CSV for analysis
- **Weekly summaries**: Automated weekly reports sent to admins
- **Audit log**: Track all configuration changes

### Bot Management
- **Per-server cog control**: Enable/disable cogs per server
- **Slash commands**: Modern Discord slash command support with autocomplete
- **Voice channel stats**: Voice channels named like `MEMBERS`, `BOTS`, `ROLES`, `BOOSTS`, `CHANNELS` or `GUILDS` (servers the bot is in) are renamed with live counts every 10 minutes (Discord's rename limit)
- **AutoMod setup**: Create Discord AutoMod rules with one command
- **SQLite database**: Persistent storage

## Setup

1. **Install Rust** (1.80 or newer) from https://rustup.rs

2. **Configure environment variables**:
   - Copy `.env.example` to `.env`
   - Add your Discord bot token to `.env`:
     ```
     DISCORD_TOKEN=your_bot_token_here
     ```

3. **Create a Discord bot**:
   - Go to https://discord.com/developers/applications
   - Create a new application
   - Under the "Bot" tab, enable the **Server Members** and **Message Content** privileged intents (the bot won't connect without them)
   - Copy the token and add it to your `.env` file
   - Under "OAuth2 > URL Generator", select:
     - `bot`
     - `applications.commands`
     - Under "Bot Permissions", select:
       - `Read Messages/View Channels`
       - `Send Messages`
       - `Ban Members` (required for ban functionality)
       - `Kick Members` (required for kick functionality)
       - `Moderate Members` (required for timeout functionality)
       - `Manage Channels` (for stats channels and ticket channels)
       - `Manage Roles` (for ticket channel permissions)
       - `Administrator` (recommended for full functionality)
   - Use the generated URL to invite the bot to your server

4. **Run the bot**:
   ```bash
   cargo run --release
   ```
   Run it from the project root: the databases are created in `database/` relative to the working directory. For a standalone binary, `cargo build --release` and use `target/release/honeypot-bot`.

## Commands

All commands can be used with `!` prefix or as slash commands (`/`). Most commands require administrator permissions, and all replies are only visible to you when used as slash commands.

### Honeypot Commands

**Channel Management:**
- `/sethoneypot [channel] [auto_ban] [message]` - Set a channel as a honeypot
- `/removehoneypot [channel]` - Remove a honeypot channel
- `/applytemplate <template> [channel]` - Apply a pre-configured template to a channel
  - Templates: `strict`, `lenient`, `stealth`, `moderate`, `newbie`, `investigation`

**Configuration:**
- `/setpunishment [channel] [punishment] [duration]` - Set punishment type (ban/kick/timeout); duration is in minutes, for timeouts
- `/setaccountage [channel] [days]` - Set minimum account age requirement
- `/setadminchannel <channel>` - Set channel for admin notifications
- `/toggleadminnotifications` - Toggle admin DM notifications
- `/addadmin [user]` - Add user to admin notification list
- `/removeadmin [user]` - Remove user from admin notification list

- `/setmessage [channel] [message]` - Change a honeypot's warning message (use `\n` for new lines)
- `/toggleautoban [channel]` - Turn punishment on or off for a honeypot channel

**Whitelist:**
- `/whitelistuser [user] [channel]` - Whitelist a user from honeypot detection
- `/unwhitelistuser [user] [channel]` - Remove user from whitelist

**Monitoring & Logs:**
- `/honeypotstatus` - Check honeypot configuration for this server
- `/honeypotlogs [limit]` - View recent honeypot activity logs, 5 per page (default 10, up to 50)
- `/clearhoneypotlogs` - Clear honeypot activity logs for this server
- `/exportlogs` - Export honeypot logs to CSV
- `/auditlog [limit]` - View configuration audit log, 10 per page (default 25, up to 100)
- `/bancount` - Show the total number of honeypot bans
- `/resetbancount` - Reset the ban counter
- `/checkoffender [user]` - Check if user is in offender list
- `/crossservercheck [user]` - Check user's activity across all servers, 5 servers per page

### Ticket Commands

- `/ticket-setup <category> <channel> <transcript> <handlers> <description>` - Post the ticket panel in `channel`. Members pick a type (Support, Purchase, Other), fill in a reason, and get a private channel in `category`
- `/ticket add <member>` / `/ticket remove <member>` - Give or remove a member's access to the current ticket
- `/ticket followup` - Post a follow-up; the ticket closes 48 hours later unless someone replies

Buttons in each ticket: **Save & Close**, **Lock**, **Unlock**, **Claim** (handlers or admins) and **Transcript** (DMs an HTML transcript to whoever clicks it). On close, the ticket's details and HTML transcript are posted in the transcript channel and the ticket channel is deleted. Tickets also close automatically after 7 days without messages, or when the member who opened them leaves the server.

### Lockdown Commands

- `/lock [time] [reason]` - Stop @everyone from sending messages in this channel. `time` is optional (e.g. `30m`, `2h`, `1d`, `1h30m`) and defaults to 24 hours
- `/unlock` - Lift the lockdown and restore the channel's original permissions

Both need **Manage Channels**. Timed lockdowns are lifted automatically, even if the bot restarts in between.

### Moderation Commands

Each command needs the matching permission for both the member using it and the bot. Nobody can moderate themselves, the bot, the server owner, or a member whose highest role is the same as or above theirs or the bot's. Members are sent a DM saying what happened and why (if their DMs are open), and the reason goes in the audit log.

- `/ban <user> [reason] [delete_messages]` - Ban a member, or anyone by pasting their user ID; optionally delete their messages from the last day or week (**Ban Members**)
- `/unban <user> [reason]` - Unban a user by pasting their user ID (**Ban Members**)
- `/kick <member> [reason]` - Kick a member (**Kick Members**)
- `/timeout <member> <length> [reason]` - Time a member out for 60 seconds up to 28 days (**Moderate Members**)
- `/untimeout <member> [reason]` - End a member's timeout (**Moderate Members**)
- Right-click a message → Apps → **Moderate** - Delete the message, or time out, end the timeout of, kick or ban its author (banning from here also deletes their last day of messages). Shown to members with **Manage Messages**
- Right-click a member → Apps → **Moderate** - Time out, end the timeout of, kick or ban them. Shown to members with **Moderate Members**

In both menus each button needs its own permission and is greyed out without it.

### Staff Policy

All moderation has to go through the bot, so it's logged properly. While the `staffpolicy` cog is enabled, a member who kicks, bans, unbans, times out or ends a timeout using Discord's own menus:

- Is sent a DM warning them about the policy
- Has their roles removed: the roles picked in `/staffpolicy setup`, or every role if none are picked (except roles managed by integrations, and roles at or above the bot's highest role, which it can't touch). They're saved so an admin can give them back. If a demoted role is set with `/staffpolicy setup`, they're given it instead; restoring takes it away again
- Has what they did undone, so it must be redone through the bot: bans are lifted, unbans are reversed by banning again, timeouts are removed, and removed timeouts are put back (if they hadn't run out). A kicked member can't be put back, so they're DMed a one-use invite valid for 7 days

The bot's own actions, other bots and the server owner are ignored. Each case is posted in the log channel chosen with `/staffpolicy setup`, or the member log channel set with `/server-setup logs` if none is chosen. Keep the bot's role above your staff roles, or it can't remove them. The bot needs **View Audit Log**, plus **Ban Members**, **Moderate Members** and **Create Invite** to undo actions.

- `/staffpolicy setup [demoted_role] [log_channel]` - Set a role, like `Demoted`, that staff are given in place of the roles taken, and the channel cases are logged in (the member log channel if not set). The reply shows the current settings, with a menu to pick which roles are removed (none picked removes every role the bot can) (admin)
- `/staffpolicy list` - List staff whose roles were taken and not yet restored (admin)
- `/staffpolicy clear` - Clear that list without giving any roles back; the cleared roles can't be restored with the bot afterwards (admin)
- `/staffpolicy restore <member>` - Give a staff member back the roles the policy took (admin)

### Economy Commands

Each server has its own economy. Members start with ₪ 100 in the bank after joining.

- `/eco join` - Open an account
- `/eco work` - Work a random job for ₪ 1–1,320 (once every 6 hours)
- `/eco daily` - Claim a daily reward, ₪ 100–5,000 by default (once a day, resets at midnight UTC)
- `/eco balance [user]` - Show a wallet and bank balance
- `/eco deposit <amount>` / `/eco withdraw <amount>` - Move money between your wallet and bank
- `/eco pay <user> <amount>` - Pay another member from your wallet
- `/eco wallet add|remove <amount> [user]` and `/eco bank add|remove <amount> [user]` - Adjust balances (**Manage Server**)
- `/eco dailyrate set <min> <max> [role]` / `remove [role]` / `list` - Set the daily reward range for everyone or for a role; members get the range of their highest role that has one (**Manage Server** to change)

### Levels Commands

Members earn XP for chatting, at most once per XP cooldown (5 minutes by default). A message earns the base XP (25), or the max XP (300) instead when it's sent within the engagement window (5 minutes) after the latest message from a *different* member in the same channel, so conversation is rewarded rather than message volume. Your own earlier messages never count, bots and webhooks are ignored, and a message on cooldown earns nothing but still counts as the channel's latest message. Each level needs 2,500 more XP than the last. Level-ups are announced in the level-up channel set with `/rank-setup`, otherwise the server's system channel, otherwise the channel the member was chatting in.

- `/rank [user]` - Show a rank card image with level, rank, reputation, highest role and progress to the next level
- `/leaderboard` - Show the top 100 members by XP, 10 per page
- `/reputation <user>` - Give a member +3 reputation (once every 2 hours)
- `/rank-setup [rate] [base] [max] [window] [cooldown] [channel]` - Change the levels settings for the server (**Administrator**):

  | Option | Meaning | Default | Range |
  |---|---|---|---|
  | `rate` | Extra XP each level needs over the last | 2,500 | 250–7,500 |
  | `base` | XP per message | 25 | 1–500 |
  | `max` | XP for replying to someone else within the window (must be at least `base`) | 300 | 1–500 |
  | `window` | Engagement window, in minutes | 5 | 1–60 |
  | `cooldown` | Seconds between XP awards for each member | 300 | 0–3,600 |
  | `channel` | Where level-ups are announced | System channel | Any text channel |

### Pet Commands

A tamagotchi for Discord. Pets are bought and fed with money from the economy wallet (`/eco join` first). One treat lasts 24 hours; a pet that runs out of food dies. Owners get a DM when their pet has about a day of food left, and when it dies. Fun drains over time too, and happy pets (from playing) can hug and battle. Pets' needs pause while the cog is disabled.

- `/pet buy <kind> <name> [gender]` - Buy a pet for ₪ 1,000 (20 kinds, from cats to dragons); gender is random if not chosen
- `/pet status [user]` - A picture card with the pet, its age, level, experience, fun, mood, treats and how long the food will last
- `/pet feed <amount>` - Give treats (₪ 5 each); pets can hold 30 plus their level
- `/pet play` - Play for experience and fun (uses a treat). Each level needs 10 more experience than the last, up to level 100
- `/pet hug <user>` - Hug someone, or their pet (your pet must be happy)
- `/pet playdate <user>` - Both pets gain fun and experience
- `/pet battle <user>` - Challenge someone's pet; they accept or decline with buttons. Higher-level pets have better odds
- `/pet release` - Let your pet go

### Games Commands

- `/hack <password>` - Try to hack the bot: it picks one of 25 common passwords at random, and if your guess matches you win ₪ 2,500 for your `/eco` wallet. Once every 5 minutes
- `/heist <amount>` - Start or join a heist with a bet from your wallet. After 60 seconds a story plays out; survivors get their bet back plus 30%. A happy pet joins its owner with a free half bet. The server waits 15 minutes between heists
- `/fight <user> [wager]` - Challenge someone to a fight; they accept with a button and the winner takes both wagers. Challenging the bot skips the accept. Fighters recover for 10 minutes

- `/counting setup <channel>` - Set the counting channel (admin). Members count up one at a time, taking turns; a wrong number or counting twice in a row gets a cheeky message from the bot and starts the count again from 1. Messages that don't start with a number are deleted
- `/counting stats` - See the current count and the server's high score
- `/counting stop` - Stop counting in this server, keeping the high score for when it's set up again (admin)

- `/guessing setup <channel>` - Set the guessing channel (admin). The bot thinks of a number from 1 to 100 and members post numbers to guess it; wrong guesses get a 🔼/🔽 hint and whoever gets it wins ₪ 500, then it starts again with a new number. Each member can guess once every 15 seconds; guesses that come too soon get ⏳ and don't count. Anything that isn't a number from 1 to 100 is deleted
- `/guessing stats` - See guesses so far, numbers found and the fewest guesses it's taken
- `/guessing stop` - Stop guessing in this server, keeping the stats (admin)

Bets on heists and fights are held until the game ends, and refunded if the bot restarts mid-game. Each game is off until an admin turns it on with `/enablegame`.

### Suggestion Commands

- `/suggest <type> <suggestion> <dm>` - Post a suggestion in the suggestions channel, with ✅/❌ reactions for voting. With `dm` on, you're messaged when it's decided. Once every 5 minutes
- `/suggestion setup <channel> <accepted> <declined>` - Choose where suggestions go and where accepted and declined ones are announced
- `/suggestion accept|decline|pending <message-id> [reason]` - Update a suggestion's status, announce it and DM the member if they asked
- `/suggestion completed <message-id>` - Mark a suggestion as done and clear its votes

All `/suggestion` commands need **Administrator**.

### Server Logs

The Logs cog (`/enablecog logs`) posts what happens in the server to a log channel per type, set with `/server-setup logs <type> <channel>`. Types without a channel aren't logged. When Discord's audit log records who made a change, the log adds **By** (and their **Reason**) — e.g. who deleted a channel, changed roles, banned or kicked someone, or deleted another member's message. This needs the bot to have **View Audit Log**; without it the logs still work, just without who did it. Joins, voice and username changes aren't in the audit log.

| Type | Logs |
|---|---|
| Channel | Channels and categories created, deleted or changed (name, topic, NSFW, slowmode, category, permissions) |
| Events | Scheduled events created, updated or cancelled |
| Emoji | Emojis added, removed or renamed |
| Sticker | Stickers added, removed or renamed |
| Guild | Server name, icon, banner, owner, verification level and other settings changed |
| Join & Leave | Members joining (with account age) and leaving |
| Member | Nicknames, roles, timeouts and server avatars changed; bans and unbans |
| User | Usernames, display names and avatars changed |
| Message | Messages deleted (with their text, if the bot saw them) or edited (before and after), and bulk deletes |
| Role | Roles created, deleted or changed (name, colour, permissions...) |
| Thread | Threads created, deleted, renamed, archived or locked |
| Voice | Members joining, leaving or moving voice channels, and server mutes and deafens |

### Announcement Commands

Announcements always go to the channel set with `/server-setup announcements`, wherever the command is used. It can be a text or announcement channel; in an announcement channel the post is also published to servers following it. Needs **Manage Server**.

- `/announce message <role> <title> <information>` - Post an announcement embed pinging `role`. Use `\n` for new lines
- `/announce help` - Show help for the command

### Profile Commands

Members make an "about me" profile that anyone can view. Profiles belong to the member, so they're the same in every server.

- `/profile create` - Create your profile
- `/profile view [user]` - View your profile or someone else's: their name, username and ID, plus the fields they've set and their money if the Economy cog is on (anything not set is left out). Also in the user right-click menu under Apps → **Profile**
- `/profile set <field> <value>` - Set `gender`, `age`, `birthday` (DD-MM-YYYY is shown like "Wednesday, 23rd February 2022"), `languages`, `favorite-color`, `favorite-animals`, `favorite-foods`, `favorite-songs`, `favorite-artists`, `favorite-movies`, `favorite-actors`, `origin`, `status`, `hobbies` or `aboutme`
- `/profile delete` - Delete your profile
- `/profile help` - Show help for the command

### Info Commands

- `/info server` - Show the server's details and statistics, with its roles on the following pages
- `/info channel [channel]` - Show a channel's details (this channel by default)
- `/info user [user]` - Show a user's account and server details (you by default). Also in the user right-click menu under Apps → **User Info**
- `/info staff` - List the server's owner, administrators (Administrator or Manage Server) and moderators (Moderate, Kick or Ban Members, or Manage Messages)

### Sticky Commands

A sticky message stays near the bottom of its channel: once enough messages are posted below it, the old copy is deleted and it's posted again at the bottom. All need **Manage Messages**.

- `/sticky stick <message> [count]` - Stick a message in this channel; it moves down after `count` messages (default 3). Use `\n` for new lines
- `/sticky unstick` - Remove this channel's sticky message
- `/sticky list` - List the server's sticky messages, 10 per page

### Rules Commands

Each server writes its own rules as sections (a title, text and colour), which are posted as embeds. Posting again deletes the previous rules messages first, so the channel only ever has one copy. All need **Manage Server**.

- `/rules add` - Add a section using a form
- `/rules edit <number>` - Edit a section (the form opens with its current text)
- `/rules remove <number>` - Remove a section
- `/rules list` - List the sections and where they were last posted
- `/rules post <channel>` - Delete the old rules messages (wherever they were posted) and post the sections in `channel`

### Other Commands

- `/automod` - Create a Discord AutoMod rule (needs **Administrator**). Each subcommand only has the options that work with it:
  - `/automod word-list <list> [allow_list]` - Block Discord's word lists: all, profanity, sexual content or slurs
  - `/automod words <words> [allow_list] [timeout_minutes]` - Block the comma-separated `words` in messages (`*` as a wildcard)
  - `/automod profile-names <words> [allow_list]` - Block `words` in member names; matching members can't chat or interact until they change it
  - `/automod pattern <pattern> [allow_list] [timeout_minutes]` - Block capitals, spoilers, emojis, headers, unicode, code blocks, bullet lists or invites, or all of them in one rule with `All`
  - `/automod mention-spam [mention_limit] [timeout_minutes] [raid_protection]` - Block messages with more than `mention_limit` mentions (default 10), and optionally mention raids
  - `/automod message-spam` - Block messages Discord flags as spam

  `allow_list` is comma-separated words that never trigger the rule. `timeout_minutes` also times the member out, up to 4 weeks (the bot needs **Moderate Members**). Every subcommand also takes `alert_channel`, where Discord posts alerts (the server's system channel if not given). Rules are enabled as soon as they're created. Discord allows only one word-list, profile-names, mention-spam and message-spam rule per server, so running one of those again overwrites the existing rule of that kind (even one made in Discord's settings) instead of failing. Exempt roles and channels set on it are kept.
- `/cleanup <amount> [user]` - Delete up to 1,000 recent messages in the channel, optionally only one member's. Messages older than 14 days are left, as Discord can't bulk delete them (needs **Manage Messages**)
- `/ping` - Show latency and uptime (5-minute cooldown)
- `/resetchannel [channel]` - After a confirm button, recreate a channel (this one by default) with the same name, category, position, settings and permissions, then delete the old one and its messages (needs **Manage Channels**)
- `/updatestats` - Update the voice channel stats now instead of waiting for the next 10-minute refresh (admin)

### Cog Management Commands

Cogs: `announcements`, `automod`, `economy`, `games`, `general`, `honeypot`, `info`, `levels`, `lockdown`, `logs`, `moderation`, `pets`, `profile`, `rules`, `serverstats`, `staffpolicy`, `sticky`, `suggestions`, `tickets`, `cogmanager`. Only `cogmanager` is enabled by default; an admin turns on the others per server with `/enablecog`. Disabling one blocks its commands and background work in that server only. `cogmanager` can't be disabled.

- `/enablecog <cog_name>` - Enable a cog for this server
- `/disablecog <cog_name>` - Disable a cog for this server
- `/enablegame <game>` / `/disablegame <game>` - Turn single games (`counting`, `fight`, `guessing`, `hack`, `heist`) on or off (only available while the Games cog is enabled). Every game is off by default, so after `/enablecog games` turn on the ones you want.
- `/cooldown set <command> <seconds> [scope]` - Set a command's cooldown (0 removes it). Scope `User` (default) makes each member wait on their own; `Global` makes one use put the command on cooldown for the whole server. `guessing` can be set too, for guesses in the guessing channel
- `/cooldown list` - List the commands with a cooldown and their scope
- `/gamestatus` - Check which games are enabled for this server (only available while the Games cog is enabled)
- `/cogstatus` - Check the status of all cogs for this server

## Templates

The bot includes 6 pre-configured templates for quick honeypot setup:

1. **strict** - Immediate ban, 7-day account age, all pattern checks
2. **lenient** - No auto-ban, 60-min timeout, logging only
3. **stealth** - No auto-ban, 10-min timeout, deceptive welcome message
4. **moderate** - Kick on trigger, 3-day account age, balanced checks
5. **newbie** - 30-min timeout, 1-day account age, focus on new accounts
6. **investigation** - 5-min timeout, no age restriction, soft observation

## How It Works

1. **Setup**: An admin uses `/sethoneypot` or `/applytemplate` to configure honeypot channels
2. **Visual Warning**: The bot posts a warning embed in the channel
3. **Monitoring**: The bot monitors all messages in honeypot channels
4. **Pattern Detection**: Analyzes messages for URLs, invites, mentions, caps, and repeated characters
5. **Account Verification**: Checks account age if configured
6. **Action**: Applies configured punishment (ban/kick/timeout) or logs only
7. **Notification**: Sends alerts to admins via DM or channel
8. **Cross-Server Intelligence**: Tracks offenders across all servers
9. **Storage**: All activity stored in SQLite database (`database/honeypot.db`)
10. **Review**: Admins can view logs and export data for analysis

## Database

The bot uses SQLite for persistent storage:
- `database/honeypot.db` - Honeypot configuration, activity logs, offenders, audit log
- `database/cog_config.db` - Per-server cog enable/disable configuration
- `database/tickets.db` - Ticket system setup and tickets
- `database/suggestions.db` - Suggestion channels and open suggestions
- `database/staff_policy.db` - Roles taken by the staff policy, for restoring
- `database/sticky.db` - Sticky messages
- `database/counting.db` - Counting channels, counts and high scores
- `database/guessing.db` - Guessing channels, current numbers and stats
- `database/lockdown.db` - Active lockdowns and the permissions to restore
- `database/economy.db` - Economy accounts
- `database/levels.db` - XP, reputation and level settings
- `database/rules.db` - Rules sections and the last posted messages
- `database/pets.db` - Pets and their needs
- `database/profiles.db` - User profiles
- `database/games.db` - Mini-game cooldowns

## Use Cases

- Detect and ban automated bots and spam accounts
- Monitor channels that might attract malicious users
- Cross-server intelligence sharing for repeat offenders
- Track suspicious patterns in user behavior
- Test bot interactions in a controlled environment
- Debug bot behavior with detailed logging

## Project Structure

```
HoneyPot/
├── Cargo.toml              # Rust dependencies
├── assets/
│   ├── fonts/              # Noto Sans (SIL OFL), built into the bot
│   └── emoji/              # Twemoji pictures for pet cards (CC-BY 4.0)
├── .env.example            # Environment variables template
├── database/               # SQLite database files (created at runtime)
│   ├── honeypot.db         # Honeypot data
│   └── cog_config.db       # Cog configuration
└── src/
    ├── main.rs             # Startup, event dispatch, cog check, error handling
    ├── log.rs              # Colored console logging
    ├── pagination.rs       # Previous / Next / Delete buttons for multi-page replies
    ├── render.rs           # Shared drawing code for the image cards
    └── cogs/
        ├── mod.rs          # Shared state, cog list, helpers
        ├── cog_manager.rs  # Per-server enable/disable
        ├── economy/
        │   ├── mod.rs      # /eco commands
        │   └── store.rs    # SQLite storage
        ├── games.rs        # /hack
        ├── general.rs      # /ping
        ├── info.rs         # /info and the User Info user menu
        ├── announce.rs     # /announce
        ├── automod.rs      # /automod
        ├── levels/
        │   ├── mod.rs      # XP on messages and level commands
        │   └── store.rs    # SQLite storage and the level curve
        ├── lockdown.rs     # /lock and /unlock
        ├── logs.rs         # Server logs for /server-setup logs channels
        ├── moderation.rs   # /ban, /kick, /timeout... and the Moderate message menu
        ├── pets/
        │   ├── mod.rs      # /pet commands, battles and hunger DMs
        │   ├── card.rs     # /pet status picture card
        │   └── store.rs    # SQLite storage and pet needs
        ├── profile.rs      # /profile and the Profile user menu
        ├── rules.rs        # /rules
        ├── staff_policy.rs # Punishing moderation done outside the bot
        ├── stats.rs        # Voice channel stats
        ├── sticky.rs       # /sticky and moving sticky messages down
        ├── suggestions.rs  # /suggest and /suggestion
        ├── tickets/
        │   ├── mod.rs      # Panel, buttons, closing, auto-close
        │   ├── commands.rs # /ticket-setup and /ticket
        │   ├── store.rs    # SQLite storage
        │   └── transcript.rs # HTML transcripts
        └── honeypot/
            ├── mod.rs      # Message trap, punishments, weekly summary
            ├── commands.rs # Honeypot commands
            └── store.rs    # SQLite storage
```

## Adding More Cogs

1. Create `src/cogs/your_cog.rs` and add `pub mod your_cog;` to `src/cogs/mod.rs`
2. Add its display name (e.g. `"YourCog"`) to `COGS` in `src/cogs/mod.rs`
3. Write commands with `category = "YourCog"` so the per-server enable/disable check applies:
   ```rust
   /// Description shown in Discord
   #[poise::command(slash_command, prefix_command, guild_only, ephemeral, category = "YourCog")]
   pub async fn yourcommand(ctx: Context<'_>) -> Result<(), Error> {
       ctx.say("Hello!").await?;
       Ok(())
   }
   ```
4. Register the command in `commands()` in `src/main.rs`
5. For event handlers or background tasks, check `data.is_cog_enabled(guild_id, "yourcog")` yourself
6. Use `log::info()`, `log::error()`, `log::warning()` or `log::debug()` for colored console output

## Security Notes

- Never commit your `.env` file or Discord token
- The bot requires administrator permissions for full functionality
- Honeypot logs are stored locally in SQLite database (`database/honeypot.db`)
- Consider implementing database backups for production deployments
- Review permissions carefully when inviting the bot to servers
- Use the whitelist feature to exempt trusted users from detection
