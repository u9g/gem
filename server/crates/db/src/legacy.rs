// The SQLite database the server kept before its database was xitdb, read once to import it. Its
// schema is the SQL files in migrations/, each named by the UTC time it was written,
// `YYYYMMDDHHMMSS_<what it does>.sql`, so migrations written on different branches never take the
// same name. A file never changes once it has shipped; a change to the schema is a new file.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::Field;

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

// Each feature numbered its own migrations in `migrations (feature, version)` before they had
// names: the name of each feature's numbered one.
const NUMBERED: &[(&str, i64, &str)] = &[
    ("rebases", 1, "20261025101253_create_rebases"),
    ("merges", 1, "20261025161634_create_merges"),
    ("rebases", 2, "20261027073443_add_rebase_titles"),
    ("merges", 2, "20261027073443_add_merge_titles"),
];

#[derive(Clone)]
pub struct Sqlite(Arc<Mutex<Connection>>);

impl Sqlite {
    pub fn open(path: impl AsRef<Path>) -> Result<Sqlite, String> {
        Self::new(Connection::open(path), MIGRATIONS)
    }

    fn new(
        db: rusqlite::Result<Connection>,
        migrations: &[(&str, &str)],
    ) -> Result<Sqlite, String> {
        let db = Sqlite(Arc::new(Mutex::new(db.map_err(error)?)));
        db.with(|db| {
            let tx = db.unchecked_transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY)")?;
            let numbered: bool = tx.query_row(
                "SELECT count(*) > 0 FROM sqlite_master WHERE name = 'migrations'",
                [],
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
    fn migrate(&self, migrations: &[(&str, &str)]) -> Result<(), String> {
        self.with(|db| {
            for (name, sql) in migrations {
                let ran = db
                    .query_row("SELECT 1 FROM schema_migrations WHERE name = ?1", [name], |_| {
                        Ok(())
                    })
                    .optional()?
                    .is_some();
                if ran {
                    continue;
                }
                let tx = db.unchecked_transaction()?;
                tx.execute_batch(sql)?;
                tx.execute("INSERT INTO schema_migrations (name) VALUES (?1)", [name])?;
                tx.commit()?;
            }
            Ok(())
        })
    }

    pub fn with<T>(
        &self,
        query: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        query(&self.0.lock().unwrap()).map_err(error)
    }
}

// A row to import into a table.
pub enum Value {
    Text(String),
    // A member of a set.
    Member,
    Row(Vec<(&'static str, Field)>),
}

// Every row, by table and key.
pub fn rows(db: &Connection) -> rusqlite::Result<Vec<(&'static str, String, Value)>> {
    let mut rows = Vec::new();
    let texts = [
        ("cluster_names", "SELECT key, name FROM cluster_names"),
        (
            "no_proposal_reasons",
            "SELECT id, reason FROM no_proposal_reasons",
        ),
        ("proposals", "SELECT id, proposal FROM proposals"),
        ("agent_tokens", "SELECT token, subject FROM agent_tokens"),
        ("snapshot", "SELECT 'response', response FROM snapshot"),
        ("cluster_agents", "SELECT pr, thread_id FROM cluster_agents"),
    ];
    for (table, sql) in texts {
        let mut statement = db.prepare(sql)?;
        let texts = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for text in texts {
            let (key, text) = text?;
            rows.push((table, key, Value::Text(text)));
        }
    }
    let mut statement = db.prepare("SELECT id FROM started_conversations")?;
    for id in statement.query_map([], |row| row.get(0))? {
        rows.push(("started_conversations", id?, Value::Member));
    }
    let mut statement = db.prepare("SELECT id, investigation, closed FROM investigations")?;
    let investigations = statement.query_map([], |row| {
        let fields = vec![
            ("investigation", Field::Text(row.get(1)?)),
            ("closed", Field::Int(row.get(2)?)),
        ];
        Ok((row.get(0)?, fields))
    })?;
    for investigation in investigations {
        let (id, fields) = investigation?;
        rows.push(("investigations", id, Value::Row(fields)));
    }
    let mut statement =
        db.prepare("SELECT url, thread_id, question, title, started_at FROM merges")?;
    let merges = statement.query_map([], |row| {
        let mut fields = vec![
            ("question", Field::Int(row.get(2)?)),
            ("title", Field::Text(row.get(3)?)),
            ("started_at", Field::Int(row.get(4)?)),
        ];
        if let Some(thread_id) = row.get::<_, Option<String>>(1)? {
            fields.push(("thread_id", Field::Text(thread_id)));
        }
        Ok((row.get(0)?, fields))
    })?;
    for merge in merges {
        let (url, fields) = merge?;
        rows.push(("merges", url, Value::Row(fields)));
    }
    let mut statement = db
        .prepare("SELECT url, head, thread_id, question, pushed, title, started_at FROM rebases")?;
    let rebases = statement.query_map([], |row| {
        let mut fields = vec![
            ("head", Field::Text(row.get(1)?)),
            ("question", Field::Int(row.get(3)?)),
            ("pushed", Field::Int(row.get(4)?)),
            ("title", Field::Text(row.get(5)?)),
            ("started_at", Field::Int(row.get(6)?)),
        ];
        if let Some(thread_id) = row.get::<_, Option<String>>(2)? {
            fields.push(("thread_id", Field::Text(thread_id)));
        }
        Ok((row.get(0)?, fields))
    })?;
    for rebase in rebases {
        let (url, fields) = rebase?;
        rows.push(("rebases", url, Value::Row(fields)));
    }
    Ok(rows)
}

fn error(error: rusqlite::Error) -> String {
    format!("gem.sqlite: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIGRATIONS: &[(&str, &str)] = &[
        (
            "20260101000000_create_things",
            "CREATE TABLE things (name TEXT PRIMARY KEY)",
        ),
        (
            "20260102000000_add_sizes",
            "ALTER TABLE things ADD COLUMN size INTEGER NOT NULL DEFAULT 0",
        ),
    ];

    fn count(db: &Sqlite, sql: &str) -> i64 {
        db.with(|db| db.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    #[test]
    fn runs_each_migration_once() {
        let db = Sqlite::new(Connection::open_in_memory(), &MIGRATIONS[..1]).unwrap();
        db.with(|db| db.execute("INSERT INTO things (name) VALUES ('a')", []))
            .unwrap();
        // A later build adds a migration; the first is not run again.
        db.migrate(MIGRATIONS).unwrap();
        db.migrate(MIGRATIONS).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM things WHERE size = 0"), 1);
        assert_eq!(count(&db, "SELECT count(*) FROM schema_migrations"), 2);
    }

    #[test]
    fn runs_a_migration_older_than_the_last_one_run() {
        let db = Sqlite::new(Connection::open_in_memory(), &MIGRATIONS[..1]).unwrap();
        // Merged after a newer one from another branch had already run.
        let older = (
            "20251231000000_create_others",
            "CREATE TABLE others (id INTEGER)",
        );
        db.migrate(&[older, MIGRATIONS[0]]).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM others"), 0);
    }

    #[test]
    fn a_failed_migration_is_run_again_next_time() {
        let db = Sqlite::new(Connection::open_in_memory(), &MIGRATIONS[..1]).unwrap();
        assert!(
            db.migrate(&[MIGRATIONS[0], ("20260102000000_bad", "NOT SQL")])
                .is_err()
        );
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
        let db = Sqlite::new(Ok(connection), super::MIGRATIONS).unwrap();
        // Only merges' second migration had not run.
        assert_eq!(
            count(&db, "SELECT count(*) FROM merges WHERE title = ''"),
            0
        );
        db.with(|db| db.execute("INSERT INTO merges (url) VALUES ('a')", []))
            .unwrap();
        assert_eq!(
            count(&db, "SELECT count(*) FROM merges WHERE title = ''"),
            1
        );
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM sqlite_master WHERE name = 'migrations'"
            ),
            0
        );
    }

    #[test]
    fn survives_reopening() {
        let path = std::env::temp_dir().join(format!("gem-db-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let db = Sqlite::open(&path).unwrap();
            db.with(|db| {
                db.execute(
                    "INSERT INTO proposals (id, proposal) VALUES ('a', '{}')",
                    [],
                )
            })
            .unwrap();
        }
        let db = Sqlite::open(&path).unwrap();
        assert_eq!(count(&db, "SELECT count(*) FROM proposals"), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
