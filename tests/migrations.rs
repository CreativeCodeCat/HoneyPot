//! Every schema in migrations/ runs on a fresh database, and again on one it already made,
//! as each store runs its schema every time it opens.

use rusqlite::Connection;

#[test]
fn migrations_run_twice() {
    let mut files: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for path in files {
        let sql = std::fs::read_to_string(&path).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        for run in ["first", "second"] {
            conn.execute_batch(&sql).unwrap_or_else(|e| panic!("{} failed on the {run} run: {e}", path.display()));
        }
    }
}
