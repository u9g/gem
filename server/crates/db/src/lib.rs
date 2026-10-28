// The server's own SQLite database. Its schema is the SQL files in migrations/, each named by the
// UTC time it was written, `YYYYMMDDHHMMSS_<what it does>.sql`, so migrations written on different
// branches never take the same name. A file never changes once it has shipped; a change to the
// schema is a new file.
//
// SQLite itself runs in gem-native (see sqlite.rs), as lust compiles no C; this is its client.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

mod migrations;
#[macro_use]
mod sqlite;

use migrations::MIGRATIONS;
pub use sqlite::{
    Connection, Error, FromSql, OptionalExtension, Result, Row, SqlValue, Statement, ToSql,
    Transaction,
};

// Each feature numbered its own migrations in `migrations (feature, version)` before they had
// names: the name of each feature's numbered one.
const NUMBERED: &[(&str, i64, &str)] = &[
    ("rebases", 1, "20261025101253_create_rebases"),
    ("merges", 1, "20261025161634_create_merges"),
    ("rebases", 2, "20261027073443_add_rebase_titles"),
    ("merges", 2, "20261027073443_add_merge_titles"),
];

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: impl AsRef<Path>) -> std::result::Result<Db, String> {
        Self::new(Connection::open(&path.as_ref().to_string_lossy()), MIGRATIONS)
    }

    pub fn open_in_memory() -> std::result::Result<Db, String> {
        Self::new(Connection::open_in_memory(), MIGRATIONS)
    }

    fn new(db: Result<Connection>, migrations: &[(&str, &str)]) -> std::result::Result<Db, String> {
        let db = Db(Arc::new(Mutex::new(db.map_err(error)?)));
        db.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY)")?;
            let numbered: bool = tx.query_row(
                "SELECT count(*) > 0 FROM sqlite_master WHERE name = 'migrations'",
                params![],
                |row| row.get(0),
            )?;
            if numbered {
                for (feature, version, name) in NUMBERED {
                    tx.execute(
                        "INSERT OR IGNORE INTO schema_migrations (name)
                         SELECT ?3 FROM migrations WHERE feature = ?1 AND version >= ?2",
                        params![feature, version, name],
                    )?;
                }
                tx.execute_batch("DROP TABLE migrations")?;
            }
            tx.commit()
        })?;
        db.migrate(migrations)?;
        Ok(db)
    }

    // Runs each migration not run yet, oldest first, each in its own transaction.
    fn migrate(&self, migrations: &[(&str, &str)]) -> std::result::Result<(), String> {
        self.with(|db| {
            for (name, sql) in migrations {
                let ran = db
                    .query_row("SELECT 1 FROM schema_migrations WHERE name = ?1", params![name], |_| {
                        Ok(())
                    })
                    .optional()?
                    .is_some();
                if ran {
                    continue;
                }
                let tx = db.unchecked_transaction()?;
                tx.execute_batch(sql)?;
                tx.execute("INSERT INTO schema_migrations (name) VALUES (?1)", params![name])?;
                tx.commit()?;
            }
            Ok(())
        })
    }

    // Queries touch a few rows of a local file, so they run on the caller's thread.
    pub fn with<T>(&self, query: impl FnOnce(&Connection) -> Result<T>) -> std::result::Result<T, String> {
        query(&self.0.lock().unwrap()).map_err(error)
    }
}

fn error(error: Error) -> String {
    format!("gem.sqlite: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIGRATIONS: &[(&str, &str)] = &[
        ("20260101000000_create_things", "CREATE TABLE things (name TEXT PRIMARY KEY)"),
        (
            "20260102000000_add_sizes",
            "ALTER TABLE things ADD COLUMN size INTEGER NOT NULL DEFAULT 0",
        ),
    ];

    fn count(db: &Db, sql: &str) -> i64 {
        db.with(|db| db.query_row(sql, params![], |row| row.get(0))).unwrap()
    }

    #[test]
    fn runs_each_migration_once() {
        let db = Db::new(Connection::open_in_memory(), &MIGRATIONS[..1]).unwrap();
        db.with(|db| db.execute("INSERT INTO things (name) VALUES ('a')", params![]))
            .unwrap();
        // A later build adds a migration; the first is not run again.
        db.migrate(MIGRATIONS).unwrap();
        db.migrate(MIGRATIONS).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM things WHERE size = 0"), 1);
        assert_eq!(count(&db, "SELECT count(*) FROM schema_migrations"), 2);
    }

    #[test]
    fn runs_a_migration_older_than_the_last_one_run() {
        let db = Db::new(Connection::open_in_memory(), &MIGRATIONS[..1]).unwrap();
        // Merged after a newer one from another branch had already run.
        let older = ("20251231000000_create_others", "CREATE TABLE others (id INTEGER)");
        db.migrate(&[older, MIGRATIONS[0]]).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM others"), 0);
    }

    #[test]
    fn a_failed_migration_is_run_again_next_time() {
        let db = Db::new(Connection::open_in_memory(), &MIGRATIONS[..1]).unwrap();
        assert!(db.migrate(&[MIGRATIONS[0], ("20260102000000_bad", "NOT SQL")]).is_err());
        assert_eq!(count(&db, "SELECT count(*) FROM schema_migrations"), 1);
    }

    #[test]
    fn names_the_migrations_features_numbered() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE migrations (feature TEXT PRIMARY KEY, version INTEGER NOT NULL);
                 INSERT INTO migrations VALUES ('rebases', 2), ('merges', 1);
                 CREATE TABLE rebases (url TEXT PRIMARY KEY, head TEXT NOT NULL, thread_id TEXT,
                     question INTEGER NOT NULL DEFAULT 0, pushed INTEGER NOT NULL DEFAULT 0,
                     title TEXT NOT NULL DEFAULT '', started_at INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE merges (url TEXT PRIMARY KEY, thread_id TEXT,
                     question INTEGER NOT NULL DEFAULT 0);",
            )
            .unwrap();
        let db = Db::new(Ok(connection), super::MIGRATIONS).unwrap();
        // Only merges' second migration had not run.
        assert_eq!(count(&db, "SELECT count(*) FROM merges WHERE title = ''"), 0);
        db.with(|db| db.execute("INSERT INTO merges (url) VALUES ('a')", params![]))
            .unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM merges WHERE title = ''"), 1);
        assert_eq!(
            count(&db, "SELECT count(*) FROM sqlite_master WHERE name = 'migrations'"),
            0
        );
    }

    #[test]
    fn survives_reopening() {
        let path = std::env::temp_dir().join(format!("gem-db-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let db = Db::open(&path).unwrap();
            db.with(|db| db.execute("INSERT INTO proposals (id, proposal) VALUES ('a', '{}')", params![]))
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM proposals"), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
