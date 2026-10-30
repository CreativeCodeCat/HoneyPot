-- Tables for server_setup; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER NOT NULL,
    key TEXT NOT NULL,
    -- Channel or role ID
    value INTEGER NOT NULL,
    PRIMARY KEY (guild_id, key)
);
-- Settings from /server-setup options that were removed, as nothing used them
DELETE FROM settings WHERE key LIKE 'status.%' OR key LIKE 'command.%'
    OR key IN ('channel.counting', 'channel.invite') OR key LIKE 'roles.%';
