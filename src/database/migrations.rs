//! Schemas. Each store's tables are in `migrations/<store>.sql`, built into the bot and run
//! every time the store opens, so they're all `IF NOT EXISTS`. Columns added after a table
//! was first made go through `add_column`, so databases made by older versions catch up.

use rusqlite::Connection;

/// Adds `column` to `table` unless it's already there. `definition` is its type and
/// constraints, e.g. `INTEGER NOT NULL DEFAULT 0`.
pub fn add_column(conn: &Connection, table: &str, column: &str, definition: &str) -> rusqlite::Result<()> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |row| row.get(0),
    )?;
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_column_runs_once() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER)").unwrap();
        add_column(&conn, "t", "b", "INTEGER NOT NULL DEFAULT 3").unwrap();
        add_column(&conn, "t", "b", "INTEGER NOT NULL DEFAULT 3").unwrap();
        conn.execute_batch("INSERT INTO t (a) VALUES (1)").unwrap();
        let b: i64 = conn.query_row("SELECT b FROM t", [], |row| row.get(0)).unwrap();
        assert_eq!(b, 3);
    }
}
