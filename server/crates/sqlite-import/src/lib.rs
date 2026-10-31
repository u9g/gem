// The one-time move of gem's state from the SQLite database servers kept before, gem.sqlite, into
// their xitdb file, gem.xitdb, at layout version 0, which the server then migrates. The launcher
// runs it before it starts a server, and so does the server: a launcher started before this
// existed keeps running its own build, and starts new servers without it.
//
// It imports into a file beside gem.xitdb, reads everything back from that file, and only then
// renames it to gem.xitdb, so a failure leaves gem.sqlite to import again and no gem.xitdb a server
// would start empty on. gem.sqlite is then moved aside, which can fail while an older `gem` MCP
// server still has it open; the next start tries again.
//
// Remove this crate once no machine has a gem.sqlite left.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use gem_db::{
    Db, Migrations,
    xitdb::{Database, History, Map, Set},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

pub const SQLITE: &str = "gem.sqlite";

// The last migration the servers that kept gem.sqlite ran on it. One that hasn't run it holds rows
// the import doesn't know how to read, e.g. tokens of agents that no longer exist.
const LAST_MIGRATION: &str = "20261030120000_create_cluster_agents";

// In an imported gem.xitdb's moment, how many rows it imported, so a gem.xitdb beside a gem.sqlite
// is known to be the import of it.
const IMPORTED: &str = "imported-from-sqlite";

// Imports gem.sqlite in the working directory, where the server keeps its state, if it is still
// there. Returns what to log, if anything.
pub fn import_here() -> Result<Option<String>, String> {
    let (sqlite, xitdb) = (Path::new(SQLITE), Path::new(gem_db::FILE));
    let message = match import(sqlite, xitdb)? {
        Imported::Nothing => return Ok(None),
        Imported::Rows { rows, left: None } => {
            format!("imported {rows} rows of {SQLITE} into {}", gem_db::FILE)
        }
        Imported::Rows { rows, left: Some(error) } => format!(
            "imported {rows} rows of {SQLITE} into {}, but cannot move it aside yet: {error}",
            gem_db::FILE
        ),
        Imported::Before { left: None } => format!("moved {SQLITE}, imported before, aside"),
        Imported::Before { left: Some(error) } => {
            format!("cannot move {SQLITE}, imported before, aside yet: {error}")
        }
    };
    Ok(Some(message))
}

pub enum Imported {
    // There is no gem.sqlite.
    Nothing,
    // Every row, with this many in all, and why gem.sqlite is still there, if it is.
    Rows { rows: usize, left: Option<String> },
    // gem.xitdb is the import of gem.sqlite, which an earlier import could not move aside.
    Before { left: Option<String> },
}

// The maps in the moment, by the names the server's modules use, that take the rows of a key and
// a text from each table.
const TEXTS: &[(&str, &str)] = &[
    ("cluster-names", "SELECT key, name FROM cluster_names"),
    ("no-proposal-reasons", "SELECT id, reason FROM no_proposal_reasons"),
    ("proposals", "SELECT id, proposal FROM proposals"),
    ("agent-tokens", "SELECT token, subject FROM agent_tokens"),
    ("cluster-agents", "SELECT pr, thread_id FROM cluster_agents"),
    ("investigations", "SELECT id, investigation FROM investigations WHERE closed = 0"),
    ("closed-investigations", "SELECT id, investigation FROM investigations WHERE closed != 0"),
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
        let agents = self.merges.len() + self.rebases.len();
        texts + self.started.len() + usize::from(self.snapshot.is_some()) + agents
    }
}

// Imports `sqlite` into `xitdb`, unless there is no `sqlite`. With an `xitdb` already there that is
// not the import of it, it touches neither and fails: a server would run on the wrong state.
pub fn import(sqlite: &Path, xitdb: &Path) -> Result<Imported, String> {
    if !sqlite.exists() {
        return Ok(Imported::Nothing);
    }
    if xitdb.exists() {
        return match imported_rows(xitdb) {
            Ok(Some(_)) => Ok(Imported::Before { left: move_aside(sqlite) }),
            Ok(None) => Err(format!(
                "{} is here beside {}, but is not its import; move one of them away",
                xitdb.display(),
                sqlite.display()
            )),
            Err(error) => Err(format!("{}: {error}", xitdb.display())),
        };
    }
    // What a failed import left, compacted copies included.
    let importing = with_suffix(xitdb, ".importing");
    remove_starting_with(&importing);
    let rows = read(sqlite)?;
    let written = write(&importing, &rows).and_then(|()| check(&importing, &rows));
    if let Err(error) = written {
        remove_starting_with(&importing);
        return Err(error);
    }
    fs::rename(&importing, xitdb).map_err(|error| format!("{}: {error}", xitdb.display()))?;
    // Its lock file.
    remove_starting_with(&importing);
    Ok(Imported::Rows { rows: rows.count(), left: move_aside(sqlite) })
}

