-- Tables for economy; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS accounts (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    wallet INTEGER NOT NULL DEFAULT 0,
    bank INTEGER NOT NULL DEFAULT 0,
    -- Unix time of the last /eco work
    last_work INTEGER,
    PRIMARY KEY (guild_id, user_id)
);
CREATE TABLE IF NOT EXISTS daily_rates (
    guild_id INTEGER NOT NULL,
    -- 0 for the default rate
    role_id INTEGER NOT NULL,
    min_amount INTEGER NOT NULL,
    max_amount INTEGER NOT NULL,
    PRIMARY KEY (guild_id, role_id)
);
-- Bets taken from wallets for games still being played, refunded if the bot stops
CREATE TABLE IF NOT EXISTS held_bets (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    -- e.g. "heist"
    game TEXT NOT NULL,
    amount INTEGER NOT NULL,
    PRIMARY KEY (guild_id, user_id, game)
);
