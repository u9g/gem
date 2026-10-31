// The one-time move of gem's state from the SQLite database servers kept before, gem.sqlite, into
// their xitdb file, gem.xitdb, at layout version 0, which the server then migrates. The launcher
// runs it before it starts a server, and so does the server: a launcher started before this
// existed keeps running its own build, and starts new servers without it.
//
// It imports into a file beside gem.xitdb, reads everything back, and only then renames that file
// to gem.xitdb and moves gem.sqlite aside to gem.sqlite.imported, so a failure at any step leaves
// gem.sqlite to import again and no gem.xitdb a server would start empty on.
//
// Remove this crate once no machine has a gem.sqlite left.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use gem_db::{
    Db, Migrations,
    xitdb::{Map, Set},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

pub const SQLITE: &str = "gem.sqlite";

// Imports gem.sqlite in the working directory, where the server keeps its state, if it is still
// there. Returns what to log, if anything.
pub fn import_here() -> Result<Option<String>, String> {
    let message = match import(Path::new(SQLITE), Path::new(gem_db::FILE))? {
        Imported::Nothing => return Ok(None),
        Imported::Rows(rows) => format!("imported {rows} rows of {SQLITE} into {}", gem_db::FILE),
        Imported::Both => format!("{SQLITE} is still here beside {}, so it is not imported", gem_db::FILE),
    };
    Ok(Some(message))
}

pub enum Imported {
    // There is no gem.sqlite.
    Nothing,
    // Every row, with this many in all.
    Rows(usize),
    // There is a gem.sqlite, but also a gem.xitdb, which it would replace, so neither is touched.
    Both,
}

// The maps in the moment, by the names the server's modules use, that take the rows of a key and
// a text from each table.
const TEXTS: &[(&str, &str, &str)] = &[
    ("cluster-names", "cluster_names", "SELECT key, name FROM cluster_names"),
    ("no-proposal-reasons", "no_proposal_reasons", "SELECT id, reason FROM no_proposal_reasons"),
    ("proposals", "proposals", "SELECT id, proposal FROM proposals"),
    ("agent-tokens", "agent_tokens", "SELECT token, subject FROM agent_tokens"),
    ("cluster-agents", "cluster_agents", "SELECT pr, thread_id FROM cluster_agents"),
    (
        "investigations",
        "investigations",
        "SELECT id, investigation FROM investigations WHERE closed = 0",
    ),
    (
        "closed-investigations",
        "investigations",
        "SELECT id, investigation FROM investigations WHERE closed != 0",
    ),
];
const STARTED: Set<str> = Set::new("started-conversations");

// A merge or a rebase, as SQLite has it.
struct Agent {
    url: String,
    head: Option<String>,
    thread_id: Option<String>,
    title: String,
    started_at: i64,
    question: bool,
    pushed: Option<bool>,
}

struct Rows {
    texts: Vec<(&'static str, Vec<(String, String)>)>,
    started: Vec<String>,
    snapshot: Option<String>,
    merges: Vec<Agent>,
    rebases: Vec<Agent>,
}

impl Rows {
    fn count(&self) -> usize {
        let texts: usize = self.texts.iter().map(|(_, rows)| rows.len()).sum();
        texts + self.started.len() + usize::from(self.snapshot.is_some())
            + self.merges.len()
            + self.rebases.len()
    }
}

// Imports `sqlite` into `xitdb` unless there is no `sqlite`, or there already is an `xitdb`.
pub fn import(sqlite: &Path, xitdb: &Path) -> Result<Imported, String> {
    if !sqlite.exists() {
        return Ok(Imported::Nothing);
    }
    if xitdb.exists() {
        return Ok(Imported::Both);
    }
    let rows = read(sqlite).map_err(|error| format!("{}: {error}", sqlite.display()))?;
    let importing = with_suffix(xitdb, ".importing");
    let _ = fs::remove_file(&importing);
    let written = write(&importing, &rows);
    if let Err(error) = written {
        let _ = fs::remove_file(&importing);
        return Err(error);
    }
    fs::rename(&importing, xitdb).map_err(|error| format!("{}: {error}", xitdb.display()))?;
    // The rollback journal goes with it: on its own it would be applied to no database.
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let file = with_suffix(sqlite, suffix);
        if file.exists() {
            let aside = with_suffix(&file, ".imported");
            fs::rename(&file, &aside).map_err(|error| format!("{}: {error}", file.display()))?;
        }
    }
    Ok(Imported::Rows(rows.count()))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path);
    name.push(suffix);
    name.into()
}

