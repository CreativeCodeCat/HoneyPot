import discord
from discord.ext import commands
from discord import app_commands
import sys
import os
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from colors import log
from datetime import datetime, timedelta, timezone

class General(commands.Cog):
    """General utility commands"""
    
    def __init__(self, bot):
        self.bot = bot
        self.start_time = datetime.now(timezone.utc)
    
    @commands.Cog.listener()
    async def on_ready(self):
        log.info("[General] Cog loaded")
    
    @commands.hybrid_command(name='ping', description='Check bot latency and uptime')
    @commands.cooldown(1, 300, commands.BucketType.user)  # 5 minute cooldown per user
    async def ping(self, ctx):
        """Check bot latency and uptime"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        # Calculate bot latency
        if ctx.interaction:
            # For slash commands: time from interaction creation to now
            command_latency = round((datetime.now(timezone.utc) - ctx.interaction.created_at).total_seconds() * 1000)
        else:
            # For prefix commands: time from message creation to now
            command_latency = round((datetime.now(timezone.utc) - ctx.message.created_at).total_seconds() * 1000)
        
        # Get API latency (WebSocket ping)
        bot_latency = round(self.bot.latency * 1000)
        
        # Calculate uptime
        uptime = datetime.now(timezone.utc) - self.start_time
        days = uptime.days
        hours, remainder = divmod(uptime.seconds, 3600)
        minutes, seconds = divmod(remainder, 60)
        
        embed = discord.Embed(
            title="🏓 Bot Status",
            color=discord.Color.blue()
        )
        
        embed.add_field(
            name="`Command Latency`",
            value=f"{command_latency} ms",
            inline=False
        )
        
        embed.add_field(
            name="`Bot Latency`",
            value=f"{bot_latency} ms",
            inline=False
        )
        
        embed.add_field(
            name="`Bot Uptime`",
            value=f"{days} Days, {hours} Hrs, {minutes} Min, {seconds} Sec",
            inline=False
        )
        
        if ctx.interaction:
            await ctx.send(embed=embed, ephemeral=True)
        else:
            await ctx.send(embed=embed)
    
    @ping.error
    async def ping_error(self, ctx, error):
        """Handle ping command errors"""
        if isinstance(error, commands.CommandOnCooldown):
            if ctx.interaction:
                await ctx.send(f"⏱️ This command is on cooldown. Try again in {error.retry_after:.0f} seconds.", ephemeral=True)
            else:
                await ctx.send(f"⏱️ This command is on cooldown. Try again in {error.retry_after:.0f} seconds.")

async def setup(bot):
    await bot.add_cog(General(bot))
