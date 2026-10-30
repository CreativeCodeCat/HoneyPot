# Discord Honeypot Bot

A Discord bot written in Rust ([serenity](https://github.com/serenity-rs/serenity), with its own slash command framework) with a cog-style module system, featuring a comprehensive honeypot system for multi-server monitoring and security.

## Features

### Honeypot System
- **Multi-server support**: Works across multiple Discord servers
- **Multiple honeypot channels**: Set up multiple honeypot channels per server
- **Message content analysis**: Detects URLs, Discord invites, mentions, excessive caps, and repeated characters
- **Configurable punishments**: Ban, kick, timeout with customizable duration, or quarantine
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

## Dashboard

Once the bot is connected, it serves a dashboard at http://localhost:8080: the bot's version, uptime and shards, totals for servers, members and channels, and every server with how many cogs it has on. It refreshes every 10 seconds, and the same data is at `/api/overview` as JSON.

Clicking a server switches the sidebar to that server: its **Overview** (members, channels, roles, boosts, owner, creation date), **Commands** (every command people can use there, from the cogs that are on, with the permission each needs, and a filter box), and **Cogs** (each cog, on or off). The server's owner and admins (and the bot's owners) can turn cogs on and off there with a switch, like `/enablecog` and `/disablecog`, and the server's slash commands update to match; people with only Manage Server see them read-only. Each cog has an **Info** button if it keeps data for the server, which opens a read-only view of its database tables (only the server's own rows, 50 per page), and a **Cmd** button if it's on, which opens its commands. The bot owner's commands aren't listed. The page's data is at `/api/servers/<id>`.

People sign in with Discord and see the servers they own or have **Administrator** or **Manage Server** in; the bot's owners (or its team) see every server. To turn it on:

