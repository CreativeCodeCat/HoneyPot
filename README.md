# Discord Honeypot Bot

A Python Discord bot with cogs architecture featuring a comprehensive honeypot system for multi-server monitoring and security.

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
- **Voice channel stats**: Auto-updating voice channel member counts
- **SQLite database**: Persistent storage with automatic migration from JSON

## Setup

1. **Install dependencies**:
   ```bash
   pip install -r requirements.txt
   ```

2. **Configure environment variables**:
   - Copy `.env.example` to `.env`
   - Add your Discord bot token to `.env`:
     ```
     DISCORD_TOKEN=your_bot_token_here
     ```

3. **Create a Discord bot**:
   - Go to https://discord.com/developers/applications
   - Create a new application
   - Enable bot features under the "Bot" tab
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
   python bot.py
   ```

## Commands

All commands can be used with `!` prefix or as slash commands (`/`). Most commands require administrator permissions.

### Honeypot Commands

**Channel Management:**
- `/sethoneypot [channel] [auto_ban] [message]` - Set a channel as a honeypot
- `/removehoneypot [channel]` - Remove a honeypot channel
- `/applytemplate [template] [channel]` - Apply a pre-configured template to a channel
  - Templates: `strict`, `lenient`, `stealth`, `moderate`, `newbie`, `investigation`

**Configuration:**
- `/setpunishment [type] [duration] [channel]` - Set punishment type (ban/kick/timeout)
- `/setaccountage [days] [channel]` - Set minimum account age requirement
- `/setadminchannel [channel]` - Set channel for admin notifications
- `/toggleadminnotifications` - Toggle admin DM notifications
- `/addadmin [user]` - Add user to admin notification list
- `/removeadmin [user]` - Remove user from admin notification list

**Whitelist:**
- `/whitelistuser [user] [channel]` - Whitelist a user from honeypot detection
- `/unwhitelistuser [user] [channel]` - Remove user from whitelist

**Monitoring & Logs:**
- `/honeypotstatus` - Check honeypot configuration for this server
- `/honeypotlogs [limit]` - View recent honeypot activity logs
- `/clearhoneypotlogs` - Clear honeypot activity logs for this server
- `/exportlogs` - Export honeypot logs to CSV
- `/auditlog [limit]` - View configuration audit log
- `/checkoffender [user]` - Check if user is in offender list
- `/crossservercheck [user]` - Check user's activity across all servers

### Cog Management Commands

- `/enablecog [cog_name]` - Enable a cog for this server
- `/disablecog [cog_name]` - Disable a cog for this server
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

The bot automatically migrates from old JSON files to SQLite on first run. Backed up JSON files are stored in the `database/` directory.

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
├── bot.py              # Main bot file
├── config.py           # Configuration management
├── colors.py           # Centralized logging utilities
├── requirements.txt    # Python dependencies
├── .env.example        # Environment variables template
├── .gitignore         # Git ignore rules
├── database/           # SQLite database files
│   ├── honeypot.db     # Honeypot data
│   └── cog_config.db   # Cog configuration
├── cogs/
│   ├── __init__.py
│   ├── honeypot.py     # Honeypot cog implementation
│   ├── cog_manager.py  # Cog management system
│   └── stats.py        # Voice channel stats
└── README.md
```

## Adding More Cogs

To add a new cog:

1. Create a new file in the `cogs/` directory (e.g., `cogs/your_cog.py`)
2. Import the logging utility: `from colors import log`
3. Create a class that inherits from `commands.Cog`
4. Implement the `setup` function at the bottom:
   ```python
   async def setup(bot):
       await bot.add_cog(YourCogName(bot))
   ```
5. The bot will automatically load it on startup
6. Use `log.info()`, `log.error()`, `log.warning()`, or `log.debug()` for colored console output

## Security Notes

- Never commit your `.env` file or Discord token
- The bot requires administrator permissions for full functionality
- Honeypot logs are stored locally in SQLite database (`database/honeypot.db`)
- Consider implementing database backups for production deployments
- Review permissions carefully when inviting the bot to servers
- Use the whitelist feature to exempt trusted users from detection

