import discord
from discord.ext import commands
import json
import os
import sqlite3
from discord import app_commands
import sys
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from colors import log

class CogManager(commands.Cog):
    """Manage cog enable/disable status per server"""
    
    def __init__(self, bot):
        self.bot = bot
        self.config_file = 'cog_config.json'
        self.db_dir = 'database'
        self.db_file = os.path.join(self.db_dir, 'cog_config.db')
        self.cog_config = {}
        
        # Create database directory if it doesn't exist
        os.makedirs(self.db_dir, exist_ok=True)
        
        # Migrate old database file if it exists in root
        if os.path.exists('cog_config.db') and not os.path.exists(self.db_file):
            log.info(f"Migrating cog_config.db to {self.db_file}")
            os.rename('cog_config.db', self.db_file)
        
        self.init_database()
        self.migrate_from_json()
        self.load_config()
    
    def init_database(self):
        """Initialize SQLite database for cog configuration"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        cursor.execute('''
            CREATE TABLE IF NOT EXISTS cog_config (
                guild_id INTEGER,
                cog_name TEXT,
                enabled BOOLEAN DEFAULT 1,
                PRIMARY KEY (guild_id, cog_name)
            )
        ''')
        
        conn.commit()
        conn.close()
    
    def migrate_from_json(self):
        """Migrate existing JSON config to SQLite"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        # Check if database is empty
        cursor.execute('SELECT COUNT(*) FROM cog_config')
        config_count = cursor.fetchone()[0]
        
        if config_count > 0:
            conn.close()
            return
        
        # Migrate from JSON if it exists
        config_path = os.path.join(os.getcwd(), self.config_file)
        if os.path.exists(config_path):
            try:
                with open(config_path, 'r') as f:
                    data = json.load(f)
                
                for guild_id_str, cog_data in data.items():
                    guild_id = int(guild_id_str)
                    for cog_name, enabled in cog_data.items():
                        cursor.execute('''
                            INSERT OR REPLACE INTO cog_config (guild_id, cog_name, enabled)
                            VALUES (?, ?, ?)
                        ''', (guild_id, cog_name, 1 if enabled else 0))
                
                conn.commit()
                
                # Backup old file to database directory
                backup_file = os.path.join(self.db_dir, f'{os.path.basename(self.config_file)}.backup')
                os.rename(config_path, backup_file)
                log.info("Migrated cog config to SQLite")
                
            except Exception as e:
                print(f"Error migrating cog config: {e}")
                conn.rollback()
        
        conn.close()
    
    def load_config(self):
        """Load cog configuration from SQLite database"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('SELECT guild_id, cog_name, enabled FROM cog_config')
            rows = cursor.fetchall()
            
            for guild_id, cog_name, enabled in rows:
                if guild_id not in self.cog_config:
                    self.cog_config[guild_id] = {}
                self.cog_config[guild_id][cog_name] = bool(enabled)
            
        except Exception as e:
            print(f"Error loading cog config: {e}")
            self.cog_config = {}
        finally:
            conn.close()
    
    def cleanup_stale_config(self):
        """Remove config for guilds that no longer exist"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('SELECT DISTINCT guild_id FROM cog_config')
            guilds = cursor.fetchall()
            
            for (guild_id,) in guilds:
                # Check if guild still exists
                guild = self.bot.get_guild(guild_id)
                if not guild:
                    print(f"Guild {guild_id} no longer exists, removing from cog config")
                    cursor.execute('DELETE FROM cog_config WHERE guild_id = ?', (guild_id,))
            
            conn.commit()
            
        except Exception as e:
            print(f"Error cleaning up stale cog config: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    async def cog_name_autocomplete(self, interaction: discord.Interaction, current: str):
        """Autocomplete for cog names"""
        # Get all loaded cogs
        loaded_cogs = list(self.bot.cogs.keys())
        
        # Filter out CogManager since it can't be disabled
        filtered = [cog for cog in loaded_cogs if cog != 'CogManager']
        
        # Filter by current input (case-insensitive)
        filtered = [cog for cog in filtered if current.lower() in cog.lower()]
        
        # Return up to 25 choices
        return [
            app_commands.Choice(name=cog, value=cog.lower().replace(' ', ''))
            for cog in filtered[:25]
        ]
    
    def get_normalized_cog_name(self, cog_class_name):
        """Get the normalized name used in config for a cog class name"""
        # Map class names to normalized config names
        name_map = {
            'HoneyPot': 'honeypot',
            'ServerStats': 'serverstats',
            'CogManager': 'cogmanager'
        }
        return name_map.get(cog_class_name, cog_class_name.lower().replace(' ', ''))
    
    def save_config(self):
        """Save cog configuration to SQLite database"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            # Clear existing config
            cursor.execute('DELETE FROM cog_config')
            
            # Insert all config
            for guild_id, cog_data in self.cog_config.items():
                for cog_name, enabled in cog_data.items():
                    cursor.execute('''
                        INSERT INTO cog_config (guild_id, cog_name, enabled)
                        VALUES (?, ?, ?)
                    ''', (guild_id, cog_name, 1 if enabled else 0))
            
            conn.commit()
        except Exception as e:
            print(f"Error saving cog config: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    def is_cog_enabled(self, guild_id, cog_name):
        """Check if a cog is enabled for a guild"""
        if guild_id not in self.cog_config:
            # Default to enabled if not configured
            return True
        
        return self.cog_config[guild_id].get(cog_name, True)
    
    def set_cog_enabled(self, guild_id, cog_name, enabled):
        """Set a cog's enabled status for a guild"""
        if guild_id not in self.cog_config:
            self.cog_config[guild_id] = {}
        
        self.cog_config[guild_id][cog_name] = enabled
        self.save_config()
    
    @commands.Cog.listener()
    async def on_ready(self):
        """Run cleanup when bot is ready"""
        log.info("[CogManager] Cog loaded")
        self.cleanup_stale_config()
    
    @commands.Cog.listener()
    async def on_guild_join(self, guild):
        """Initialize config for new guild"""
        if guild.id not in self.cog_config:
            self.cog_config[guild.id] = {}
            self.save_config()
    
    @commands.hybrid_command(name='enablecog', description='Enable a cog for this server')
    @commands.has_permissions(administrator=True)
    @app_commands.autocomplete(cog_name=cog_name_autocomplete)
    async def enablecog(self, ctx, cog_name: str):
        """Enable a cog for this server"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        # Normalize cog name
        cog_name = cog_name.lower().replace(' ', '')
        
        self.set_cog_enabled(guild_id, cog_name, True)
        
        if ctx.interaction:
            await ctx.send(f"✅ Cog '{cog_name}' has been enabled for this server.", ephemeral=True)
        else:
            await ctx.send(f"✅ Cog '{cog_name}' has been enabled for this server.")
    
    @commands.hybrid_command(name='disablecog', description='Disable a cog for this server')
    @commands.has_permissions(administrator=True)
    @app_commands.autocomplete(cog_name=cog_name_autocomplete)
    async def disablecog(self, ctx, cog_name: str):
        """Disable a cog for this server"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        # Normalize cog name
        cog_name = cog_name.lower().replace(' ', '')
        
        # Prevent disabling the CogManager itself
        if cog_name == 'cogmanager':
            if ctx.interaction:
                await ctx.send("❌ You cannot disable the CogManager cog.", ephemeral=True)
            else:
                await ctx.send("❌ You cannot disable the CogManager cog.")
            return
        
        self.set_cog_enabled(guild_id, cog_name, False)
        
        if ctx.interaction:
            await ctx.send(f"✅ Cog '{cog_name}' has been disabled for this server.", ephemeral=True)
        else:
            await ctx.send(f"✅ Cog '{cog_name}' has been disabled for this server.")
    
    @commands.hybrid_command(name='cogstatus', description='Check the status of all cogs for this server')
    @commands.has_permissions(administrator=True)
    async def cogstatus(self, ctx):
        """Check the status of all cogs for this server"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        # Get all loaded cogs
        loaded_cogs = list(self.bot.cogs.keys())
        
        # Normalize cog names for display
        cog_info = []
        for cog_name in loaded_cogs:
            normalized = cog_name.lower().replace(' ', '')
            status = "✅ Enabled" if self.is_cog_enabled(guild_id, normalized) else "❌ Disabled"
            cog_info.append(f"{status}: {cog_name}")
        
        if not cog_info:
            status_msg = "No cogs are currently loaded."
        else:
            status_msg = "\n".join(cog_info)
        
        embed = discord.Embed(
            title="Cog Status",
            description=status_msg,
            color=discord.Color.blue()
        )
        
        if ctx.interaction:
            await ctx.send(embed=embed, ephemeral=True)
        else:
            await ctx.send(embed=embed)

async def setup(bot):
    await bot.add_cog(CogManager(bot))
