// The server's own SQLite database. Each feature owns its tables and migrates them under its own
// name, so features add tables without coordinating with each other or with this crate.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

pub use rusqlite;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, String> {
        Self::new(Connection::open(path))
    }

    pub fn open_in_memory() -> Result<Db, String> {
        Self::new(Connection::open_in_memory())
    }

    fn new(db: rusqlite::Result<Connection>) -> Result<Db, String> {
        let db = db.map_err(error)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS migrations (
                feature TEXT PRIMARY KEY,
                version INTEGER NOT NULL
            )",
        )
        .map_err(error)?;
        Ok(Db(Arc::new(Mutex::new(db))))
    }

    // Runs the feature's migrations it has not run yet, each in its own transaction. A feature
    // appends to its list; it never edits an entry that has shipped.
    pub fn migrate(&self, feature: &str, migrations: &[&str]) -> Result<(), String> {
        self.with(|db| {
            let version: usize = db
                .query_row(
                    "SELECT version FROM migrations WHERE feature = ?1",
                    [feature],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            for (index, migration) in migrations.iter().enumerate().skip(version) {
                let tx = db.unchecked_transaction()?;
                tx.execute_batch(migration)?;
                tx.execute(
                    "INSERT INTO migrations (feature, version) VALUES (?1, ?2)
                     ON CONFLICT (feature) DO UPDATE SET version = excluded.version",
                    params![feature, index + 1],
                )?;
                tx.commit()?;
            }
            Ok(())
        })
    }

    // Queries touch a few rows of a local file, so they run on the async runtime's threads.
    pub fn with<T>(
        &self,
        query: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        query(&self.0.lock().unwrap()).map_err(error)
    }
}

fn error(error: rusqlite::Error) -> String {
    format!("gem.sqlite: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIGRATIONS: &[&str] = &[
        "CREATE TABLE things (name TEXT PRIMARY KEY)",
        "ALTER TABLE things ADD COLUMN size INTEGER NOT NULL DEFAULT 0",
    ];

    fn count(db: &Db, sql: &str) -> i64 {
        db.with(|db| db.query_row(sql, [], |row| row.get(0))).unwrap()
    }

    #[test]
    fn runs_each_migration_once() {
        let db = Db::open_in_memory().unwrap();
        db.migrate("things", &MIGRATIONS[..1]).unwrap();
        db.with(|db| db.execute("INSERT INTO things (name) VALUES ('a')", []))
            .unwrap();
        // A later build appends a migration; the first is not run again.
        db.migrate("things", MIGRATIONS).unwrap();
        db.migrate("things", MIGRATIONS).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM things WHERE size = 0"), 1);
        assert_eq!(
            count(&db, "SELECT version FROM migrations WHERE feature = 'things'"),
            2
        );
    }

    #[test]
    fn features_migrate_independently() {
        let db = Db::open_in_memory().unwrap();
        db.migrate("things", MIGRATIONS).unwrap();
        db.migrate("others", &["CREATE TABLE others (id INTEGER)"])
            .unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM migrations"), 2);
    }

    #[test]
    fn a_failed_migration_leaves_its_version() {
        let db = Db::open_in_memory().unwrap();
        db.migrate("things", &MIGRATIONS[..1]).unwrap();
        assert!(db.migrate("things", &[MIGRATIONS[0], "NOT SQL"]).is_err());
        assert_eq!(
            count(&db, "SELECT version FROM migrations WHERE feature = 'things'"),
            1
        );
    }

    #[test]
    fn survives_reopening() {
        let path = std::env::temp_dir().join(format!("gem-db-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let db = Db::open(&path).unwrap();
            db.migrate("things", MIGRATIONS).unwrap();
            db.with(|db| db.execute("INSERT INTO things (name) VALUES ('a')", []))
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        db.migrate("things", MIGRATIONS).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM things"), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