fn read(path: &Path) -> rusqlite::Result<Rows> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // A table an older server never made has no rows.
    let has = |table: &str| -> rusqlite::Result<bool> {
        let sql = "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1";
        Ok(db.query_row(sql, [table], |_| Ok(())).optional()?.is_some())
    };
    let mut texts = Vec::new();
    for (name, table, sql) in TEXTS {
        let mut rows = Vec::new();
        if has(table)? {
            let mut statement = db.prepare(sql)?;
            for row in statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
                rows.push(row?);
            }
        }
        texts.push((*name, rows));
    }
    let mut started = Vec::new();
    if has("started_conversations")? {
        let mut statement = db.prepare("SELECT id FROM started_conversations")?;
        for id in statement.query_map([], |row| row.get(0))? {
            started.push(id?);
        }
    }
    let snapshot = match has("snapshot")? {
        true => db.query_row("SELECT response FROM snapshot", [], |row| row.get(0)).optional()?,
        false => None,
    };
    let mut merges = Vec::new();
    if has("merges")? {
        let sql = "SELECT url, thread_id, question, title, started_at FROM merges";
        let mut statement = db.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok(Agent {
                url: row.get(0)?,
                head: None,
                thread_id: row.get(1)?,
                question: row.get(2)?,
                title: row.get(3)?,
                started_at: row.get(4)?,
                pushed: None,
            })
        })?;
        for merge in rows {
            merges.push(merge?);
        }
    }
    let mut rebases = Vec::new();
    if has("rebases")? {
        let sql = "SELECT url, head, thread_id, question, pushed, title, started_at FROM rebases";
        let mut statement = db.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok(Agent {
                url: row.get(0)?,
                head: Some(row.get(1)?),
                thread_id: row.get(2)?,
                question: row.get(3)?,
                pushed: Some(row.get(4)?),
                title: row.get(5)?,
                started_at: row.get(6)?,
            })
        })?;
        for rebase in rows {
            rebases.push(rebase?);
        }
    }
    Ok(Rows { texts, started, snapshot, merges, rebases })
}

// Writes `rows` into a new xitdb file at `path` in one transaction, then reads them back.
fn write(path: &Path, rows: &Rows) -> Result<(), String> {
    let no_migrations = Migrations::new([]).map_err(|error| error.to_string())?;
    let (db, _) = Db::open(path, &no_migrations)?;
    db.write(|moment| {
        for (name, texts) in &rows.texts {
            let map: Map<str, String> = Map::new(name);
            for (key, text) in texts {
                map.put(moment, key, text)?;
            }
        }
        for id in &rows.started {
            STARTED.insert(moment, id)?;
        }
        if let Some(snapshot) = &rows.snapshot {
            moment.put("snapshot", &**snapshot)?;
        }
        // Merges and rebases are hash maps of their fields, as the servers' records store them:
        // `question` and `pushed` as 0 or 1, and no `thread` while it starts.
        for (name, agents) in [("merges", &rows.merges), ("rebases", &rows.rebases)] {
            let mut map = moment.put_map(name)?;
            for agent in agents {
                let mut fields = map.put_map(&agent.url)?;
                if let Some(head) = &agent.head {
                    fields.put("head", &**head)?;
                }
                if let Some(thread_id) = &agent.thread_id {
                    fields.put("thread", &**thread_id)?;
                }
                fields.put("title", &*agent.title)?;
                fields.put("started-at", agent.started_at)?;
                fields.put("question", u64::from(agent.question))?;
                if let Some(pushed) = agent.pushed {
                    fields.put("pushed", u64::from(pushed))?;
                }
            }
        }
        Ok(())
    })?;
    check(&db, rows).map_err(|error| format!("{}: {error}", path.display()))
}

