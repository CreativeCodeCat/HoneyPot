import discord
from discord.ext import commands, tasks
from discord import app_commands
import json
import os
import csv
import re
import aiohttp
import sqlite3
from datetime import datetime, timedelta, timezone
import sys
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from colors import log

class HoneyPot(commands.Cog):
    """Honeypot cog for detecting and monitoring suspicious activity across multiple servers"""
    
    def __init__(self, bot):
        self.bot = bot
        self.honeypot_channels = {}
        self.db_dir = 'database'
        self.db_file = os.path.join(self.db_dir, 'honeypot.db')
        
        # Create database directory if it doesn't exist
        os.makedirs(self.db_dir, exist_ok=True)
        
        # Migrate old database file if it exists in root
        if os.path.exists('honeypot.db') and not os.path.exists(self.db_file):
            log.info(f"Migrating honeypot.db to {self.db_file}")
            os.rename('honeypot.db', self.db_file)
        
        self.init_database()
        self.migrate_from_json()
        self.load_honeypot_data()
        self.weekly_summary_task.start()
    
    def init_database(self):
        """Initialize SQLite database with required tables"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        # Guilds table
        cursor.execute('''
            CREATE TABLE IF NOT EXISTS guilds (
                guild_id INTEGER PRIMARY KEY,
                ban_count INTEGER DEFAULT 0,
                admin_role_id INTEGER,
                notification_channel_id INTEGER,
                notification_dm BOOLEAN DEFAULT 0,
                admin_user_ids TEXT
            )
        ''')
        
        # Honeypot channels table
        cursor.execute('''
            CREATE TABLE IF NOT EXISTS honeypot_channels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER,
                channel_id INTEGER,
                enabled BOOLEAN DEFAULT 1,
                auto_ban BOOLEAN DEFAULT 0,
                custom_message TEXT,
                punishment_type TEXT DEFAULT 'ban',
                punishment_duration INTEGER DEFAULT 0,
                whitelisted_users TEXT,
                whitelisted_roles TEXT,
                min_account_age_days INTEGER DEFAULT 0,
                check_patterns BOOLEAN DEFAULT 1,
                FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
            )
        ''')
        
        # Activity logs table
        cursor.execute('''
            CREATE TABLE IF NOT EXISTS activity_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER,
                user_id INTEGER,
                username TEXT,
                action TEXT,
                details TEXT,
                timestamp TEXT,
                FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
            )
        ''')
        
        # Index for faster user_id lookups
        cursor.execute('CREATE INDEX IF NOT EXISTS idx_user_id ON activity_logs(user_id)')
        cursor.execute('CREATE INDEX IF NOT EXISTS idx_guild_id ON activity_logs(guild_id)')
        
        # Offenders table
        cursor.execute('''
            CREATE TABLE IF NOT EXISTS offenders (
                user_id INTEGER PRIMARY KEY,
                username TEXT,
                first_offense TEXT,
                last_offense TEXT,
                offense_count INTEGER DEFAULT 0,
                servers TEXT
            )
        ''')
        
        # Audit log table
        cursor.execute('''
            CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER,
                admin_id INTEGER,
                action TEXT,
                details TEXT,
                timestamp TEXT,
                FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
            )
        ''')
        
        conn.commit()
        conn.close()
    
    def migrate_from_json(self):
        """Migrate existing JSON data to SQLite"""
        # Check if database is empty
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        cursor.execute('SELECT COUNT(*) FROM guilds')
        guild_count = cursor.fetchone()[0]
        
        if guild_count > 0:
            log.info(f"[HONEYPOT] Database already has data, skipping migration")
            conn.close()
            return
        
        log.warning(f"[HONEYPOT] Starting migration from JSON to SQLite")
        
        # Migrate honeypot_logs.json
        json_file = os.path.join(os.getcwd(), 'honeypot_logs.json')
        if os.path.exists(json_file):
            try:
                with open(json_file, 'r') as f:
                    data = json.load(f)
                
                for guild_id_str, guild_data in data.items():
                    guild_id = int(guild_id_str)
                    
                    # Insert guild
                    cursor.execute('''
                        INSERT OR REPLACE INTO guilds 
                        (guild_id, ban_count, admin_role_id, notification_channel_id, notification_dm, admin_user_ids)
                        VALUES (?, ?, ?, ?, ?, ?)
                    ''', (
                        guild_id,
                        guild_data.get('ban_count', 0),
                        guild_data.get('admin_role_id'),
                        guild_data.get('notification_channel_id'),
                        1 if guild_data.get('notification_dm', False) else 0,
                        json.dumps(guild_data.get('admin_user_ids', []))
                    ))
                    
                    # Insert channels (handle both old and new structure)
                    if 'channels' in guild_data:
                        for channel in guild_data['channels']:
                            cursor.execute('''
                                INSERT INTO honeypot_channels 
                                (guild_id, channel_id, enabled, auto_ban, custom_message, 
                                 punishment_type, punishment_duration, whitelisted_users, 
                                 whitelisted_roles, min_account_age_days, check_patterns)
                                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                            ''', (
                                guild_id,
                                channel['channel_id'],
                                1 if channel.get('enabled', True) else 0,
                                1 if channel.get('auto_ban', False) else 0,
                                channel.get('custom_message'),
                                channel.get('punishment_type', 'ban'),
                                channel.get('punishment_duration', 0),
                                json.dumps(channel.get('whitelisted_users', [])),
                                json.dumps(channel.get('whitelisted_roles', [])),
                                channel.get('min_account_age_days', 0),
                                1 if channel.get('check_patterns', True) else 0
                            ))
                    elif 'channel_id' in guild_data:
                        # Old structure
                        cursor.execute('''
                            INSERT INTO honeypot_channels 
                            (guild_id, channel_id, enabled, auto_ban, custom_message, 
                             punishment_type, punishment_duration, whitelisted_users, 
                             whitelisted_roles, min_account_age_days, check_patterns)
                            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        ''', (
                            guild_id,
                            guild_data['channel_id'],
                            1 if guild_data.get('enabled', True) else 0,
                            1 if guild_data.get('auto_ban', False) else 0,
                            guild_data.get('custom_message'),
                            'ban',
                            0,
                            json.dumps([]),
                            json.dumps([]),
                            0,
                            1
                        ))
                
                conn.commit()
                log.info(f"[HONEYPOT] Migrated honeypot configuration")
                
                # Backup old file to database directory
                backup_file = os.path.join(self.db_dir, f'{os.path.basename(json_file)}.backup')
                os.rename(json_file, backup_file)
                log.info(f"[HONEYPOT] Backed up old JSON file")
                
            except Exception as e:
                log.error(f"Error migrating honeypot config: {e}")
                conn.rollback()
        
        # Migrate activity logs from current directory
        current_dir = os.getcwd()
        for filename in os.listdir(current_dir):
            if filename.startswith('honeypot_activity_') and filename.endswith('.json'):
                filepath = os.path.join(current_dir, filename)
                guild_id = int(filename.split('_')[2].split('.')[0])
                
                try:
                    with open(filepath, 'r') as f:
                        logs = json.load(f)
                    
                    for log_entry in logs:
                        cursor.execute('''
                            INSERT INTO activity_logs 
                            (guild_id, user_id, username, action, details, timestamp)
                            VALUES (?, ?, ?, ?, ?, ?)
                        ''', (
                            guild_id,
                            log_entry.get('user_id'),
                            log_entry.get('username'),
                            log_entry.get('action'),
                            json.dumps(log_entry.get('details', {})),
                            log_entry.get('timestamp')
                        ))
                    
                    conn.commit()
                    log.info(f"[HONEYPOT] Migrated activity logs for guild {guild_id}")
                    
                    # Backup old file to database directory
                    backup_file = os.path.join(self.db_dir, f'{os.path.basename(filename)}.backup')
                    os.rename(filepath, backup_file)
                    
                except Exception as e:
                    log.error(f"Error migrating activity logs for guild {guild_id}: {e}")
                    conn.rollback()
        
        # Migrate offender list
        offender_file = os.path.join(os.getcwd(), 'offender_list.json')
        if os.path.exists(offender_file):
            try:
                with open(offender_file, 'r') as f:
                    offenders = json.load(f)
                
                for user_id_str, offender_data in offenders.items():
                    cursor.execute('''
                        INSERT OR REPLACE INTO offenders 
                        (user_id, username, first_offense, last_offense, offense_count, servers)
                        VALUES (?, ?, ?, ?, ?, ?)
                    ''', (
                        int(user_id_str),
                        offender_data.get('username'),
                        offender_data.get('first_offense'),
                        offender_data.get('last_offense'),
                        offender_data.get('offense_count', 0),
                        json.dumps(offender_data.get('servers', []))
                    ))
                
                conn.commit()
                log.info(f"[HONEYPOT] Migrated offender list")
                
                # Backup old file to database directory
                backup_file = os.path.join(self.db_dir, f'{os.path.basename(offender_file)}.backup')
                os.rename(offender_file, backup_file)
                
            except Exception as e:
                log.error(f"Error migrating offender list: {e}")
                conn.rollback()
        
        conn.close()
        log.info(f"[HONEYPOT] Migration complete")
    
    def is_enabled(self, guild_id):
        """Check if this cog is enabled for the guild"""
        cog_manager = self.bot.get_cog('CogManager')
        if cog_manager:
            return cog_manager.is_cog_enabled(guild_id, 'honeypot')
        return True
    
    async def cog_check(self, ctx):
        """Check if cog is enabled before running any command"""
        if not ctx.guild:
            return True
        return self.is_enabled(ctx.guild.id)
    
    def cog_unload(self):
        """Clean up when cog is unloaded"""
        self.weekly_summary_task.cancel()
    
    @commands.Cog.listener()
    async def on_ready(self):
        """Run cleanup when bot is ready"""
        log.info(f"[HONEYPOT] Cog loaded")
        # Clean up stale data after bot is connected
        self.cleanup_stale_data()
    
    def load_honeypot_data(self):
        """Load honeypot channel data from SQLite database"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            # Load guilds
            cursor.execute('SELECT guild_id, ban_count, admin_role_id, notification_channel_id, notification_dm, admin_user_ids FROM guilds')
            guilds = cursor.fetchall()
            
            for guild_id, ban_count, admin_role_id, notification_channel_id, notification_dm, admin_user_ids in guilds:
                self.honeypot_channels[guild_id] = {
                    'ban_count': ban_count,
                    'admin_role_id': admin_role_id,
                    'notification_channel_id': notification_channel_id,
                    'notification_dm': bool(notification_dm),
                    'channels': [],
                    'admin_user_ids': json.loads(admin_user_ids) if admin_user_ids else []
                }
            
            # Load honeypot channels for each guild
            cursor.execute('''
                SELECT guild_id, channel_id, enabled, auto_ban, custom_message, 
                       punishment_type, punishment_duration, whitelisted_users, 
                       whitelisted_roles, min_account_age_days, check_patterns
                FROM honeypot_channels
            ''')
            channels = cursor.fetchall()
            
            for (guild_id, channel_id, enabled, auto_ban, custom_message, 
                 punishment_type, punishment_duration, whitelisted_users, 
                 whitelisted_roles, min_account_age_days, check_patterns) in channels:
                
                if guild_id in self.honeypot_channels:
                    self.honeypot_channels[guild_id]['channels'].append({
                        'channel_id': channel_id,
                        'enabled': bool(enabled),
                        'auto_ban': bool(auto_ban),
                        'custom_message': custom_message,
                        'punishment_type': punishment_type,
                        'punishment_duration': punishment_duration,
                        'whitelisted_users': json.loads(whitelisted_users) if whitelisted_users else [],
                        'whitelisted_roles': json.loads(whitelisted_roles) if whitelisted_roles else [],
                        'min_account_age_days': min_account_age_days,
                        'check_patterns': bool(check_patterns)
                    })
            
            log.info(f"[HONEYPOT] Loaded data for {len(self.honeypot_channels)} guilds")
            
        except Exception as e:
            log.error(f"Error loading honeypot data: {e}")
            self.honeypot_channels = {}
        finally:
            conn.close()
    
    def cleanup_stale_data(self):
        """Remove data for guilds and channels that no longer exist"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            # Load guilds
            cursor.execute('SELECT guild_id FROM guilds')
            guilds = cursor.fetchall()
            
            valid_guilds = []
            
            for (guild_id,) in guilds:
                # Check if guild still exists (bot is still in the server)
                guild = self.bot.get_guild(guild_id)
                if not guild:
                    log.warning(f"[HONEYPOT] Guild {guild_id} no longer exists, removing from database")
                    cursor.execute('DELETE FROM guilds WHERE guild_id = ?', (guild_id,))
                    cursor.execute('DELETE FROM honeypot_channels WHERE guild_id = ?', (guild_id,))
                    cursor.execute('DELETE FROM activity_logs WHERE guild_id = ?', (guild_id,))
                    cursor.execute('DELETE FROM audit_log WHERE guild_id = ?', (guild_id,))
                    continue
                
                valid_guilds.append(guild_id)
            
            # Load honeypot channels for each guild
            cursor.execute('''
                SELECT guild_id, channel_id
                FROM honeypot_channels
            ''')
            channels = cursor.fetchall()
            
            for guild_id, channel_id in channels:
                # Skip if guild was marked for deletion
                if guild_id not in valid_guilds:
                    continue
                
                # Check if channel still exists in the guild
                guild = self.bot.get_guild(guild_id)
                if guild:
                    channel = guild.get_channel(channel_id)
                    if not channel:
                        log.warning(f"[HONEYPOT] Channel {channel_id} in guild {guild_id} no longer exists, removing from database")
                        cursor.execute('DELETE FROM honeypot_channels WHERE channel_id = ?', (channel_id,))
                        continue
                else:
                    # Guild no longer exists, already handled above
                    cursor.execute('DELETE FROM honeypot_channels WHERE channel_id = ?', (channel_id,))
                    continue
            
            conn.commit()
            
        except Exception as e:
            log.error(f"Error cleaning up stale data: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    def save_honeypot_data(self):
        """Save honeypot channel data to SQLite database"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            # Save guilds
            for guild_id, guild_data in self.honeypot_channels.items():
                cursor.execute('''
                    INSERT OR REPLACE INTO guilds 
                    (guild_id, ban_count, admin_role_id, notification_channel_id, notification_dm, admin_user_ids)
                    VALUES (?, ?, ?, ?, ?, ?)
                ''', (
                    guild_id,
                    guild_data.get('ban_count', 0),
                    guild_data.get('admin_role_id'),
                    guild_data.get('notification_channel_id'),
                    1 if guild_data.get('notification_dm', False) else 0,
                    json.dumps(guild_data.get('admin_user_ids', []))
                ))
            
            # Save honeypot channels (delete old ones first, then insert new)
            for guild_id, guild_data in self.honeypot_channels.items():
                cursor.execute('DELETE FROM honeypot_channels WHERE guild_id = ?', (guild_id,))
                
                for channel in guild_data.get('channels', []):
                    cursor.execute('''
                        INSERT INTO honeypot_channels 
                        (guild_id, channel_id, enabled, auto_ban, custom_message, 
                         punishment_type, punishment_duration, whitelisted_users, 
                         whitelisted_roles, min_account_age_days, check_patterns)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ''', (
                        guild_id,
                        channel['channel_id'],
                        1 if channel.get('enabled', True) else 0,
                        1 if channel.get('auto_ban', False) else 0,
                        channel.get('custom_message'),
                        channel.get('punishment_type', 'ban'),
                        channel.get('punishment_duration', 0),
                        json.dumps(channel.get('whitelisted_users', [])),
                        json.dumps(channel.get('whitelisted_roles', [])),
                        channel.get('min_account_age_days', 0),
                        1 if channel.get('check_patterns', True) else 0
                    ))
            
            conn.commit()
        except Exception as e:
            log.error(f"Error saving honeypot data: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    def add_audit_log(self, guild_id, admin_id, action, details):
        """Add an entry to the audit log in SQLite"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('''
                INSERT INTO audit_log (guild_id, admin_id, action, details, timestamp)
                VALUES (?, ?, ?, ?, ?)
            ''', (guild_id, admin_id, action, json.dumps(details), datetime.now(timezone.utc).isoformat()))
            
            # Keep only last 100 entries per guild
            cursor.execute('''
                DELETE FROM audit_log 
                WHERE id NOT IN (
                    SELECT id FROM audit_log 
                    WHERE guild_id = ? 
                    ORDER BY timestamp DESC 
                    LIMIT 100
                ) AND guild_id = ?
            ''', (guild_id, guild_id))
            
            conn.commit()
        except Exception as e:
            log.error(f"Error adding audit log: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    def check_suspicious_patterns(self, content):
        """Check message content for suspicious patterns"""
        patterns = {
            'urls': r'https?://[^\s]+',
            'mentions': r'<@!?[0-9]+>',
            'invite_links': r'(discord\.gg/[^\s]+|discord\.com/invite/[^\s]+)',
            'spam_caps': r'[A-Z]{5,}',
            'repeated_chars': r'(.)\1{4,}'
        }
        
        detected = {}
        for pattern_name, pattern in patterns.items():
            if re.search(pattern, content, re.IGNORECASE):
                detected[pattern_name] = True
        
        return detected
    
    def is_whitelisted(self, member, channel_config):
        """Check if user is whitelisted"""
        # Check user whitelist
        if member.id in channel_config.get('whitelisted_users', []):
            return True
        
        # Check role whitelist
        for role in member.roles:
            if role.id in channel_config.get('whitelisted_roles', []):
                return True
        
        return False
    
    def passes_account_age_check(self, member, channel_config):
        """Check if account passes minimum age requirement"""
        min_days = channel_config.get('min_account_age_days', 0)
        if min_days == 0:
            return True
        
        account_age = (datetime.now(timezone.utc) - member.created_at).days
        return account_age >= min_days
    
    async def notify_admins(self, guild, message, offender):
        """Send notifications to admins about honeypot trigger"""
        guild_id = guild.id
        guild_data = self.honeypot_channels.get(guild_id, {})
        
        # DM notification
        if guild_data.get('notification_dm', False):
            for admin_id in guild_data.get('admin_user_ids', []):
                try:
                    admin = await self.bot.fetch_user(admin_id)
                    await admin.send(f"🚨 **Honeypot Triggered in {guild.name}**\nUser: {offender} ({offender.id})\nMessage: {message.content[:100]}")
                except Exception as e:
                    log.error(f"Failed to DM admin {admin_id}: {e}")
        
        # Channel notification
        notification_channel_id = guild_data.get('notification_channel_id')
        if notification_channel_id:
            channel = self.bot.get_channel(notification_channel_id)
            if channel:
                try:
                    embed = discord.Embed(
                        title="🚨 Honeypot Triggered",
                        color=discord.Color.red()
                    )
                    embed.add_field(name="User", value=f"{offender} ({offender.id})", inline=False)
                    embed.add_field(name="Channel", value=message.channel.mention, inline=False)
                    embed.add_field(name="Message", value=message.content[:200], inline=False)
                    await channel.send(embed=embed)
                except Exception as e:
                    log.error(f"Failed to send notification to channel: {e}")
    
    def add_to_offender_list(self, user_id, username, guild_id):
        """Add user to cross-server offender list in SQLite"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('SELECT username, first_offense, offense_count, servers FROM offenders WHERE user_id = ?', (user_id,))
            result = cursor.fetchone()
            
            if result:
                # Update existing offender
                offense_count = result[2] + 1
                servers = json.loads(result[3]) if result[3] else []
                if guild_id not in servers:
                    servers.append(guild_id)
                
                cursor.execute('''
                    UPDATE offenders 
                    SET username = ?, last_offense = ?, offense_count = ?, servers = ?
                    WHERE user_id = ?
                ''', (username, datetime.now(timezone.utc).isoformat(), offense_count, json.dumps(servers), user_id))
            else:
                # Insert new offender
                cursor.execute('''
                    INSERT INTO offenders (user_id, username, first_offense, last_offense, offense_count, servers)
                    VALUES (?, ?, ?, ?, ?, ?)
                ''', (user_id, username, datetime.now(timezone.utc).isoformat(), datetime.now(timezone.utc).isoformat(), 1, json.dumps([guild_id])))
            
            conn.commit()
        except Exception as e:
            log.error(f"Error adding to offender list: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    def check_offender_list(self, user_id):
        """Check if user is in the offender list"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('SELECT username, first_offense, last_offense, offense_count, servers FROM offenders WHERE user_id = ?', (user_id,))
            result = cursor.fetchone()
            
            if result:
                return {
                    'username': result[0],
                    'first_offense': result[1],
                    'last_offense': result[2],
                    'offense_count': result[3],
                    'servers': json.loads(result[4]) if result[4] else []
                }
            return None
        except Exception as e:
            log.error(f"Error checking offender list: {e}")
            return None
        finally:
            conn.close()
    
    async def check_known_malicious(self, user_id):
        """Check if user is in known malicious databases or has been banned before"""
        known_bad = False
        sources = []
        
        # Check local activity logs for previous bans using SQLite
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('''
                SELECT DISTINCT guild_id
                FROM activity_logs
                WHERE user_id = ? AND action = 'user_banned'
            ''', (user_id,))
            
            guilds = cursor.fetchall()
            
            for guild_id in guilds:
                known_bad = True
                sources.append(f'Previously banned in guild {guild_id[0]}')
                
        except Exception as e:
            log.warning(f"Error checking known malicious: {e}")
        finally:
            conn.close()
        
        return known_bad, sources
    
    @tasks.loop(hours=168)  # Run every week (168 hours)
    async def weekly_summary_task(self):
        """Send weekly summary to admins"""
        await self.bot.wait_until_ready()
        
        for guild_id, guild_data in self.honeypot_channels.items():
            try:
                guild = self.bot.get_guild(guild_id)
                if not guild:
                    continue
                
                # Get activity logs for the week from SQLite
                conn = sqlite3.connect(self.db_file)
                cursor = conn.cursor()
                
                cursor.execute('''
                    SELECT id, guild_id, user_id, username, action, details, timestamp
                    FROM activity_logs
                    WHERE guild_id = ?
                    ORDER BY timestamp DESC
                ''', (guild_id,))
                
                logs = []
                for row in cursor.fetchall():
                    logs.append({
                        'id': row[0],
                        'guild_id': row[1],
                        'user_id': row[2],
                        'username': row[3],
                        'action': row[4],
                        'details': json.loads(row[5]) if row[5] else {},
                        'timestamp': row[6]
                    })
                
                conn.close()
                
                # Filter logs from the last week
                one_week_ago = datetime.now(timezone.utc) - timedelta(days=7)
                recent_logs = [
                    log_entry for log_entry in logs
                    if datetime.fromisoformat(log_entry['timestamp']).replace(tzinfo=timezone.utc) > one_week_ago
                ]
                
                if not recent_logs:
                    continue
                
                # Calculate stats
                total_triggers = len(recent_logs)
                bans = len([l for l in recent_logs if l['action'] == 'user_banned'])
                kicks = len([l for l in recent_logs if l['action'] == 'user_kicked'])
                timeouts = len([l for l in recent_logs if l['action'] == 'user_timed_out'])
                
                # Create summary embed
                embed = discord.Embed(
                    title="📊 Weekly Honeypot Summary",
                    color=discord.Color.blue()
                )
                embed.add_field(name="Total Triggers", value=total_triggers, inline=True)
                embed.add_field(name="Bans", value=bans, inline=True)
                embed.add_field(name="Kicks", value=kicks, inline=True)
                embed.add_field(name="Timeouts", value=timeouts, inline=True)
                embed.add_field(name="Total Bans (All Time)", value=guild_data.get('ban_count', 0), inline=True)
                embed.add_field(name="Active Honeypots", value=len(guild_data.get('channels', [])), inline=True)
                
                # Send to notification channel
                notification_channel_id = guild_data.get('notification_channel_id')
                if notification_channel_id:
                    channel = self.bot.get_channel(notification_channel_id)
                    if channel:
                        await channel.send(embed=embed)
                
                # DM to admins if enabled
                if guild_data.get('notification_dm', False):
                    for admin_id in guild_data.get('admin_user_ids', []):
                        try:
                            admin = await self.bot.fetch_user(admin_id)
                            await admin.send(f"📊 Weekly Honeypot Summary for {guild.name}", embed=embed)
                        except:
                            pass
                
            except Exception as e:
                log.error(f"Error sending weekly summary for guild {guild_id}: {e}")
    
    @weekly_summary_task.before_loop
    async def before_weekly_summary(self):
        await self.bot.wait_until_ready()
    
    def _format_audit_details(self, action, details):
        """Format audit log details based on action type"""
        if not details:
            return "No details"
        
        # Helper function to get channel name from ID
        def get_channel_name(channel_id):
            channel = self.bot.get_channel(channel_id) if channel_id else None
            return channel.name if channel else f"ID: {channel_id}"
        
        # Helper function to get user display name from ID
        def get_user_name(user_id):
            user = self.bot.get_user(user_id) if user_id else None
            if user:
                return user.global_name if user.global_name else user.display_name
            return f"ID: {user_id}"
        
        if action == 'set_honeypot':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Channel: #{channel_name}\nAuto-ban: {details.get('auto_ban', False)}"
        elif action == 'remove_honeypot':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Channel: #{channel_name}"
        elif action == 'apply_template':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Template: {details.get('template')}\nChannel: #{channel_name}"
        elif action == 'set_punishment':
            channel_name = get_channel_name(details.get('channel_id'))
            duration = details.get('duration', 0)
            duration_text = f"{duration} min" if duration > 0 else "N/A"
            return f"Channel: #{channel_name}\nPunishment: {details.get('punishment')}\nDuration: {duration_text}"
        elif action == 'whitelist_user':
            user_name = get_user_name(details.get('user_id'))
            channel_name = get_channel_name(details.get('channel_id'))
            return f"User: {user_name}\nChannel: #{channel_name}"
        elif action == 'unwhitelist_user':
            user_name = get_user_name(details.get('user_id'))
            channel_name = get_channel_name(details.get('channel_id'))
            return f"User: {user_name}\nChannel: #{channel_name}"
        elif action == 'set_account_age':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Channel: #{channel_name}\nMin Age: {details.get('days', 0)} days"
        elif action == 'set_notification_channel':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Channel: #{channel_name}"
        elif action == 'toggle_dm_notifications':
            return f"New Status: {'Enabled' if details.get('new_status') else 'Disabled'}"
        elif action == 'add_admin':
            user_name = get_user_name(details.get('user_id'))
            return f"User: {user_name}"
        elif action == 'remove_admin':
            user_name = get_user_name(details.get('user_id'))
            return f"User: {user_name}"
        elif action == 'set_message':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Channel: #{channel_name}"
        elif action == 'toggle_auto_ban':
            channel_name = get_channel_name(details.get('channel_id'))
            return f"Channel: #{channel_name}\nNew Status: {'Enabled' if details.get('new_status') else 'Disabled'}"
        elif action == 'clear_logs':
            return "Logs cleared"
        elif action == 'reset_ban_count':
            return "Ban counter reset"
        elif action == 'export_logs':
            return f"Logs exported ({details.get('log_count', 0)} entries)"
        else:
            return str(details)[:200]
    
    async def template_autocomplete(self, interaction: discord.Interaction, current: str):
        """Autocomplete for template names"""
        templates = ['strict', 'lenient', 'stealth', 'moderate', 'newbie', 'investigation']
        
        filtered = [t for t in templates if current.lower() in t.lower()]
        
        return [
            app_commands.Choice(name=t.title(), value=t)
            for t in filtered
        ]
    
    async def punishment_autocomplete(self, interaction: discord.Interaction, current: str):
        """Autocomplete for punishment types"""
        punishments = ['ban', 'kick', 'timeout']
        
        filtered = [p for p in punishments if current.lower() in p.lower()]
        
        return [
            app_commands.Choice(name=p.title(), value=p)
            for p in filtered
        ]
    
    def log_activity(self, guild_id, user_id, username, action, details):
        """Log suspicious activity to SQLite database"""
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('''
                INSERT INTO activity_logs 
                (guild_id, user_id, username, action, details, timestamp)
                VALUES (?, ?, ?, ?, ?, ?)
            ''', (
                guild_id,
                user_id,
                username,
                action,
                json.dumps(details),
                datetime.now(timezone.utc).isoformat()
            ))
            conn.commit()
        except Exception as e:
            log.error(f"Error logging activity: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    @commands.Cog.listener()
    async def on_message(self, message):
        """Monitor honeypot channels for messages"""
        if message.author.bot or not message.guild:
            return
        
        # Check if cog is enabled for this guild
        if not self.is_enabled(message.guild.id):
            return
        
        guild_id = message.guild.id
        if guild_id not in self.honeypot_channels:
            return
        
        guild_data = self.honeypot_channels[guild_id]
        
        # Check all honeypot channels for this guild
        for channel_config in guild_data.get('channels', []):
            if not channel_config.get('enabled', True):
                continue
            
            if message.channel.id == channel_config['channel_id']:
                # Check whitelist
                if self.is_whitelisted(message.author, channel_config):
                    log.warning(f"[HONEYPOT] {message.author} is whitelisted, skipping")
                    return
                
                # Check account age
                if not self.passes_account_age_check(message.author, channel_config):
                    log.warning(f"[HONEYPOT] {message.author} account too young, taking action")
                    # Still proceed with punishment
                
                # Check for suspicious patterns if enabled
                patterns_detected = {}
                if channel_config.get('check_patterns', True):
                    patterns_detected = self.check_suspicious_patterns(message.content)
                
                # Log the activity
                self.log_activity(
                    guild_id=guild_id,
                    user_id=message.author.id,
                    username=str(message.author),
                    action='message_in_honeypot',
                    details={
                        'content': message.content,
                        'channel_id': message.channel.id,
                        'patterns_detected': patterns_detected
                    }
                )
                
                # Delete the message
                try:
                    await message.delete()
                except discord.Forbidden:
                    log.warning(f"[HONEYPOT] Failed to delete message - missing permissions")
                except Exception as e:
                    log.error(f"[HONEYPOT] Error deleting message: {e}")
                
                # Add to offender list
                self.add_to_offender_list(message.author.id, str(message.author), guild_id)
                
                # Check known malicious databases
                is_malicious, sources = await self.check_known_malicious(message.author.id)
                if is_malicious:
                    log.error(f"[HONEYPOT] {message.author} found in malicious databases: {', '.join(sources)}")
                    # Log this information
                    self.log_activity(
                        guild_id=guild_id,
                        user_id=message.author.id,
                        username=str(message.author),
                        action='known_malicious',
                        details={'sources': sources}
                    )
                
                # Notify admins
                await self.notify_admins(message.guild, message, message.author)
                
                # Apply punishment if auto_ban is enabled
                if channel_config.get('auto_ban', False):
                    punishment_type = channel_config.get('punishment_type', 'ban')
                    punishment_duration = channel_config.get('punishment_duration', 0)
                    
                    # Check if user is admin
                    if message.author.guild_permissions.administrator:
                        log.warning(f"[HONEYPOT] Skipping punishment for {message.author} - user is administrator")
                        return
                    
                    try:
                        if punishment_type == 'ban':
                            if message.guild.me.guild_permissions.ban_members:
                                await message.author.ban(reason="Sent message in honeypot channel", delete_message_days=7)
                                self.honeypot_channels[guild_id]['ban_count'] = self.honeypot_channels[guild_id].get('ban_count', 0) + 1
                                self.save_honeypot_data()
                                self.log_activity(
                                    guild_id=guild_id,
                                    user_id=message.author.id,
                                    username=str(message.author),
                                    action='user_banned',
                                    details={
                                        'reason': 'Sent message in honeypot channel',
                                        'content': message.content
                                    }
                                )
                                log.info(f"[HONEYPOT] Banned {message.author} for sending message in honeypot channel")
                            else:
                                log.warning(f"[HONEYPOT] Bot lacks ban permissions")
                        
                        elif punishment_type == 'kick':
                            if message.guild.me.guild_permissions.kick_members:
                                await message.author.kick(reason="Sent message in honeypot channel")
                                self.log_activity(
                                    guild_id=guild_id,
                                    user_id=message.author.id,
                                    username=str(message.author),
                                    action='user_kicked',
                                    details={
                                        'reason': 'Sent message in honeypot channel',
                                        'content': message.content
                                    }
                                )
                                log.info(f"[HONEYPOT] Kicked {message.author} for sending message in honeypot channel")
                            else:
                                log.warning(f"[HONEYPOT] Bot lacks kick permissions")
                        
                        elif punishment_type == 'timeout':
                            if message.guild.me.guild_permissions.moderate_members:
                                duration = timedelta(minutes=punishment_duration) if punishment_duration > 0 else timedelta(minutes=60)
                                await message.author.timeout(duration, reason="Sent message in honeypot channel")
                                self.log_activity(
                                    guild_id=guild_id,
                                    user_id=message.author.id,
                                    username=str(message.author),
                                    action='user_timed_out',
                                    details={
                                        'reason': 'Sent message in honeypot channel',
                                        'content': message.content,
                                        'duration_minutes': punishment_duration if punishment_duration > 0 else 60
                                    }
                                )
                                log.info(f"[HONEYPOT] Timed out {message.author} for sending message in honeypot channel")
                            else:
                                log.warning(f"[HONEYPOT] Bot lacks timeout permissions")
                        
                        elif punishment_type == 'mute':
                            # This would require a mute role setup - simplified version here
                            log.warning(f"[HONEYPOT] Mute punishment not yet implemented")
                    
                    except discord.Forbidden:
                        log.warning(f"[HONEYPOT] Failed to apply punishment - insufficient permissions")
                    except Exception as e:
                        log.error(f"[HONEYPOT] Error applying punishment: {e}")
                
                log.debug(f"[HONEYPOT] {message.author} sent message in honeypot channel: {message.content[:50]}...")
                return
    
    async def create_honeypot_embed(self, custom_message=None):
        """Create the honeypot warning embed"""
        embed = discord.Embed(
            description="",
            color=0x2b2d31  # Dark grey color like in the image
        )
        
        message = custom_message if custom_message else "**DO NOT SEND MESSAGES IN THIS CHANNEL**\n\nThis channel is used to catch spam bots. Any messages sent here will result in a softban."
        
        embed.add_field(
            name="",
            value=message,
            inline=False
        )
        
        return embed
    
    @commands.Cog.listener()
    async def on_guild_join(self, guild):
        """Initialize honeypot data for new guild"""
        if guild.id not in self.honeypot_channels:
            self.honeypot_channels[guild.id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
            self.save_honeypot_data()
    
    @commands.hybrid_command(name='sethoneypot', description='Set a channel as a honeypot for this server')
    @commands.has_permissions(administrator=True)
    async def sethoneypot(self, ctx, channel: discord.TextChannel = None, auto_ban: bool = False, message: str = None):
        """Set a channel as a honeypot"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        # Initialize guild data if not exists
        if guild_id not in self.honeypot_channels:
            self.honeypot_channels[guild_id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
        
        # Convert \n to actual newlines if message is provided
        if message:
            message = message.replace('\\n', '\n')
        
        # Check if channel is already a honeypot
        for existing_channel in self.honeypot_channels[guild_id]['channels']:
            if existing_channel['channel_id'] == channel.id:
                if ctx.interaction:
                    await ctx.send(f"❌ {channel.mention} is already a honeypot channel.", ephemeral=True)
                else:
                    await ctx.send(f"❌ {channel.mention} is already a honeypot channel.")
                return
        
        # Add new honeypot channel
        new_channel_config = {
            'channel_id': channel.id,
            'enabled': True,
            'auto_ban': auto_ban,
            'custom_message': message,
            'punishment_type': 'ban',
            'punishment_duration': 0,
            'whitelisted_users': [],
            'whitelisted_roles': [],
            'min_account_age_days': 0,
            'check_patterns': True
        }
        
        self.honeypot_channels[guild_id]['channels'].append(new_channel_config)
        self.save_honeypot_data()
        
        # Add to audit log
        self.add_audit_log(guild_id, ctx.author.id, 'set_honeypot', {
            'channel_id': channel.id,
            'auto_ban': auto_ban
        })
        
        # Create and send the honeypot embed
        embed = await self.create_honeypot_embed(message)
        await channel.send(embed=embed)
        
        ban_status = "with auto-ban enabled" if auto_ban else "without auto-ban"
        if ctx.interaction:
            await ctx.send(f"✅ Honeypot channel set to {channel.mention} {ban_status}. All messages in this channel will be logged.", ephemeral=True)
        else:
            await ctx.send(f"✅ Honeypot channel set to {channel.mention} {ban_status}. All messages in this channel will be logged.")
    
    @commands.hybrid_command(name='removehoneypot', description='Remove a honeypot channel for this server')
    @commands.has_permissions(administrator=True)
    async def removehoneypot(self, ctx, channel: discord.TextChannel = None):
        """Remove a honeypot channel"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot channels are set for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot channels are set for this server.")
            return
        
        # Find and remove the channel
        original_length = len(self.honeypot_channels[guild_id]['channels'])
        self.honeypot_channels[guild_id]['channels'] = [
            c for c in self.honeypot_channels[guild_id]['channels'] 
            if c['channel_id'] != channel.id
        ]
        
        if len(self.honeypot_channels[guild_id]['channels']) < original_length:
            self.save_honeypot_data()
            self.add_audit_log(guild_id, ctx.author.id, 'remove_honeypot', {
                'channel_id': channel.id
            })
            if ctx.interaction:
                await ctx.send(f"✅ Honeypot channel {channel.mention} removed.", ephemeral=True)
            else:
                await ctx.send(f"✅ Honeypot channel {channel.mention} removed.")
        else:
            if ctx.interaction:
                await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
            else:
                await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='honeypotstatus', description='Check the honeypot status for this server')
    async def honeypotstatus(self, ctx):
        """Check honeypot status"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        # Reload to ensure we have latest data
        self.load_honeypot_data()
        
        if guild_id not in self.honeypot_channels or not self.honeypot_channels[guild_id].get('channels'):
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        channels = self.honeypot_channels[guild_id]['channels']
        embed = discord.Embed(
            title="🍯 Honeypot Status",
            color=discord.Color.orange()
        )
        
        for i, channel_config in enumerate(channels, 1):
            channel = self.bot.get_channel(channel_config['channel_id'])
            status = "✅ Enabled" if channel_config.get('enabled', True) else "❌ Disabled"
            auto_ban = "✅" if channel_config.get('auto_ban', False) else "❌"
            punishment = channel_config.get('punishment_type', 'ban').title()
            duration = channel_config.get('punishment_duration', 0)
            
            if channel:
                embed.add_field(
                    name=f"Channel {i}: #{channel.name}",
                    value=f"Status: {status}\nAuto-ban: {auto_ban}\nPunishment: {punishment}{f' ({duration} min)' if duration > 0 and punishment == 'Timeout' else ''}",
                    inline=False
                )
            else:
                embed.add_field(
                    name=f"Channel {i}: Unknown (ID: {channel_config['channel_id']})",
                    value=f"Status: {status}\nAuto-ban: {auto_ban}\nPunishment: {punishment}{f' ({duration} min)' if duration > 0 and punishment == 'Timeout' else ''}",
                    inline=False
                )
        
        embed.add_field(
            name="Total Bans",
            value=self.honeypot_channels[guild_id].get('ban_count', 0),
            inline=False
        )
        
        if ctx.interaction:
            await ctx.send(embed=embed, ephemeral=True)
        else:
            await ctx.send(embed=embed)
    
    @commands.hybrid_command(name='honeypotlogs', description='View recent honeypot activity logs')
    @commands.has_permissions(administrator=True)
    async def honeypotlogs(self, ctx, limit: int = 10):
        """View recent honeypot logs"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('''
                SELECT id, user_id, username, action, details, timestamp
                FROM activity_logs
                WHERE guild_id = ?
                ORDER BY timestamp DESC
                LIMIT ?
            ''', (guild_id, limit))
            
            logs = []
            for row in cursor.fetchall():
                logs.append({
                    'id': row[0],
                    'user_id': row[1],
                    'username': row[2],
                    'action': row[3],
                    'details': json.loads(row[4]) if row[4] else {},
                    'timestamp': row[5]
                })
            
            conn.close()
            
            recent_logs = logs[-limit:] if len(logs) > limit else logs
            
            if not recent_logs:
                if ctx.interaction:
                    await ctx.send("No recent activity to show.", ephemeral=True)
                else:
                    await ctx.send("No recent activity to show.")
                return
            
            embed = discord.Embed(
                title=f"Recent Honeypot Activity (Last {len(recent_logs)})",
                color=discord.Color.orange()
            )
            
            for i, log_entry in enumerate(recent_logs):
                timestamp = log_entry.get('timestamp', 'Unknown')
                user_id = log_entry.get('user_id')
                username = log_entry.get('username', 'Unknown')
                action = log_entry.get('action', 'Unknown')
                details = log_entry.get('details', {})
                
                # Format timestamp to be more readable
                try:
                    dt = datetime.fromisoformat(timestamp)
                    if dt.tzinfo is None:
                        dt = dt.replace(tzinfo=timezone.utc)
                    formatted_time = dt.strftime('%Y-%m-%d %H:%M:%S')
                except:
                    formatted_time = timestamp
                
                # Try to get current display name from user ID
                display_name = username
                if user_id and ctx.guild:
                    try:
                        member = await ctx.guild.fetch_member(user_id)
                        display_name = member.display_name
                    except:
                        pass  # User may have left the server
                
                # Format details nicely
                details_text = ""
                if isinstance(details, dict):
                    for key, value in details.items():
                        if key == 'content':
                            details_text += f"**Content:** {value}\n"
                        elif key == 'channel_id':
                            # Try to get channel name from ID
                            channel = self.bot.get_channel(value)
                            channel_name = channel.name if channel else f"Unknown ({value})"
                            details_text += f"**Channel:** #{channel_name}\n"
                        elif key == 'channel_name':
                            details_text += f"**Channel:** #{value}\n"
                        elif key == 'reason':
                            details_text += f"**Reason:** {value}\n"
                        else:
                            details_text += f"**{key.title()}:** {value}\n"
                else:
                    details_text = str(details)
                
                # Format action nicely
                action_display = action.replace('_', ' ').title()
                
                embed.add_field(
                    name=f"{display_name} - {formatted_time}",
                    value=f"**Action:** {action_display}\n{details_text}",
                    inline=False
                )
                
                # Add empty field for spacing if there are more logs to show
                if i < len(recent_logs) - 1:
                    embed.add_field(name="\u200b", value="\u200b", inline=False)
            
            if ctx.interaction:
                await ctx.send(embed=embed, ephemeral=True)
            else:
                await ctx.send(embed=embed)
            
        except Exception as e:
            if ctx.interaction:
                await ctx.send(f"Error reading logs: {e}", ephemeral=True)
            else:
                await ctx.send(f"Error reading logs: {e}")
    
    @commands.hybrid_command(name='clearhoneypotlogs', description='Clear honeypot activity logs for this server')
    @commands.has_permissions(administrator=True)
    async def clearhoneypotlogs(self, ctx):
        """Clear honeypot logs"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('DELETE FROM activity_logs WHERE guild_id = ?', (guild_id,))
            conn.commit()
            self.add_audit_log(guild_id, ctx.author.id, 'clear_logs', {})
            
            if ctx.interaction:
                await ctx.send("✅ Honeypot logs cleared for this server.", ephemeral=True)
            else:
                await ctx.send("✅ Honeypot logs cleared for this server.")
        except Exception as e:
            if ctx.interaction:
                await ctx.send(f"Error clearing logs: {e}", ephemeral=True)
            else:
                await ctx.send(f"Error clearing logs: {e}")
            conn.rollback()
        finally:
            conn.close()
    
    @commands.hybrid_command(name='exportlogs', description='Export honeypot logs to CSV')
    @commands.has_permissions(administrator=True)
    async def exportlogs(self, ctx):
        """Export honeypot logs to CSV"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('''
                SELECT timestamp, user_id, username, action, details
                FROM activity_logs
                WHERE guild_id = ?
                ORDER BY timestamp DESC
            ''', (guild_id,))
            
            logs = []
            for row in cursor.fetchall():
                logs.append({
                    'timestamp': row[0],
                    'user_id': row[1],
                    'username': row[2],
                    'action': row[3],
                    'details': row[4]
                })
            
            conn.close()
            
            csv_filename = f'honeypot_export_{guild_id}.csv'
            with open(csv_filename, 'w', newline='') as csvfile:
                fieldnames = ['timestamp', 'user_id', 'username', 'action', 'details']
                writer = csv.DictWriter(csvfile, fieldnames=fieldnames)
                writer.writeheader()
                
                for log_entry in logs:
                    writer.writerow({
                        'timestamp': log_entry.get('timestamp', ''),
                        'user_id': log_entry.get('user_id', ''),
                        'username': log_entry.get('username', ''),
                        'action': log_entry.get('action', ''),
                        'details': str(log_entry.get('details', ''))
                    })
            
            self.add_audit_log(guild_id, ctx.author.id, 'export_logs', {
                'log_count': len(logs)
            })
            
            file = discord.File(csv_filename)
            if ctx.interaction:
                await ctx.send(file=file, ephemeral=True)
            else:
                await ctx.send(file=file)
            
            # Clean up
            os.remove(csv_filename)
            
        except Exception as e:
            if ctx.interaction:
                await ctx.send(f"Error exporting logs: {e}", ephemeral=True)
            else:
                await ctx.send(f"Error exporting logs: {e}")
    
    @commands.hybrid_command(name='resetbancount', description='Reset the ban counter for this server')
    @commands.has_permissions(administrator=True)
    async def resetbancount(self, ctx):
        """Reset the ban counter"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        if guild_id in self.honeypot_channels:
            self.honeypot_channels[guild_id]['ban_count'] = 0
            self.save_honeypot_data()
            self.add_audit_log(guild_id, ctx.author.id, 'reset_ban_count', {})
            
            if ctx.interaction:
                await ctx.send("✅ Ban counter reset to 0.", ephemeral=True)
            else:
                await ctx.send("✅ Ban counter reset to 0.")
        else:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
    
    @commands.hybrid_command(name='toggleautoban', description='Toggle auto-ban for a honeypot channel')
    @commands.has_permissions(administrator=True)
    async def toggleautoban(self, ctx, channel: discord.TextChannel = None):
        """Toggle auto-ban on/off"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        # Find the channel
        for channel_config in self.honeypot_channels[guild_id]['channels']:
            if channel_config['channel_id'] == channel.id:
                current_status = channel_config.get('auto_ban', False)
                new_status = not current_status
                channel_config['auto_ban'] = new_status
                self.save_honeypot_data()
                self.add_audit_log(guild_id, ctx.author.id, 'toggle_auto_ban', {
                    'channel_id': channel.id,
                    'new_status': new_status
                })
                
                status_text = "enabled" if new_status else "disabled"
                if ctx.interaction:
                    await ctx.send(f"✅ Auto-ban has been {status_text} for {channel.mention}.", ephemeral=True)
                else:
                    await ctx.send(f"✅ Auto-ban has been {status_text} for {channel.mention}.")
                return
        
        if ctx.interaction:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
        else:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='bancount', description='Check the current ban count for this server')
    @commands.has_permissions(administrator=True)
    async def bancount(self, ctx):
        """Check the ban count"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        if guild_id in self.honeypot_channels:
            ban_count = self.honeypot_channels[guild_id].get('ban_count', 0)
            auto_ban_status = "enabled" if self.honeypot_channels[guild_id].get('auto_ban', False) else "disabled"
            if ctx.interaction:
                await ctx.send(f"🍯 Total bans: {ban_count}\nAuto-ban: {auto_ban_status}", ephemeral=True)
            else:
                await ctx.send(f"🍯 Total bans: {ban_count}\nAuto-ban: {auto_ban_status}")
        else:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
    
    @commands.hybrid_command(name='setmessage', description='Set a custom message for a honeypot channel')
    @commands.has_permissions(administrator=True)
    async def setmessage(self, ctx, channel: discord.TextChannel = None, message: str = None):
        """Set a custom honeypot message"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        if message is None:
            if ctx.interaction:
                await ctx.send("❌ Please provide a message.", ephemeral=True)
            else:
                await ctx.send("❌ Please provide a message.")
            return
        
        # Convert \n to actual newlines
        message = message.replace('\\n', '\n')
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        # Find and update the channel
        for channel_config in self.honeypot_channels[guild_id]['channels']:
            if channel_config['channel_id'] == channel.id:
                channel_config['custom_message'] = message
                self.save_honeypot_data()
                self.add_audit_log(guild_id, ctx.author.id, 'set_message', {
                    'channel_id': channel.id
                })
                
                # Update the embed in the channel
                if channel:
                    try:
                        # Find and update the bot's message
                        async for msg in channel.history(limit=10):
                            if msg.author == self.bot.user:
                                new_embed = await self.create_honeypot_embed(message)
                                await msg.edit(embed=new_embed)
                                break
                    except Exception as e:
                        log.error(f"Error updating honeypot message: {e}")
                
                if ctx.interaction:
                    await ctx.send("✅ Custom message updated.", ephemeral=True)
                else:
                    await ctx.send("✅ Custom message updated.")
                return
        
        if ctx.interaction:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
        else:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='setpunishment', description='Set punishment type for a honeypot channel')
    @commands.has_permissions(administrator=True)
    @app_commands.autocomplete(punishment=punishment_autocomplete)
    async def setpunishment(self, ctx, channel: discord.TextChannel = None, punishment: str = None, duration: int = 0):
        """Set punishment type (ban/kick/timeout) and duration (for timeout in minutes)"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        if punishment is None:
            if ctx.interaction:
                await ctx.send("❌ Please specify a punishment type: ban, kick, or timeout", ephemeral=True)
            else:
                await ctx.send("❌ Please specify a punishment type: ban, kick, or timeout")
            return
        
        punishment = punishment.lower()
        if punishment not in ['ban', 'kick', 'timeout']:
            if ctx.interaction:
                await ctx.send("❌ Invalid punishment type. Use: ban, kick, or timeout", ephemeral=True)
            else:
                await ctx.send("❌ Invalid punishment type. Use: ban, kick, or timeout")
            return
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        # Find and update the channel
        for channel_config in self.honeypot_channels[guild_id]['channels']:
            if channel_config['channel_id'] == channel.id:
                channel_config['punishment_type'] = punishment
                channel_config['punishment_duration'] = duration
                self.save_honeypot_data()
                self.add_audit_log(guild_id, ctx.author.id, 'set_punishment', {
                    'channel_id': channel.id,
                    'punishment': punishment,
                    'duration': duration
                })
                
                duration_text = f" for {duration} minutes" if punishment == 'timeout' and duration > 0 else ""
                if ctx.interaction:
                    await ctx.send(f"✅ Punishment set to {punishment}{duration_text} for {channel.mention}.", ephemeral=True)
                else:
                    await ctx.send(f"✅ Punishment set to {punishment}{duration_text} for {channel.mention}.")
                return
        
        if ctx.interaction:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
        else:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='whitelistuser', description='Whitelist a user from honeypot')
    @commands.has_permissions(administrator=True)
    async def whitelistuser(self, ctx, user: discord.Member, channel: discord.TextChannel = None):
        """Whitelist a user from honeypot detection"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        # Find and update the channel
        for channel_config in self.honeypot_channels[guild_id]['channels']:
            if channel_config['channel_id'] == channel.id:
                if user.id not in channel_config.get('whitelisted_users', []):
                    channel_config.setdefault('whitelisted_users', []).append(user.id)
                    self.save_honeypot_data()
                    self.add_audit_log(guild_id, ctx.author.id, 'whitelist_user', {
                        'user_id': user.id,
                        'channel_id': channel.id
                    })
                    if ctx.interaction:
                        await ctx.send(f"✅ {user.mention} has been whitelisted from {channel.mention}.", ephemeral=True)
                    else:
                        await ctx.send(f"✅ {user.mention} has been whitelisted from {channel.mention}.")
                else:
                    if ctx.interaction:
                        await ctx.send(f"❌ {user.mention} is already whitelisted.", ephemeral=True)
                    else:
                        await ctx.send(f"❌ {user.mention} is already whitelisted.")
                return
        
        if ctx.interaction:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
        else:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='unwhitelistuser', description='Remove user from honeypot whitelist')
    @commands.has_permissions(administrator=True)
    async def unwhitelistuser(self, ctx, user: discord.Member, channel: discord.TextChannel = None):
        """Remove a user from honeypot whitelist"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        # Find and update the channel
        for channel_config in self.honeypot_channels[guild_id]['channels']:
            if channel_config['channel_id'] == channel.id:
                if user.id in channel_config.get('whitelisted_users', []):
                    channel_config['whitelisted_users'].remove(user.id)
                    self.save_honeypot_data()
                    self.add_audit_log(guild_id, ctx.author.id, 'unwhitelist_user', {
                        'user_id': user.id,
                        'channel_id': channel.id
                    })
                    if ctx.interaction:
                        await ctx.send(f"✅ {user.mention} has been removed from whitelist.", ephemeral=True)
                    else:
                        await ctx.send(f"✅ {user.mention} has been removed from whitelist.")
                else:
                    if ctx.interaction:
                        await ctx.send(f"❌ {user.mention} is not whitelisted.", ephemeral=True)
                    else:
                        await ctx.send(f"❌ {user.mention} is not whitelisted.")
                return
        
        if ctx.interaction:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
        else:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='setaccountage', description='Set minimum account age (in days) for honeypot')
    @commands.has_permissions(administrator=True)
    async def setaccountage(self, ctx, channel: discord.TextChannel = None, days: int = 0):
        """Set minimum account age requirement (0 = no requirement)"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            if ctx.interaction:
                await ctx.send("❌ No honeypot is configured for this server.", ephemeral=True)
            else:
                await ctx.send("❌ No honeypot is configured for this server.")
            return
        
        # Find and update the channel
        for channel_config in self.honeypot_channels[guild_id]['channels']:
            if channel_config['channel_id'] == channel.id:
                channel_config['min_account_age_days'] = days
                self.save_honeypot_data()
                self.add_audit_log(guild_id, ctx.author.id, 'set_account_age', {
                    'channel_id': channel.id,
                    'days': days
                })
                
                if days == 0:
                    if ctx.interaction:
                        await ctx.send(f"✅ Account age requirement removed for {channel.mention}.", ephemeral=True)
                    else:
                        await ctx.send(f"✅ Account age requirement removed for {channel.mention}.")
                else:
                    if ctx.interaction:
                        await ctx.send(f"✅ Account age requirement set to {days} days for {channel.mention}.", ephemeral=True)
                    else:
                        await ctx.send(f"✅ Account age requirement set to {days} days for {channel.mention}.")
                return
        
        if ctx.interaction:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.", ephemeral=True)
        else:
            await ctx.send(f"❌ {channel.mention} is not a honeypot channel.")
    
    @commands.hybrid_command(name='setadminchannel', description='Set channel for honeypot notifications')
    @commands.has_permissions(administrator=True)
    async def setadminchannel(self, ctx, channel: discord.TextChannel):
        """Set channel to receive honeypot notifications"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            self.honeypot_channels[guild_id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
        
        self.honeypot_channels[guild_id]['notification_channel_id'] = channel.id
        self.save_honeypot_data()
        self.add_audit_log(guild_id, ctx.author.id, 'set_notification_channel', {
            'channel_id': channel.id
        })
        
        if ctx.interaction:
            await ctx.send(f"✅ Notification channel set to {channel.mention}.", ephemeral=True)
        else:
            await ctx.send(f"✅ Notification channel set to {channel.mention}.")
    
    @commands.hybrid_command(name='toggleadminnotifications', description='Toggle DM notifications for admins')
    @commands.has_permissions(administrator=True)
    async def toggleadminnotifications(self, ctx):
        """Toggle DM notifications for admins"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            self.honeypot_channels[guild_id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
        
        current = self.honeypot_channels[guild_id].get('notification_dm', False)
        self.honeypot_channels[guild_id]['notification_dm'] = not current
        self.save_honeypot_data()
        self.add_audit_log(guild_id, ctx.author.id, 'toggle_dm_notifications', {
            'new_status': not current
        })
        
        status = "enabled" if not current else "disabled"
        if ctx.interaction:
            await ctx.send(f"✅ DM notifications {status}.", ephemeral=True)
        else:
            await ctx.send(f"✅ DM notifications {status}.")
    
    @commands.hybrid_command(name='addadmin', description='Add an admin to receive notifications')
    @commands.has_permissions(administrator=True)
    async def addadmin(self, ctx, user: discord.Member):
        """Add a user to receive admin notifications"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            self.honeypot_channels[guild_id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
        
        if user.id not in self.honeypot_channels[guild_id].get('admin_user_ids', []):
            self.honeypot_channels[guild_id].setdefault('admin_user_ids', []).append(user.id)
            self.save_honeypot_data()
            self.add_audit_log(guild_id, ctx.author.id, 'add_admin', {'user_id': user.id})
            
            # Update database
            conn = sqlite3.connect(self.db_file)
            cursor = conn.cursor()
            try:
                cursor.execute('UPDATE guilds SET admin_user_ids = ? WHERE guild_id = ?', 
                             (json.dumps(self.honeypot_channels[guild_id]['admin_user_ids']), guild_id))
                conn.commit()
            except Exception as e:
                log.error(f"Error updating admin list in database: {e}")
                conn.rollback()
            finally:
                conn.close()
            
            if ctx.interaction:
                await ctx.send(f"✅ {user.mention} added to admin list.", ephemeral=True)
            else:
                await ctx.send(f"✅ {user.mention} added to admin list.")
        else:
            if ctx.interaction:
                await ctx.send(f"❌ {user.mention} is already an admin.", ephemeral=True)
            else:
                await ctx.send(f"❌ {user.mention} is already an admin.")
    
    @commands.hybrid_command(name='removeadmin', description='Remove an admin from notifications')
    @commands.has_permissions(administrator=True)
    async def removeadmin(self, ctx, user: discord.Member):
        """Remove a user from admin notifications"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        if guild_id in self.honeypot_channels and user.id in self.honeypot_channels[guild_id].get('admin_user_ids', []):
            self.honeypot_channels[guild_id]['admin_user_ids'].remove(user.id)
            self.save_honeypot_data()
            self.add_audit_log(guild_id, ctx.author.id, 'remove_admin', {'user_id': user.id})
            
            # Update database
            conn = sqlite3.connect(self.db_file)
            cursor = conn.cursor()
            try:
                cursor.execute('UPDATE guilds SET admin_user_ids = ? WHERE guild_id = ?', 
                             (json.dumps(self.honeypot_channels[guild_id]['admin_user_ids']), guild_id))
                conn.commit()
            except Exception as e:
                log.error(f"Error updating admin list in database: {e}")
                conn.rollback()
            finally:
                conn.close()
            
            if ctx.interaction:
                await ctx.send(f"✅ {user.mention} removed from admin list.", ephemeral=True)
            else:
                await ctx.send(f"✅ {user.mention} removed from admin list.")
        else:
            if ctx.interaction:
                await ctx.send(f"❌ {user.mention} is not an admin.", ephemeral=True)
            else:
                await ctx.send(f"❌ {user.mention} is not an admin.")
    
    @commands.hybrid_command(name='auditlog', description='View honeypot audit log')
    @commands.has_permissions(administrator=True)
    async def auditlog(self, ctx, limit: int = 10):
        """View recent audit log entries"""
        # Defer response if this is an interaction to avoid timeout
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        guild_id = ctx.guild.id
        
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            cursor.execute('''
                SELECT id, admin_id, action, details, timestamp
                FROM audit_log
                WHERE guild_id = ?
                ORDER BY timestamp DESC
                LIMIT ?
            ''', (guild_id, limit))
            
            audit_log = []
            for row in cursor.fetchall():
                audit_log.append({
                    'id': row[0],
                    'admin_id': row[1],
                    'action': row[2],
                    'details': json.loads(row[3]) if row[3] else {},
                    'timestamp': row[4]
                })
            
            if not audit_log:
                if ctx.interaction:
                    await ctx.send("No audit log entries.", ephemeral=True)
                else:
                    await ctx.send("No audit log entries.")
                return
            
            recent_logs = audit_log
        except Exception as e:
            if ctx.interaction:
                await ctx.send(f"Error loading audit log: {e}", ephemeral=True)
            else:
                await ctx.send(f"Error loading audit log: {e}")
            return
        finally:
            conn.close()
        
        embed = discord.Embed(
            title=f"Audit Log (Last {len(recent_logs)})",
            color=discord.Color.blue()
        )
        
        for log_entry in reversed(recent_logs):
            timestamp = log_entry.get('timestamp', 'Unknown')
            admin_id = log_entry.get('admin_id')
            action = log_entry.get('action', 'Unknown')
            details = log_entry.get('details', {})
            
            try:
                dt = datetime.fromisoformat(timestamp)
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=timezone.utc)
                formatted_time = dt.strftime('%Y-%m-%d %H:%M:%S')
            except:
                formatted_time = timestamp
            
            # Try to get current display name from user ID
            admin = self.bot.get_user(admin_id) if admin_id else None
            admin_name = admin.name if admin else f"ID: {admin_id}"
            if admin_id and ctx.guild:
                try:
                    member = await ctx.guild.fetch_member(admin_id)
                    admin_name = member.display_name
                except:
                    pass  # User may have left the server
            
            # Format details based on action type
            details_text = self._format_audit_details(action, details)
            
            embed.add_field(
                name=f"{admin_name} - {formatted_time}",
                value=f"**Action:** {action}\n{details_text}",
                inline=False
            )
        
        if ctx.interaction:
            await ctx.send(embed=embed, ephemeral=True)
        else:
            await ctx.send(embed=embed)
    
    @commands.hybrid_command(name='checkoffender', description='Check if a user is in the offender list')
    @commands.has_permissions(administrator=True)
    async def checkoffender(self, ctx, user: discord.Member):
        """Check if a user is in the cross-server offender list"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        offender_data = self.check_offender_list(user.id)
        
        if offender_data:
            embed = discord.Embed(
                title="⚠️ Offender Found",
                color=discord.Color.red()
            )
            embed.add_field(name="Username", value=offender_data.get('username', 'Unknown'), inline=False)
            embed.add_field(name="Offense Count", value=offender_data.get('offense_count', 0), inline=False)
            embed.add_field(name="First Offense", value=offender_data.get('first_offense', 'Unknown'), inline=False)
            embed.add_field(name="Last Offense", value=offender_data.get('last_offense', 'Unknown'), inline=False)
            embed.add_field(name="Servers", value=str(len(offender_data.get('servers', []))), inline=False)
            
            if ctx.interaction:
                await ctx.send(embed=embed, ephemeral=True)
            else:
                await ctx.send(embed=embed)
        else:
            if ctx.interaction:
                await ctx.send(f"✅ {user.mention} is not in the offender list.", ephemeral=True)
            else:
                await ctx.send(f"✅ {user.mention} is not in the offender list.")
        return
    
    @commands.hybrid_command(name='crossservercheck', description='Check a user\'s activity across all servers')
    @commands.has_permissions(administrator=True)
    async def crossservercheck(self, ctx, user: discord.Member):
        """Check a user's activity across all servers"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        user_id_str = str(user.id)
        cross_server_data = {}
        
        # Ensure current server is included in the check
        current_guild_id = ctx.guild.id
        if current_guild_id not in self.honeypot_channels:
            self.honeypot_channels[current_guild_id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
        
        # Check all guilds' activity logs (including current server) from SQLite
        conn = sqlite3.connect(self.db_file)
        cursor = conn.cursor()
        
        try:
            # Get all guild IDs from database
            cursor.execute('SELECT DISTINCT guild_id FROM activity_logs')
            all_guild_ids = [row[0] for row in cursor.fetchall()]
            
            for guild_id in all_guild_ids:
                cursor.execute('''
                    SELECT id, user_id, username, action, details, timestamp
                    FROM activity_logs
                    WHERE guild_id = ? AND user_id = ?
                    ORDER BY timestamp DESC
                ''', (guild_id, user_id_str))
                
                user_logs = []
                for row in cursor.fetchall():
                    user_logs.append({
                        'id': row[0],
                        'user_id': row[1],
                        'username': row[2],
                        'action': row[3],
                        'details': json.loads(row[4]) if row[4] else {},
                        'timestamp': row[5]
                    })
                
                if user_logs:
                    guild = self.bot.get_guild(guild_id)
                    guild_name = guild.name if guild else f"ID: {guild_id}"
                    
                    # Count different action types and collect details
                    actions = {}
                    channels = set()
                    
                    for log_entry in user_logs:
                        action = log_entry.get('action', 'unknown')
                        actions[action] = actions.get(action, 0) + 1
                        
                        # Collect channel information
                        details = log_entry.get('details', {})
                        if isinstance(details, dict):
                            channel_id = details.get('channel_id')
                            if channel_id:
                                channels.add(channel_id)
                    
                    cross_server_data[guild_id] = {
                        'guild_name': guild_name,
                        'total_events': len(user_logs),
                        'actions': actions,
                        'channels': list(channels),
                        'latest_event': user_logs[-1].get('timestamp', 'Unknown')
                    }
        except Exception as e:
            log.warning(f"Error checking logs: {e}")
        finally:
            conn.close()
        
        if not cross_server_data:
            if ctx.interaction:
                await ctx.send(f"✅ No activity found for {user.mention} across all servers.", ephemeral=True)
            else:
                await ctx.send(f"✅ No activity found for {user.mention} across all servers.")
            return
        
        # Create embed with cross-server data
        embed = discord.Embed(
            title=f"🌍 Cross-Server Activity for {user.display_name}",
            color=discord.Color.blue()
        )
        
        for guild_id, data in cross_server_data.items():
            action_summary = []
            for action, count in data['actions'].items():
                action_display = action.replace('_', ' ').title()
                action_summary.append(f"{action_display}: {count}")
            
            # Get channel names
            channel_names = []
            for channel_id in data.get('channels', []):
                channel = self.bot.get_channel(channel_id)
                if channel:
                    channel_names.append(f"#{channel.name}")
                else:
                    channel_names.append(f"ID: {channel_id}")
            
            try:
                dt = datetime.fromisoformat(data['latest_event'])
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=timezone.utc)
                latest_time = dt.strftime('%Y-%m-%d %H:%M:%S')
            except:
                latest_time = data['latest_event']
            
            field_value = f"Total Events: {data['total_events']}\n"
            field_value += f"Actions: {', '.join(action_summary)}\n"
            
            if channel_names:
                field_value += f"Channels: {', '.join(channel_names[:5])}"
                if len(channel_names) > 5:
                    field_value += f" (+{len(channel_names) - 5} more)"
                field_value += "\n"
            
            field_value += f"Latest: {latest_time}"
            
            embed.add_field(
                name=f"📊 {data['guild_name']}",
                value=field_value,
                inline=False
            )
        
        # Add summary
        total_events = sum(data['total_events'] for data in cross_server_data.values())
        total_servers = len(cross_server_data)
        embed.add_field(
            name="📈 Summary",
            value=f"Total Events: {total_events}\nServers: {total_servers}",
            inline=False
        )
        
        if ctx.interaction:
            await ctx.send(embed=embed, ephemeral=True)
        else:
            await ctx.send(embed=embed)
    
    @commands.hybrid_command(name='applytemplate', description='Apply a pre-configured honeypot template')
    @commands.has_permissions(administrator=True)
    @app_commands.autocomplete(template=template_autocomplete)
    async def applytemplate(self, ctx, template: str, channel: discord.TextChannel = None):
        """Apply a honeypot template (strict/lenient/stealth)"""
        if ctx.interaction and not ctx.interaction.response.is_done():
            await ctx.defer(ephemeral=True)
        
        if channel is None:
            channel = ctx.channel
        
        templates = {
            'strict': {
                'auto_ban': True,
                'punishment_type': 'ban',
                'punishment_duration': 0,
                'min_account_age_days': 7,
                'check_patterns': True,
                'custom_message': "**⚠️ HIGH SECURITY ZONE**\n\nThis channel is a strict honeypot. Any message will result in an immediate ban."
            },
            'lenient': {
                'auto_ban': False,
                'punishment_type': 'timeout',
                'punishment_duration': 60,
                'min_account_age_days': 0,
                'check_patterns': True,
                'custom_message': "**🍯 Honeypot Channel**\n\nPlease do not send messages here. Messages will be logged."
            },
            'stealth': {
                'auto_ban': False,
                'punishment_type': 'timeout',
                'punishment_duration': 10,
                'min_account_age_days': 0,
                'check_patterns': False,
                'custom_message': "**Welcome!**\n\nFeel free to introduce yourself here."
            },
            'moderate': {
                'auto_ban': True,
                'punishment_type': 'kick',
                'punishment_duration': 0,
                'min_account_age_days': 3,
                'check_patterns': True,
                'custom_message': "**⚠️ Warning**\n\nThis is a monitored channel. Suspicious activity will result in a kick."
            },
            'newbie': {
                'auto_ban': True,
                'punishment_type': 'timeout',
                'punishment_duration': 30,
                'min_account_age_days': 1,
                'check_patterns': True,
                'custom_message': "**New User Check**\n\nThis channel helps us verify new accounts. Please wait for verification."
            },
            'investigation': {
                'auto_ban': False,
                'punishment_type': 'timeout',
                'punishment_duration': 5,
                'min_account_age_days': 0,
                'check_patterns': True,
                'custom_message': "**Rules & Guidelines**\n\nPlease read our rules before posting. Messages are monitored."
            }
        }
        
        template = template.lower()
        if template not in templates:
            if ctx.interaction:
                await ctx.send("❌ Invalid template. Use: strict, lenient, stealth, moderate, newbie, or investigation", ephemeral=True)
            else:
                await ctx.send("❌ Invalid template. Use: strict, lenient, stealth, moderate, newbie, or investigation")
            return
        
        guild_id = ctx.guild.id
        
        if guild_id not in self.honeypot_channels:
            self.honeypot_channels[guild_id] = {
                'channels': [],
                'ban_count': 0,
                'admin_role_id': None,
                'admin_user_ids': [],
                'notification_channel_id': None,
                'notification_dm': False,
                'audit_log': []
            }
        
        template_config = templates[template]
        
        # Check if channel is already a honeypot
        for existing_channel in self.honeypot_channels[guild_id]['channels']:
            if existing_channel['channel_id'] == channel.id:
                # Update existing
                existing_channel.update(template_config)
                self.save_honeypot_data()
                self.add_audit_log(guild_id, ctx.author.id, 'apply_template', {
                    'template': template,
                    'channel_id': channel.id
                })
                
                # Update embed
                embed = await self.create_honeypot_embed(template_config['custom_message'])
                try:
                    async for msg in channel.history(limit=10):
                        if msg.author == self.bot.user:
                            await msg.edit(embed=embed)
                            break
                except:
                    pass
                
                if ctx.interaction:
                    await ctx.send(f"✅ Template '{template}' applied to {channel.mention}.", ephemeral=True)
                else:
                    await ctx.send(f"✅ Template '{template}' applied to {channel.mention}.")
                return
        
        # Create new honeypot with template
        new_channel_config = {
            'channel_id': channel.id,
            'enabled': True,
            'whitelisted_users': [],
            'whitelisted_roles': []
        }
        new_channel_config.update(template_config)
        
        self.honeypot_channels[guild_id]['channels'].append(new_channel_config)
        self.save_honeypot_data()
        self.add_audit_log(guild_id, ctx.author.id, 'apply_template', {
            'template': template,
            'channel_id': channel.id
        })
        
        # Send embed
        embed = await self.create_honeypot_embed(template_config['custom_message'])
        await channel.send(embed=embed)
        
        if ctx.interaction:
            await ctx.send(f"✅ Template '{template}' applied to {channel.mention}.", ephemeral=True)
        else:
            await ctx.send(f"✅ Template '{template}' applied to {channel.mention}.")
    
    @sethoneypot.error
    @removehoneypot.error
    @honeypotlogs.error
    @clearhoneypotlogs.error
    @resetbancount.error
    @toggleautoban.error
    @bancount.error
    @setmessage.error
    @setpunishment.error
    @whitelistuser.error
    @unwhitelistuser.error
    @setaccountage.error
    @setadminchannel.error
    @toggleadminnotifications.error
    @addadmin.error
    @removeadmin.error
    @auditlog.error
    @checkoffender.error
    @crossservercheck.error
    @applytemplate.error
    @exportlogs.error
    async def command_error(self, ctx, error):
        """Handle command errors"""
        if isinstance(error, commands.MissingPermissions):
            if ctx.interaction:
                try:
                    await ctx.send("❌ You need administrator permissions to use this command.", ephemeral=True)
                except discord.NotFound:
                    pass  # Interaction already expired
            else:
                await ctx.send("❌ You need administrator permissions to use this command.")
        else:
            if ctx.interaction:
                try:
                    await ctx.send(f"An error occurred: {error}", ephemeral=True)
                except discord.NotFound:
                    pass  # Interaction already expired
            else:
                await ctx.send(f"An error occurred: {error}")

async def setup(bot):
    await bot.add_cog(HoneyPot(bot))
