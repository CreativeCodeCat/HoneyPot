import unicodedata
import discord
from discord.ext import commands, tasks


STAT_MAPPINGS = {
    "MEMBERS": lambda g: sum(1 for m in g.members if not m.bot),
    "BOTS": lambda g: sum(1 for m in g.members if m.bot),
    "ROLES": lambda g: len(g.roles),
    "BOOSTS": lambda g: g.premium_subscription_count or 0,
    "CHANNELS": lambda g: len(g.channels),
}


SMALL_CAPS = str.maketrans(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "ᴀʙᴄᴅᴇғɢʜɪᴊᴋʟᴍɴᴏᴘǫʀsᴛᴜᴠᴡxʏᴢ"
)


SMALL_CAPS_TO_ASCII = str.maketrans(
    "ᴀʙᴄᴅᴇғɢʜɪᴊᴋʟᴍɴᴏᴘǫʀsᴛᴜᴠᴡxʏᴢ",
    "abcdefghijklmnopqrstuvwxyz"
)


def normalize_text(text: str) -> str:
    """Convert normal and small-cap Unicode text to uppercase ASCII."""

    text = text.translate(SMALL_CAPS_TO_ASCII)

    normalized = unicodedata.normalize("NFKD", text)

    normalized = "".join(
        c for c in normalized
        if not unicodedata.combining(c)
    )

    return normalized.upper()


class ServerStats(commands.Cog):

    def __init__(self, bot: commands.Bot):
        self.bot = bot
        self.update_stats_task.start()
    
    def is_enabled(self, guild_id):
        """Check if this cog is enabled for the guild"""
        cog_manager = self.bot.get_cog('CogManager')
        if cog_manager:
            return cog_manager.is_cog_enabled(guild_id, 'serverstats')
        return True
    
    async def cog_check(self, ctx):
        """Check if cog is enabled before running any command"""
        if not ctx.guild:
            return True
        return self.is_enabled(ctx.guild.id)

    def cog_unload(self):
        self.update_stats_task.cancel()

    async def update_guild_channels(self, guild: discord.Guild):
        # Check if cog is enabled for this guild
        if not self.is_enabled(guild.id):
            return

        for channel in guild.voice_channels:

            try:
                normalized_name = normalize_text(channel.name)

                for keyword, stat_func in STAT_MAPPINGS.items():

                    if keyword not in normalized_name:
                        continue

                    value = stat_func(guild)

                    small_caps_keyword = keyword.translate(SMALL_CAPS)

                    new_name = f"【📺】{small_caps_keyword}: {value}"

                    if channel.name == new_name:
                        break

                    try:
                        await channel.edit(
                            name=new_name,
                            reason="Automatic server statistics update"
                        )

                    except discord.Forbidden:
                        pass

                    except discord.HTTPException:
                        pass

                    break

            except Exception:
                pass

    @tasks.loop(minutes=1)
    async def update_stats_task(self):

        for guild in self.bot.guilds:
            await self.update_guild_channels(guild)

    @update_stats_task.before_loop
    async def before_update(self):
        await self.bot.wait_until_ready()

    @commands.Cog.listener()
    async def on_member_join(self, member: discord.Member):
        if not self.is_enabled(member.guild.id):
            return
        await self.update_guild_channels(member.guild)

    @commands.Cog.listener()
    async def on_member_remove(self, member: discord.Member):
        if not self.is_enabled(member.guild.id):
            return
        await self.update_guild_channels(member.guild)

    @commands.Cog.listener()
    async def on_guild_channel_create(
        self,
        channel: discord.abc.GuildChannel
    ):
        if not self.is_enabled(channel.guild.id):
            return
        await self.update_guild_channels(channel.guild)

    @commands.Cog.listener()
    async def on_guild_channel_delete(
        self,
        channel: discord.abc.GuildChannel
    ):
        if not self.is_enabled(channel.guild.id):
            return
        await self.update_guild_channels(channel.guild)


async def setup(bot: commands.Bot):
    await bot.add_cog(ServerStats(bot))