1. In the [Discord Developer Portal](https://discord.com/developers/applications), open the bot's application, go to **OAuth2**, and copy (or reset) the **Client Secret**
2. Under **Redirects**, add `http://localhost:8080/callback`, or your `DASHBOARD_URL` followed by `/callback`
3. Put the secret in `.env` as `DISCORD_CLIENT_SECRET`, and restart the bot

Without the secret, the dashboard stays off. Other settings in `.env`:
- `DASHBOARD_URL` - Where people open it, e.g. `https://dash.example.com` (default `http://localhost:8080`). Use HTTPS if it's reachable from the internet, so sign-ins can't be read on the way
- `DASHBOARD_ADDR` - Where it listens (default `127.0.0.1:8080`, only this machine). Use `0.0.0.0:8080` to reach it from other machines, ideally behind a reverse proxy that adds HTTPS

Sign-ins last a week, and are kept in memory, so restarting the bot signs everyone out. The servers someone manages are read when they sign in; sign out and in again to update them.

## Commands

All commands are slash commands (`/`). Most commands require administrator permissions, and most replies are only visible to you.

### Honeypot Commands

**Channel Management:**
- `/sethoneypot [channel] [auto_ban] [message]` - Set a channel as a honeypot
- `/removehoneypot [channel]` - Remove a honeypot channel
- `/applytemplate <template> [channel]` - Apply a pre-configured template to a channel
  - Templates: `strict`, `lenient`, `stealth`, `moderate`, `newbie`, `investigation`

**Configuration:**
- `/setpunishment [channel] [punishment] [duration]` - Set punishment type (ban/kick/timeout/quarantine); duration is in minutes, for timeouts. Quarantine needs `/quarantine setup`, and falls back to a 24-hour timeout until its done
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

`/ticket add`, `/ticket remove` and `/ticket followup` only work inside the ticket channel they're for. `/ticket-setup` follows the commands channel rule like other commands (staff with Administrator or Manage Server can use it anywhere).

Buttons in each ticket: **Save & Close**, **Lock**, **Unlock**, **Claim** (handlers or admins) and **Transcript** (DMs an HTML transcript to whoever clicks it). On close, the ticket's details and HTML transcript are posted in the transcript channel and the ticket channel is deleted. Tickets also close automatically after 7 days without messages, or when the member who opened them leaves the server.

### Lockdown Commands

- `/lock [time] [reason]` - Stop @everyone from sending messages in this channel. `time` is optional (e.g. `30m`, `2h`, `1d`, `1h30m`) and defaults to 24 hours
- `/unlock` - Lift the lockdown and restore the channel's original permissions

Both need **Manage Channels**. Timed lockdowns are lifted automatically, even if the bot restarts in between.

### Raid Protection

The RaidProtection cog (`/enablecog raidprotection`) watches how fast members join. When too many join too quickly (10 in 30 seconds by default), the server goes into **raid mode**:

- 🚨 Staff get an alert, in the chosen channel, else the join & leave log, else the server's system channel
- 🔒 Text channels are locked the same way `/lock` does, so `/unlock` works on them too. The alert channel and honeypot channels stay open (raiders who post in a honeypot still get caught). Raid locks also lift themselves after 24 hours, as a safety net
- 🛡️ The verification level is raised to High: new members must wait 10 minutes on the server before they can talk (and need a verified email and an account at least 5 minutes old)
- 👢 Every account that joins until it's over (including the ones that set it off) is timed out, kicked (default), banned or quarantined. Bots are never touched

Raid mode ends by itself once there have been no raid-speed joins for 10 minutes (adjustable), or with `/raid end`. Channels it locked are unlocked and the verification level is put back. If the Incidents cog is on, every account it acts on gets an incident report.

- `/raid setup` - Change the threshold (joins and seconds), the quiet time, whether to lock channels and raise verification, what happens to joiners, the timeout length and the alert channel. Every option is optional; unset ones keep their current value
- `/raid status` - See the settings and whether a raid is going on
- `/raid end` - End raid mode now

These need **Manage Server**. The bot needs **Manage Channels**, **Manage Server**, and **Moderate Members**, **Kick Members** or **Ban Members** for the joiner action.

### Suspicious Accounts

The SuspiciousAccounts cog (`/enablecog suspiciousaccounts`) scores members from 0 to 100 on how suspicious their account looks. Each group of signals has a cap, so no single group can decide a score alone:

| Group (cap) | Signals |
|---|---|
| **Account & profile** (40) | Account under 1 day old +25 (under 7 days +15, under 30 days +5) · default avatar +10 · no display name +5 · username with 5+ digits +10 |
| **Activity** (30) | Posting in bursts (5 messages in 10 seconds) +15 · repeating the same message (3 times in a minute) +15 · first message within a minute of joining +5 · joined less than a day ago +5 · 30+ days on the server with 100+ messages **−15** |
| **Message content** (30) | Discord invites +15 (3 or more +25) · mass mentions or @everyone/@here +15 · links while new +5 · shouting or repeated characters (3+ messages) +5 |
| **History** (50) | Caught by a honeypot here +30 · caught by honeypots on other servers the bot is in +20 · past incidents +10 each (up to +20) · joined during a raid +15 |

Scores are worked out on joining and as members post. When a member reaches the threshold (60 by default), staff get an alert with the score and why, once per member. Admins can also have them quarantined automatically (a 24-hour timeout if quarantine isn't set up). Scores also show on incident reports and quarantine review cards.

- `/suspicious check <user>` - See a member's score and what makes it up
- `/suspicious list` - See the most suspicious members (anyone with recorded activity, or who joined in the last 30 days)
- `/suspicious setup [threshold] [alert_channel] [auto_quarantine]` - Change the threshold, where alerts go (default: the quarantine review channel, join & leave log, or system channel), and auto-quarantine

Staff commands need **Moderate Members** (setup needs **Manage Server**). Activity is only known from when the cog was turned on.

### Quarantine

The Quarantine cog (`/enablecog quarantine`) isolates suspicious members until staff review them. A quarantined member's roles are taken away and saved, and they get a **Quarantined** role that can only see a quarantine channel, where they can talk to staff. Leaving and rejoining doesn't get them out.

Members are quarantined:

- **By the honeypot**, when a honeypot's punishment is `quarantine` (`/setpunishment`)
- **During a raid**, when Raid Protection's joiner action is Quarantine (`/raid setup joiners: Quarantine`)
- **On joining with a new account**, younger than the age set with `/quarantine setup new_account_days`

Each quarantine posts a review card in the review channel with the member, their account age, why they were quarantined and the roles taken, plus **Release**, **Kick** and **Ban** buttons (each needs the matching permission). Release gives their roles back. If quarantine isn't set up (or the cog is off), the honeypot and raid protection time the member out for 24 hours instead, so nobody gets through untouched.

- `/quarantine setup <review_channel> [new_account_days]` - Create the Quarantined role and the quarantine channel, hide every other channel from the role, and choose where review cards go. Run it again any time; it reuses what's there. Channels made later are hidden from the role automatically
- `/quarantine list` - See who's in quarantine
- `/quarantine release <user>` - Release a member and give their roles back

Staff commands need **Moderate Members** (setup also needs **Manage Server**, **Manage Roles** and **Manage Channels**). The bot needs **Manage Roles** and **Manage Channels**, and its role must be above the roles it takes away; roles it can't take are listed on the review card.

### Moderation Commands

These work in any channel, even when a commands channel is set with `/server-setup commands`. Each command needs the matching permission for both the member using it and the bot. Nobody can moderate themselves, the bot, the server owner, or a member whose highest role is the same as or above theirs or the bot's. Members are sent a DM saying what happened and why (if their DMs are open), and the reason goes in the audit log.

- `/ban <user> [reason] [delete_messages]` - Ban a member, or anyone by pasting their user ID; optionally delete their messages from the last day or week (**Ban Members**)
- `/unban <user> [reason]` - Unban a user by pasting their user ID (**Ban Members**)
- `/kick <member> [reason]` - Kick a member (**Kick Members**)
- `/timeout <member> <length> [reason]` - Time a member out for up to 28 days. Pick a usual length or type your own, like `90m`, `2h30m`, `1d 12h` or `3 days` (**Moderate Members**)
- `/untimeout <member> [reason]` - End a member's timeout (**Moderate Members**)

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

### Evidence Collection (Incidents)

The Incidents cog (`/enablecog incidents`) opens an incident report whenever Discord's audit log records a ban, kick or timeout: from the bot's moderation commands, the honeypot, or staff doing it by hand. Each report saves, as they were at the time:

- **The action**: what happened, the reason, who did it and when
- **The account**: username, display name, ID, account age, when they joined and their roles
- **Their messages**: every message of theirs the bot had seen across the server's channels and threads (up to the newest 200), with attachment links, including ones deleted along with a ban

Reports also show the member's **moderation history**: their other incidents on the server and any honeypot catches. Reports aren't posted anywhere; staff with **Moderate Members** read them with:

- `/incident list [user]` - List incident reports, newest first
- `/incident view <id>` - Read a report, with their messages on the following pages
- `/incident export <id>` - Download a report in full as a text file

The bot needs **View Audit Log**. It only knows messages it saw since it last started (up to 50 per channel), so older messages aren't in reports.

### Economy Commands

Each server has its own economy. Members start with ₪ 100 in the bank after joining.

- `/eco join` - Open an account
- `/eco jobs` - List every job with its pay range and how often it comes up
- `/eco work` - Work a random job (once every 6 hours). Better jobs are rarer and pay more: from Cleaner (₪ 50–150, the most common) up to Space Invader (₪ 4,000–8,000, about 1 shift in 100)
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
- `/defuse [bet]` - Defuse a bomb: cut one of five coloured wires before the 30-second timer runs out. One wire defuses it, one sets it off, and the other three make the timer jump forward 8 seconds. A clue says which wire the defuse wire is next to. An optional bet (₪ 10 to ₪ 1,000 from your wallet) is paid back doubled if you defuse it. Once a minute
- `/heist <amount>` - Start or join a heist with a bet from your wallet. After 60 seconds a story plays out; survivors get their bet back plus 30%. A happy pet joins its owner with a free half bet. The server waits 15 minutes between heists
- `/fight <user> [wager]` - Challenge someone to a fight; they accept with a button and the winner takes both wagers. Challenging the bot skips the accept. Fighters recover for 10 minutes

- `/counting setup <channel>` - Set the counting channel (admin). Members count up one at a time, taking turns; a wrong number or counting twice in a row gets a cheeky message from the bot and starts the count again from 1. Messages that don't start with a number are deleted
- `/counting stats` - See the current count and the server's high score
- `/counting stop` - Stop counting in this server, keeping the high score for when it's set up again (admin)

- `/guessing setup <channel>` - Set the guessing channel (admin). The bot thinks of a number from 1 to 100 and members post numbers to guess it; wrong guesses get a 🔼/🔽 hint and whoever gets it wins ₪ 500, then it starts again with a new number. Each member can guess once every 15 seconds; guesses that come too soon get ⏳ and don't count. Anything that isn't a number from 1 to 100 is deleted
- `/guessing stats` - See guesses so far, numbers found and the fewest guesses it's taken
- `/guessing stop` - Stop guessing in this server, keeping the stats (admin)

Bets on heists, fights and bombs are held until the game ends, and refunded if the bot restarts mid-game. Each game is off until an admin turns it on with `/enablegame`. Once an admin sets a games channel with `/server-setup games <channel>`, `/hack`, `/heist`, `/fight` and `/defuse` only work there (even if a commands channel is set too), and using them anywhere else tells the member to head to the games channel. Like the commands channel, any message there that isn't a command is deleted, so it stays clear for playing. Other commands used there tell the member to use the commands channel instead (staff, and setup, moderation and ticket commands, still work there). Counting and guessing keep their own channels.

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
| Boosts | A thank-you image when a member starts boosting (their avatar, name, and the server's boosts and level), and a note when they stop |
| Welcome | A welcome image for each new member (not bots), with no message or ping: their avatar in a glowing ring, their name, the server's name and which member they are |
| Member | Nicknames, roles, timeouts and server avatars changed; bans and unbans |
| User | Usernames, display names and avatars changed |
| Message | Messages deleted (with their text, if the bot saw them) or edited (before and after), and bulk deletes |
| Role | Roles created, deleted or changed (name, colour, permissions...) |
| Thread | Threads created, deleted, renamed, archived or locked |
| Voice | Members joining, leaving or moving voice channels, and server mutes and deafens |

### Live Notifications (Twitch)

The LiveStatus cog (`/enablecog livestatus`) follows one Twitch streamer per server. When they go live, it posts a live card in the chosen channel, pinging a role if set:

- **Game** and **Subject**, from the stream's category and title on Twitch
- An **I AM LIVE** section with links to watch on Twitch (automatic), plus TikTok, YouTube and Kick if set
- A **SPOTLIGHT** section with your own message (merch, events...), if set
- A coloured accent bar and lines (green by default)

When the stream ends, the card is deleted. Twitch is checked every minute.

- `/live setup <twitch> <channel> [ping_role] [tiktok] [youtube] [kick] [colour]` - Choose the streamer (username or twitch.tv link), where the card goes, who it pings, the extra links and the card colour. Run it again to change any of them
- `/live spotlight` - Write the spotlight message in a pop-up (leave it empty to remove it)
- `/live preview` - See the card as it'll look, only to you (with their real stream if they're live)
- `/live status` - See the settings and whether they're live
- `/live remove` - Stop following the streamer

These need **Manage Server**. The bot needs a Twitch app: create one at https://dev.twitch.tv/console (any name, OAuth redirect `http://localhost`, category "Chat Bot"), then put its **Client ID** and a **Client Secret** in `.env` as `TWITCH_CLIENT_ID` and `TWITCH_CLIENT_SECRET` and restart the bot. Without them, the commands work but nothing is posted.

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

### AFK Commands

- `/afk [reason]` - Mark yourself as AFK (away), or clear it if you already are. While you're AFK, anyone who mentions you is told why and since when, and the next time you send a message you're welcomed back and it clears. The bot's AFK notices delete themselves after 10 seconds. AFK is per server

### Application Commands

Admins create their own positions to apply for, each with up to five questions (Discord allows five boxes in a pop-up, with questions up to 45 characters) and an optional role. Members apply in a pop-up, and their answers are posted in the review channel with **Accept** and **Deny** buttons, which work for members with **Manage Server**, even after a restart. The applicant gets a DM with the decision, and accepting gives them the position's role. A member can't apply for the same position again while their last application is waiting.

- `/apply <position>` - Apply for a position, answering its questions in a pop-up. Members can also press the position's button on an application panel
- `/application channel <channel>` - Where applications are posted for review (**Manage Server**)
- `/application add <name> [role]` - Add a position, writing its questions in a pop-up (**Manage Server**)
- `/application edit <position> [role]` - Change a position's questions, and optionally its role (**Manage Server**)
- `/application remove <position>` - Remove a position (**Manage Server**)
- `/application panel <channel>` - Post the server's panel listing the positions, replacing (and deleting) the previous one if there is one, with an **Apply** button for each (up to 25). It can be posted before any positions exist, saying there are none open until some are added. The buttons keep working after restarts, and every posted panel updates itself when positions are added, changed or removed (**Manage Server**)
- `/application list` - List the positions, their questions and roles, and the review channel (**Manage Server**)

### Info Commands

- `/info server` - Show the server's details and statistics, with its roles on the following pages
- `/info channel [channel]` - Show a channel's details (this channel by default)
- `/info user [user]` - Show a user's account and server details (you by default). Also in the user right-click menu under Apps → **User Info**
- `/info bot` - Show the bot's details over several pages: its client (version, ping, owner, start time), stats (commands, servers, members), database, host, memory and CPU (admin)
- `/info staff` - List the server's owner, administrators (Administrator or Manage Server) and moderators (Moderate, Kick or Ban Members, or Manage Messages)

### Sticky Commands

A sticky message stays near the bottom of its channel: once enough messages are posted below it, the bot waits a short delay (5 seconds unless changed with `/sticky delay`), then deletes the old copy and posts it again at the bottom. All need **Manage Messages**.

- `/sticky stick <message> [count]` - Stick a message in this channel; it moves down after `count` messages (default 3). Use `\n` for new lines
- `/sticky unstick` - Remove this channel's sticky message
- `/sticky list` - List the server's sticky messages, 10 per page
- `/sticky delay <seconds>` - How long every sticky in the server waits before moving down once it's due, from 0 (straight away) to 300 seconds. The default is 5

### Reaction Roles Commands

Staff pick emojis for a message that's already in a channel: reacting with one gives its role, and taking the reaction off takes the role away. `message` is the message's link (right-click it, **Copy Message Link**), or its ID if it's in the channel you run the command in; the bot checks it can find the message first. All need **Manage Roles**, and the bot's highest role has to be above the roles it gives.

- `/reaction-roles add <message> <emoji> <role>` - Make reacting with the emoji give the role; the bot adds the reaction
- `/reaction-roles remove <message> <emoji>` - Stop the emoji giving a role, and take the bot's reaction off
- `/reaction-roles list` - List the server's reaction roles, by message

The bot tells the member in the channel when it gives or takes a role (or couldn't), and deletes that message after 5 seconds. Deleting the message removes its reaction roles.

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

### Developer Commands

Only the bot's owner can use these (the owner of its Discord application, or the members of the team that owns it). They work in every server and channel and can't be disabled.

- `/maintenance` - Turn maintenance mode on or off. While it's on, everyone else is told the bot is under maintenance when they use a command, and the bot's status shows Do Not Disturb with "Under maintenance". Background features (honeypots, logs, AutoMod, staff policy...) keep running. It stays on across restarts
- `/presence <status> [activity] [text]` - Set the bot's status (Online, Idle, Do Not Disturb or Invisible) and activity (a custom status, Playing, Watching, Listening to, Competing in, or none). Without `text` it shows "Development of <bot> v<version>". It's kept across restarts; maintenance mode's status shows instead while it's on
- `/leave <server>` - Make the bot leave a server, picked from a list of the servers it's in, after a confirm button. Servers owned by a bot owner can't be left this way
- `/blacklist add <server> <type>` - Leave a server, after a confirm button, and blacklist either the server (the bot leaves straight away if it's added again) or its owner (the bot leaves any server they own, now or later, and they can't use its commands). Servers owned by a bot owner, and bot owners themselves, can't be blacklisted
- `/blacklist remove <entry>` - Take a server or user off the blacklist, picked from a list
- `/blacklist list` - List the blacklisted servers and users

### Server Setup Commands

Admins tell the bot which channels to use. Settings are kept by ID, so renaming a channel is fine, but one that's deleted and recreated has to be set again.

- `/server-setup logs <type> <channel>` - Where each type of server log goes (see Server Logs)
- `/server-setup commands <channel>` - Members can only use commands there (staff, and some cogs' commands, work anywhere)
- `/server-setup games <channel>` - Where `/hack`, `/heist`, `/fight` and `/defuse` are played. Other messages there are deleted
- `/server-setup announcements <channel>` - Where `/announce message` posts
- `/server-setup reset` - Clear all of the above
- `/server-setup help` - Show the commands and what's set

### Cog Management Commands

Cogs: `afk`, `announcements`, `applications`, `automod`, `economy`, `games`, `general`, `honeypot`, `info`, `levels`, `lockdown`, `logs`, `moderation`, `pets`, `profile`, `rules`, `serverstats`, `staffpolicy`, `sticky`, `suggestions`, `tickets`, `cogmanager`. Only `cogmanager` is enabled by default; an admin turns on the others per server with `/enablecog`. Disabling one blocks its commands and background work in that server only. `cogmanager` can't be disabled.

- `/enablecog <cog_name>` - Enable a cog for this server
- `/disablecog <cog_name>` - Disable a cog for this server
- `/enablegame <game>` / `/disablegame <game>` - Turn single games (`counting`, `defuse`, `fight`, `guessing`, `hack`, `heist`) on or off (only available while the Games cog is enabled). Every game is off by default, so after `/enablecog games` turn on the ones you want.
- `/cooldown set <command> <seconds> [scope]` - Set a command's cooldown (0 removes it). Scope `User` (default) makes each member wait on their own; `Global` makes one use put the command on cooldown for the whole server. `guessing` can be set too, for guesses in the guessing channel
- `/cooldown list` - List the commands with a cooldown and their scope
- `/gamestatus` - Check which games are enabled for this server (only available while the Games cog is enabled)
- `/games` - Anyone can list the enabled games played in the games channel (`defuse`, `fight`, `hack`, `heist`; not counting or guessing, which have their own channels), with what each one is (only available while the Games cog is enabled)
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
6. **Action**: Applies configured punishment (ban/kick/timeout/quarantine) or logs only
7. **Notification**: Sends alerts to admins via DM or channel
8. **Cross-Server Intelligence**: Tracks offenders across all servers
9. **Storage**: All activity stored in SQLite database (`database/honeypot.db`)
10. **Review**: Admins can view logs and export data for analysis

## Database

The bot uses SQLite for persistent storage:
- `database/honeypot.db` - Honeypot configuration, activity logs, offenders, audit log
- `database/afk.db` - Members who are AFK
- `database/applications.db` - Application positions, the review channel and applications
- `database/blacklist.db` - Blacklisted servers and users
- `database/cog_config.db` - Per-server cog enable/disable configuration
- `database/tickets.db` - Ticket system setup and tickets
- `database/suggestions.db` - Suggestion channels and open suggestions
- `database/maintenance` - Exists while maintenance mode is on
- `database/presence.json` - The bot's status and activity set with `/presence`
- `database/staff_policy.db` - Roles taken by the staff policy, for restoring
- `database/sticky.db` - Sticky messages
- `database/reaction_roles.db` - Reaction role emojis and the roles they give
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
├── macros/               # #[command] and #[derive(Choice)] for the framework
├── migrations/             # Each store's tables, as SQL (built into the bot)
├── tests/                  # Integration tests (run with `cargo test`)
└── src/
    ├── main.rs             # Entry point; runs core::gateway
    ├── lib.rs              # The bot as a library, so tests/ can use it
    ├── core/
    │   ├── framework/      # Our slash command framework: commands, options, replies, modals
    │   ├── gateway.rs      # Opens the databases, builds the framework, connects
    │   ├── cog_data.rs     # Which database tables each cog keeps, for the dashboard's Info pages
    │   ├── command_router.rs # Every registered command, and command error replies
    │   ├── dashboard.rs    # The web dashboard on port 8080 (page in assets/dashboard/)
    │   ├── event_router.rs # Sends gateway events to features; startup work and loops
    │   ├── permissions.rs  # The check before every command (blacklist, cog enabled, cooldowns)
    │   ├── error.rs        # Error and Context types
    │   └── state.rs        # Shared state: a handle to every store
    ├── database/
    │   ├── connection.rs   # Opens the SQLite files under database/
    │   ├── migrations.rs   # Runs migrations/*.sql; adds columns for older databases
    │   ├── guild_config.rs # Per-server cog/game enabled flags
    │   └── repositories/   # One store per feature: the data each feature keeps
    ├── shared/
    │   ├── log.rs          # Colored console logging
    │   ├── models.rs       # Types several features share (e.g. a member on an image card)
    │   ├── pagination.rs   # Previous / Next / Delete buttons for multi-page replies
    │   ├── render.rs       # Shared drawing code for the image cards
    │   └── utils.rs        # Timestamp and text helpers
    └── features/           # One module per feature ("cog")
        ├── mod.rs          # Cog list and cog helpers
        ├── cog_manager.rs  # Per-server enable/disable
        ├── developer.rs    # Bot owner commands: /leave, /blacklist and /maintenance
        ├── presence.rs     # The bot's status and activity, and /presence
        ├── honeypot/       # Message trap, punishments, weekly summary, commands
        ├── moderation/     # /ban, /kick, /timeout…, plus automod, lockdown, staff_policy
        ├── welcome/        # Welcome and boost images (card.rs draws them)
        ├── leveling/       # XP, /rank, /leaderboard and the rank card
        ├── tickets/        # Panel, buttons, transcripts, /ticket
        ├── logging/        # Server logs for /server-setup logs channels
        ├── economy/        # /eco commands
        ├── games/          # /hack, /heist, /defuse, /fight, counting, guessing
        ├── pets/           # /pet commands and the pet card
        └── …               # afk, announce, applications, general, info, profile,
                            # reaction_roles, rules, server_setup, stats, sticky, suggestions, cooldowns
```

## Adding More Cogs

1. Create `src/features/your_cog.rs` and add `pub mod your_cog;` to `src/features/mod.rs`
2. Add its display name (e.g. `"YourCog"`) to `COGS` in `src/features/mod.rs`
3. Write commands with `#[command]` (from `crate::core::framework`) and `category = "YourCog"` so the per-server enable/disable check applies:
   ```rust
   /// Description shown in Discord
   #[command(slash_command, guild_only, ephemeral, category = "YourCog")]
   pub async fn yourcommand(ctx: Context<'_>) -> Result<(), Error> {
       ctx.say("Hello!").await?;
       Ok(())
   }
   ```
4. Register the command in `commands()` in `src/core/command_router.rs`
5. Route its events from `event_handler` in `src/core/event_router.rs`; there, check `data.is_cog_enabled(guild_id, "yourcog")` yourself
6. If it stores data, add its store in `src/database/repositories/your_cog.rs` (registered in `repositories/mod.rs`), with its tables in `migrations/your_cog.sql`, and give `State` in `src/core/state.rs` a field for it
7. Use `log::info()`, `log::error()`, `log::warning()` or `log::debug()` for colored console output

## Security Notes

- Never commit your `.env` file or Discord token
- The bot requires administrator permissions for full functionality
- Honeypot logs are stored locally in SQLite database (`database/honeypot.db`)
- Consider implementing database backups for production deployments
- Review permissions carefully when inviting the bot to servers
- Use the whitelist feature to exempt trusted users from detection
