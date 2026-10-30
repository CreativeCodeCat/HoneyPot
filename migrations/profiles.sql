-- Tables for profiles; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS profiles (
    user_id INTEGER PRIMARY KEY,
    updated TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS fields (
    user_id INTEGER NOT NULL,
    -- A `Field::key`
    field TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (user_id, field)
);
