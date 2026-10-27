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
- **Voice channel stats**: Voice channels named like `MEMBERS`, `BOTS`, `ROLES`, `BOOSTS` or `CHANNELS` are renamed with live counts every minute
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
       - `Manage Channels` (for honeypot setup)
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
- `/honeypotlogs [limit]` - View recent honeypot activity logs (up to 12)
- `/clearhoneypotlogs` - Clear honeypot activity logs for this server
- `/exportlogs` - Export honeypot logs to CSV
- `/auditlog [limit]` - View configuration audit log (up to 25)
- `/bancount` - Show the total number of honeypot bans
- `/resetbancount` - Reset the ban counter
- `/checkoffender [user]` - Check if user is in offender list
- `/crossservercheck [user]` - Check user's activity across all servers

### Other Commands

- `/automod <rule_type> [mention_limit]` - Create a Discord AutoMod rule (flagged words, capitals, spoilers, emojis, headers, unicode, code blocks, bullet lists, invites, mention spam, message spam)
- `/ping` - Show latency and uptime (5-minute cooldown)

### Cog Management Commands

Cogs: `automod`, `general`, `honeypot`, `serverstats`, `cogmanager`. All cogs are enabled by default. Disabling one blocks its commands and background work in that server only. `cogmanager` can't be disabled.

- `/enablecog <cog_name>` - Enable a cog for this server
- `/disablecog <cog_name>` - Disable a cog for this server
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
├── .env.example            # Environment variables template
├── database/               # SQLite database files (created at runtime)
│   ├── honeypot.db         # Honeypot data
│   └── cog_config.db       # Cog configuration
└── src/
    ├── main.rs             # Startup, event dispatch, cog check, error handling
    ├── log.rs              # Colored console logging
    └── cogs/
        ├── mod.rs          # Shared state, cog list, helpers
        ├── cog_manager.rs  # Per-server enable/disable
        ├── general.rs      # /ping
        ├── automod.rs      # /automod
        ├── stats.rs        # Voice channel stats
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