// Moves `sqlite` and its rollback journal aside, to names no earlier import took. Returns why it
// could not, if it could not.
fn move_aside(sqlite: &Path) -> Option<String> {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let file = with_suffix(sqlite, suffix);
        if !file.exists() {
            continue;
        }
        let mut aside = with_suffix(&file, ".imported");
        let mut n = 1;
        while aside.exists() {
            n += 1;
            aside = with_suffix(&file, &format!(".imported-{n}"));
        }
        if let Err(error) = fs::rename(&file, &aside) {
            return Some(format!("{}: {error}", file.display()));
        }
    }
    None
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path);
    name.push(suffix);
    name.into()
}

// Removes the files whose names start with `path`'s.
fn remove_starting_with(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let name = name.to_string_lossy();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&*name) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// How many rows `xitdb` imported, or None if it is not an import.
fn imported_rows(xitdb: &Path) -> gem_db::Result<Option<u64>> {
    let db = Database::open_read_only(xitdb)?;
    History::new(&db)?.latest()?.get_uint(IMPORTED)
}

fn read(path: &Path) -> Result<Rows, String> {
    let read = || -> Result<Rows, String> {
        // Read-write, so SQLite rolls back what a server killed while writing left in its journal.
        let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|error| error.to_string())?;
        let has = |sql: &str, name: &str| -> rusqlite::Result<bool> {
            Ok(db.query_row(sql, [name], |_| Ok(())).optional()?.is_some())
        };
        let table = "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1";
        let ran = "SELECT 1 FROM schema_migrations WHERE name = ?1";
        let migrated = has(table, "schema_migrations").and_then(|has_table| {
            Ok(has_table && has(ran, LAST_MIGRATION)?)
        });
        if !migrated.map_err(|error| error.to_string())? {
            return Err(format!(
                "a server older than the last that kept it wrote it, before {LAST_MIGRATION}: \
                 start that build once first"
            ));
        }
        // One read transaction, so the rows are all from one moment.
        let tx = db.transaction().map_err(|error| error.to_string())?;
        rows(&tx).map_err(|error| error.to_string())
    };
    read().map_err(|error| format!("{}: {error}", path.display()))
}

fn rows(db: &Connection) -> rusqlite::Result<Rows> {
    let mut texts = Vec::new();
    for (name, sql) in TEXTS {
        let mut rows = Vec::new();
        let mut statement = db.prepare(sql)?;
        for row in statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            rows.push(row?);
        }
        texts.push((*name, rows));
    }
    let mut started = Vec::new();
    let mut statement = db.prepare("SELECT id FROM started_conversations")?;
    for id in statement.query_map([], |row| row.get(0))? {
        started.push(id?);
    }
    let snapshot =
        db.query_row("SELECT response FROM snapshot", [], |row| row.get(0)).optional()?;
    let mut merges = Vec::new();
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
    let mut rebases = Vec::new();
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
    Ok(Rows { texts, started, snapshot, merges, rebases })
}

// Writes `rows` into a new xitdb file at `path` in one transaction.
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
        moment.put(IMPORTED, rows.count() as u64)
    })
}

// Reads the file at `path` again, as a server would, and compares it with `rows`. Returns the
// first part that differs.
fn check(path: &Path, rows: &Rows) -> Result<(), String> {
    let no_migrations = Migrations::new([]).map_err(|error| error.to_string())?;
    let db = Db::open_reader(path, no_migrations);
    let differs = db.read(|moment| {
        for (name, texts) in &rows.texts {
            let map: Map<str, String> = Map::new(name);
            for (key, text) in texts {
                if map.get(moment, key)?.as_ref() != Some(text) {
                    return Ok(Some(*name));
                }
            }
            if map.iter(moment)?.count() != texts.len() {
                return Ok(Some(*name));
            }
        }
        for id in &rows.started {
            if !STARTED.contains(moment, id)? {
                return Ok(Some(STARTED.name()));
            }
        }
        if STARTED.iter(moment)?.count() != rows.started.len() {
            return Ok(Some(STARTED.name()));
        }
        if moment.get_string("snapshot", None)? != rows.snapshot {
            return Ok(Some("snapshot"));
        }
        for (name, agents) in [("merges", &rows.merges), ("rebases", &rows.rebases)] {
            let Some(map) = moment.get_map(name)? else {
                return Ok(Some(name));
            };
            if map.entries()?.count() != agents.len() {
                return Ok(Some(name));
            }
            for agent in agents {
                let Some(fields) = map.get_map(&agent.url)? else {
                    return Ok(Some(name));
                };
                let same = fields.get_string("head", None)? == agent.head
                    && fields.get_string("thread", None)? == agent.thread_id
                    && fields.get_string("title", None)?.as_ref() == Some(&agent.title)
                    && fields.get_int("started-at")? == Some(agent.started_at)
                    && fields.get_uint("question")? == Some(u64::from(agent.question))
                    && fields.get_uint("pushed")? == agent.pushed.map(u64::from);
                if !same {
                    return Ok(Some(name));
                }
            }
        }
        Ok(None)
    })?;
    match differs {
        None => Ok(()),
        Some(name) => Err(format!("{}: {name} did not import whole", path.display())),
    }
}

