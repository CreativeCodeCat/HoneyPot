-- Tables for pets; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS pets (
    guild_id INTEGER NOT NULL,
    owner_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    born INTEGER NOT NULL,
    food REAL NOT NULL,
    fun REAL NOT NULL,
    exp REAL NOT NULL,
    male BOOLEAN NOT NULL,
    -- Unix time hunger and boredom were last applied
    last_decay INTEGER NOT NULL,
    PRIMARY KEY (guild_id, owner_id)
);
