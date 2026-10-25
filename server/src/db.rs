// The server's own database, gem.sqlite in its working directory next to its other state. It
// outlives the server, which the launcher restarts onto every new commit of main.

use std::sync::{Mutex, OnceLock};

use rusqlite::Connection;

const FILE: &str = "gem.sqlite";

// Each entry runs once, in order, on a database whose user_version is its index. Append new
// entries; never edit one that has shipped.
const MIGRATIONS: &[&str] = &["
    -- A T3 Code agent rebasing a pull request, until its thread settles or stops and GitHub
    -- lists any new head.
    CREATE TABLE rebases (
        url TEXT PRIMARY KEY,
        -- The head commit when the rebase started.
        head TEXT NOT NULL,
        -- NULL while the thread is starting.
        thread_id TEXT,
        question INTEGER NOT NULL DEFAULT 0,
        pushed INTEGER NOT NULL DEFAULT 0
    );
"];

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn open() -> rusqlite::Result<()> {
    let mut db = Connection::open(FILE)?;
    let version: usize = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = db.transaction()?;
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", index + 1)?;
        tx.commit()?;
    }
    let _ = DB.set(Mutex::new(db));
    Ok(())
}

// Queries touch a few rows of a local file, so they run on the async runtime's threads.
pub fn with<T>(query: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    let db = DB.get().expect("db::open runs before the server starts");
    query(&db.lock().unwrap()).map_err(|e| format!("gem.sqlite: {e}"))
}