## Setup

1. **Install dependencies**:
   ```bash
   pip install -r requirements.txt
   ```

2. **Configure environment variables**:
   - Copy `.env.example` to `.env`
   - Add your Discord bot token to `.env`:
     ```
     DISCORD_TOKEN=your_bot_token_here
     ```

3. **Create a Discord bot**:
   - Go to https://discord.com/developers/applications
   - Create a new application
   - Enable bot features under the "Bot" tab
   - Copy the token and add it to your `.env` file
   - Under "OAuth2 > URL Generator", select:
     - `bot`
     - `applications.commands`
     - Under "Bot Permissions", select:
       - `Read Messages/View Channels`
       - `Send Messages`
       - `Ban Members` (required for auto-ban functionality)
       - `Manage Channels` (for honeypot setup)
       - `Administrator` (recommended for full functionality)
   - Use the generated URL to invite the bot to your server

4. **Run the bot**:
   ```bash
   python bot.py
   ```

## Commands

All commands can be used with `!` prefix or as slash commands (`/`).

### Honeypot Commands

- `/sethoneypot [channel] [auto_ban] [message]` - Set a channel as a honeypot (Admin only)
  - If no channel specified, uses the current channel
  - Set `auto_ban` to `true` to automatically ban users who send messages
  - Optionally provide a custom message for the honeypot embed
  - All messages in this channel will be logged

- `/removehoneypot` - Remove the honeypot channel (Admin only)

- `/honeypotstatus` - Check if a honeypot is configured for this server

- `/honeypotlogs [limit]` - View recent honeypot activity logs (Admin only)
  - Default: shows last 10 entries
  - Specify a number to show more/fewer entries

- `/clearhoneypotlogs` - Clear honeypot activity logs for this server (Admin only)

- `/resetbancount` - Reset the ban counter to 0 (Admin only)

- `/toggleautoban` - Toggle auto-ban on/off for the honeypot (Admin only)

- `/bancount` - Check the current ban count and auto-ban status (Admin only)

- `/setmessage [message]` - Set a custom message for the honeypot embed (Admin only)

## How It Works

1. **Setup**: An admin uses `/sethoneypot` to designate a channel as a honeypot (optionally with auto-ban)
2. **Visual Warning**: The bot posts a stylish embed with a warning message and ban counter
3. **Monitoring**: The bot logs all messages sent to honeypot channels
4. **Auto-Ban**: If enabled, users who send messages are automatically banned
5. **Auto-Update**: The ban counter automatically updates when users send messages
6. **Storage**: Activity is stored in JSON files per server (`honeypot_activity_[guild_id].json`)
7. **Review**: Admins can view logs with `/honeypotlogs` to analyze suspicious activity

## Use Cases

- Monitor channels that might attract automated bots or malicious users
- Automatically ban spam bots and suspicious users
- Track spam attempts and ban history
- Identify suspicious patterns in user behavior
- Debug bot interactions in a controlled environment

## Project Structure

```
HoneyPot/
├── bot.py              # Main bot file
├── config.py           # Configuration management
├── requirements.txt    # Python dependencies
├── .env.example        # Environment variables template
├── .gitignore         # Git ignore rules
├── cogs/
│   ├── __init__.py
│   └── honeypot.py    # Honeypot cog implementation
└── README.md
```

## Adding More Cogs

To add a new cog:

1. Create a new file in the `cogs/` directory (e.g., `cogs/moderation.py`)
2. Create a class that inherits from `commands.Cog`
3. Implement the `setup` function at the bottom:
   ```python
   async def setup(bot):
       await bot.add_cog(YourCogName(bot))
   ```
4. The bot will automatically load it on startup

## Security Notes

- Never commit your `.env` file or Discord token
- The bot requires administrator permissions for full functionality
- Honeypot logs are stored locally in JSON files
- Consider implementing log rotation for long-running deployments
