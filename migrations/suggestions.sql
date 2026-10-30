-- Tables for suggestions; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS setup (
    guild_id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL,
    accepted_channel_id INTEGER NOT NULL,
    declined_channel_id INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS suggestions (
    guild_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    member_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL,
    dm BOOLEAN NOT NULL,
    PRIMARY KEY (guild_id, message_id)
);
