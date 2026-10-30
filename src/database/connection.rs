//! Opening the SQLite databases. Each store keeps its own file under `database/`.

use rusqlite::Connection;

/// Where the database files live, relative to the working directory
pub const DIR: &str = "database";

/// Opens (creating if needed) the database at `path`; `":memory:"` opens a throwaway one.
pub fn open(path: &str) -> rusqlite::Result<Connection> {
    if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    Connection::open(path)
}
