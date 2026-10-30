-- Tables for blacklist; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS blacklist (
    -- 'server' or 'user'
    kind TEXT NOT NULL,
    id INTEGER NOT NULL,
    name TEXT NOT NULL,
    added TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);
