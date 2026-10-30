-- Tables for reaction_roles; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS reaction_roles (
    message_id INTEGER NOT NULL,
    -- A custom emoji's ID, or the emoji itself
    emoji TEXT NOT NULL,
    guild_id INTEGER NOT NULL,
    channel_id INTEGER NOT NULL,
    role_id INTEGER NOT NULL,
    PRIMARY KEY (message_id, emoji)
);
