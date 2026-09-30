import discord
from discord.ext import commands
import config
import os
from colors import log

intents = discord.Intents.default()
intents.message_content = True
intents.members = True
intents.guilds = True

bot = commands.Bot(command_prefix='!', intents=intents)

@bot.event
async def on_ready():
    log.info(f'{bot.user} has connected to Discord!')
    log.blue(f'Connected to {len(bot.guilds)} guilds')
    
    # Clear commands for each guild
    for guild in bot.guilds:
        try:
            bot.tree.clear_commands(guild=guild)
            await bot.tree.sync(guild=guild)
            log.warning(f'Cleared commands for guild: {guild.name}')
        except Exception as e:
            log.error(f'Error clearing commands for guild {guild.name}: {e}')
    
    # Sync global commands
    try:
        synced = await bot.tree.sync()
        log.info(f'Synced {len(synced)} global command(s)')
    except Exception as e:
        log.error(f'Error syncing global commands: {e}')

@bot.event
async def on_guild_join(guild):
    log.info(f'Joined new guild: {guild.name} (ID: {guild.id})')

@bot.event
async def on_guild_remove(guild):
    log.warning(f'Left guild: {guild.name} (ID: {guild.id})')

async def load_extensions():
    """Load all cogs from the cogs directory"""
    for filename in os.listdir('./cogs'):
        if filename.endswith('.py') and filename != '__init__.py':
            try:
                await bot.load_extension(f'cogs.{filename[:-3]}')
                log.info(f'Loaded extension: cogs.{filename[:-3]}')
            except Exception as e:
                log.error(f'Failed to load extension {filename}: {e}')

async def main():
    async with bot:
        await load_extensions()
        await bot.start(config.DISCORD_TOKEN)

if __name__ == '__main__':
    import asyncio
    asyncio.run(main())
