-- Tables for honeypot; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS guilds (
    guild_id INTEGER PRIMARY KEY,
    ban_count INTEGER DEFAULT 0,
    admin_role_id INTEGER,
    notification_channel_id INTEGER,
    notification_dm BOOLEAN DEFAULT 0,
    admin_user_ids TEXT
);
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
);
CREATE TABLE IF NOT EXISTS activity_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER,
    user_id INTEGER,
    username TEXT,
    action TEXT,
    details TEXT,
    timestamp TEXT,
    FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
);
CREATE INDEX IF NOT EXISTS idx_user_id ON activity_logs(user_id);
CREATE INDEX IF NOT EXISTS idx_guild_id ON activity_logs(guild_id);
CREATE TABLE IF NOT EXISTS offenders (
    user_id INTEGER PRIMARY KEY,
    username TEXT,
    first_offense TEXT,
    last_offense TEXT,
    offense_count INTEGER DEFAULT 0,
    servers TEXT
);
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guild_id INTEGER,
    admin_id INTEGER,
    action TEXT,
    details TEXT,
    timestamp TEXT,
    FOREIGN KEY (guild_id) REFERENCES guilds(guild_id)
);