// gem.sqlite's tables as the last server that kept it made them, with its migrations run, for
// tests that make one.
pub const SCHEMA: &str = "
    CREATE TABLE schema_migrations (name TEXT PRIMARY KEY);
    INSERT INTO schema_migrations VALUES ('20261030120000_create_cluster_agents');
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

#[cfg(test)]
mod tests {
    use super::*;


    fn dir(name: &str) -> PathBuf {
        let id = std::process::id();
        let dir = std::env::temp_dir().join(format!("gem-sqlite-import-{name}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sqlite(dir: &Path, sql: &str) -> PathBuf {
        let path = dir.join("gem.sqlite");
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        path
    }

    fn rows(imported: Imported) -> usize {
        match imported {
            Imported::Rows { rows, left: None } => rows,
            _ => panic!("not imported whole"),
        }
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
        assert_eq!(rows(import(&sqlite, &xitdb).unwrap()), 12);
        assert!(!sqlite.exists() && dir.join("gem.sqlite.imported").exists());
        assert!(!dir.join("gem.xitdb.importing").exists());

        let db = Db::open_reader(&xitdb, Migrations::new([]).unwrap());
        db.read(|moment| {
            let names: Map<str, String> = Map::new("cluster-names");
            assert_eq!(names.get(moment, "k")?.as_deref(), Some("Renames"));
            assert!(STARTED.contains(moment, "c2")?);
            let closed: Map<str, String> = Map::new("closed-investigations");
            assert!(closed.contains(moment, "i2")? && !closed.contains(moment, "i1")?);
            let snapshot = moment.get_string("snapshot", None)?;
            assert_eq!(snapshot.as_deref(), Some("{\"clusters\":[]}"));
            let merge = moment.get_map("merges")?.unwrap().get_map("m")?.unwrap();
            assert_eq!(merge.get_string("thread", None)?.as_deref(), Some("thread-m"));
            assert_eq!(merge.get_int("started-at")?, Some(7));
            assert_eq!(merge.get_uint("question")?, Some(1));
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
    fn moves_the_sqlite_file_aside_later_without_replacing_an_earlier_one() {
        let dir = dir("later");
        let sqlite = sqlite(&dir, SCHEMA);
        let xitdb = dir.join("gem.xitdb");
        rows(import(&sqlite, &xitdb).unwrap());
        // As if moving it aside had failed, or something made it again.
        fs::copy(dir.join("gem.sqlite.imported"), &sqlite).unwrap();
        assert!(matches!(import(&sqlite, &xitdb).unwrap(), Imported::Before { left: None }));
        assert!(!sqlite.exists());
        let aside = ["gem.sqlite.imported", "gem.sqlite.imported-2"];
        assert!(aside.iter().all(|name| dir.join(name).exists()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_an_xitdb_file_that_is_not_its_import() {
        let dir = dir("both");
        let sqlite = sqlite(&dir, SCHEMA);
        let xitdb = dir.join("gem.xitdb");
        let (db, _) = Db::open(&xitdb, &Migrations::new([]).unwrap()).unwrap();
        drop(db);
        let before = fs::read(&xitdb).unwrap();
        assert!(import(&sqlite, &xitdb).is_err());
        assert!(sqlite.exists());
        assert_eq!(fs::read(&xitdb).unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_a_database_without_the_last_migration() {
        let dir = dir("older");
        let sqlite = sqlite(&dir, &SCHEMA.replace("20261030120000_create_cluster_agents", "older"));
        let xitdb = dir.join("gem.xitdb");
        let error = import(&sqlite, &xitdb).err().unwrap();
        assert!(error.contains("start that build once first"), "{error}");
        assert!(sqlite.exists() && !xitdb.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_import_leaves_the_sqlite_file_and_no_xitdb_file() {
        let dir = dir("failed");
        let sqlite = dir.join("gem.sqlite");
        fs::write(&sqlite, b"not a database").unwrap();
        // Left by an import that failed before.
        fs::write(dir.join("gem.xitdb.importing.compacting-0"), b"").unwrap();
        let xitdb = dir.join("gem.xitdb");
        assert!(import(&sqlite, &xitdb).is_err());
        assert!(sqlite.exists() && !xitdb.exists());
        let left = fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name());
        assert_eq!(left.collect::<Vec<_>>(), ["gem.sqlite"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
