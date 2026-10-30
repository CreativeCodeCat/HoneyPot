-- Tables for staff_policy; run by its store every time it opens, so each is IF NOT EXISTS.
-- Columns added later are in Rust, through database::migrations::add_column.

CREATE TABLE IF NOT EXISTS removed_roles (
    guild_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    -- Comma-separated role IDs
    role_ids TEXT NOT NULL,
    removed_at TEXT NOT NULL,
    PRIMARY KEY (guild_id, user_id)
);
CREATE TABLE IF NOT EXISTS settings (
    guild_id INTEGER PRIMARY KEY,
    -- Given to staff in place of the roles taken
    demoted_role INTEGER
);