// Reads back what `write` wrote, and compares it with `rows`.
fn check(db: &Db, rows: &Rows) -> Result<(), String> {
    let wrong = |what: &str| Err(format!("{what} did not import whole"));
    for (name, texts) in &rows.texts {
        let map: Map<str, String> = Map::new(name);
        for (key, text) in texts {
            if db.read(|moment| map.get(moment, key))?.as_ref() != Some(text) {
                return wrong(name);
            }
        }
        if db.read(|moment| Ok(map.iter(moment)?.count()))? != texts.len() {
            return wrong(name);
        }
    }
    if db.read(|moment| Ok(STARTED.iter(moment)?.count()))? != rows.started.len() {
        return wrong(STARTED.name());
    }
    if db.read(|moment| moment.get_string("snapshot", None))? != rows.snapshot {
        return wrong("snapshot");
    }
    for (name, agents) in [("merges", &rows.merges), ("rebases", &rows.rebases)] {
        let count = db.read(|moment| match moment.get_map(name)? {
            Some(map) => Ok(map.entries()?.count()),
            None => Ok(0),
        })?;
        if count != agents.len() {
            return wrong(name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // gem.sqlite's tables as the last server that kept it made them.
    const SCHEMA: &str = "
        CREATE TABLE rebases (url TEXT PRIMARY KEY, head TEXT NOT NULL, thread_id TEXT,
            question INTEGER NOT NULL DEFAULT 0, pushed INTEGER NOT NULL DEFAULT 0,
            title TEXT NOT NULL DEFAULT '', started_at INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE merges (url TEXT PRIMARY KEY, thread_id TEXT,
            question INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL DEFAULT '',
            started_at INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE cluster_names (key TEXT PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE started_conversations (id TEXT PRIMARY KEY);
        CREATE TABLE no_proposal_reasons (id TEXT PRIMARY KEY, reason TEXT NOT NULL);
        CREATE TABLE proposals (id TEXT PRIMARY KEY, proposal TEXT NOT NULL);
        CREATE TABLE investigations (id TEXT PRIMARY KEY, investigation TEXT NOT NULL,
            closed INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE agent_tokens (token TEXT PRIMARY KEY, subject TEXT NOT NULL);
        CREATE TABLE snapshot (id INTEGER PRIMARY KEY CHECK (id = 0), response TEXT NOT NULL);
        CREATE TABLE cluster_agents (pr TEXT PRIMARY KEY, thread_id TEXT NOT NULL);
    ";

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gem-sqlite-import-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sqlite(dir: &Path, sql: &str) -> PathBuf {
        let path = dir.join("gem.sqlite");
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        path
    }

    #[test]
    fn imports_every_row_then_moves_the_sqlite_file_aside() {
        let dir = dir("import");
        let sqlite = sqlite(
            &dir,
            &format!(
                "{SCHEMA}
                INSERT INTO cluster_names VALUES ('k', 'Renames');
                INSERT INTO started_conversations VALUES ('c1'), ('c2');
                INSERT INTO no_proposal_reasons VALUES ('f', 'flaky');
                INSERT INTO proposals VALUES ('p', '{{\"id\":\"p\"}}');
                INSERT INTO investigations VALUES ('i1', '{{}}', 0), ('i2', '{{}}', 1);
                INSERT INTO agent_tokens VALUES ('t', '{{\"cluster\":{{\"pr\":\"u\"}}}}');
                INSERT INTO snapshot VALUES (0, '{{\"clusters\":[]}}');
                INSERT INTO cluster_agents VALUES ('u', 'thread-a');
                INSERT INTO merges (url, thread_id, question, title, started_at)
                    VALUES ('m', 'thread-m', 1, 'Merge c', 7);
                INSERT INTO rebases (url, head) VALUES ('r', 'abc');"
            ),
        );
        let xitdb = dir.join("gem.xitdb");
        let Imported::Rows(rows) = import(&sqlite, &xitdb).unwrap() else { panic!() };
        assert_eq!(rows, 12);
        assert!(!sqlite.exists() && dir.join("gem.sqlite.imported").exists());
        assert!(!dir.join("gem.xitdb.importing").exists());

        let db = Db::open_reader(&xitdb, Migrations::new([]).unwrap());
        db.read(|moment| {
            let names: Map<str, String> = Map::new("cluster-names");
            assert_eq!(names.get(moment, "k")?.as_deref(), Some("Renames"));
            assert!(STARTED.contains(moment, "c2")?);
            let closed: Map<str, String> = Map::new("closed-investigations");
            assert!(closed.contains(moment, "i2")? && !closed.contains(moment, "i1")?);
            assert_eq!(moment.get_string("snapshot", None)?.as_deref(), Some("{\"clusters\":[]}"));
            let merge = moment.get_map("merges")?.unwrap().get_map("m")?.unwrap();
            assert_eq!(merge.get_string("thread", None)?.as_deref(), Some("thread-m"));
            assert_eq!((merge.get_int("started-at")?, merge.get_uint("question")?), (Some(7), Some(1)));
            let rebase = moment.get_map("rebases")?.unwrap().get_map("r")?.unwrap();
            assert_eq!(rebase.get_string("head", None)?.as_deref(), Some("abc"));
            assert_eq!(rebase.get_string("thread", None)?, None);
            assert_eq!(rebase.get_uint("pushed")?, Some(0));
            Ok(())
        })
        .unwrap();

        // Once imported, there is nothing to import.
        assert!(matches!(import(&sqlite, &xitdb).unwrap(), Imported::Nothing));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_both_files_when_there_already_is_an_xitdb_file() {
        let dir = dir("both");
        let sqlite = sqlite(&dir, SCHEMA);
        let xitdb = dir.join("gem.xitdb");
        fs::write(&xitdb, b"not empty").unwrap();
        assert!(matches!(import(&sqlite, &xitdb).unwrap(), Imported::Both));
        assert!(sqlite.exists());
        assert_eq!(fs::read(&xitdb).unwrap(), b"not empty");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_a_database_from_before_some_tables() {
        let dir = dir("older");
        let sqlite = sqlite(&dir, "CREATE TABLE cluster_names (key TEXT PRIMARY KEY, name TEXT NOT NULL);
            INSERT INTO cluster_names VALUES ('k', 'Renames');");
        let Imported::Rows(rows) = import(&sqlite, &dir.join("gem.xitdb")).unwrap() else { panic!() };
        assert_eq!(rows, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_import_leaves_the_sqlite_file_and_no_xitdb_file() {
        let dir = dir("failed");
        let sqlite = dir.join("gem.sqlite");
        fs::write(&sqlite, b"not a database").unwrap();
        let xitdb = dir.join("gem.xitdb");
        assert!(import(&sqlite, &xitdb).is_err());
        assert!(sqlite.exists() && !xitdb.exists() && !dir.join("gem.xitdb.importing").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
