# Discord Honeypot Bot

A Python Discord bot with cogs architecture featuring a honeypot system for multi-server monitoring.

## Features

- **Multi-server support**: Works across multiple Discord servers
- **Honeypot channels**: Set up channels that log all messages for security monitoring
- **Visual warning embed**: Displays a professional warning message with ban counter
- **Auto-ban functionality**: Automatically ban users who send messages in honeypot channels
- **Activity logging**: Tracks suspicious activity in honeypot channels
- **Slash commands**: Modern Discord slash command support
- **Per-server configuration**: Each server can have its own honeypot channel
- **Auto-updating counter**: Ban counter updates automatically when users trigger the honeypot

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
