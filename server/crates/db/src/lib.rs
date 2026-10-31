// The server's own database, an xitdb file. Its root is a history of moments: each write appends a
// copy of the last moment with its changes, so a reader, even in another process, sees each write
// whole or not at all. A moment is a hash map from each table's name to the table, a hash map from
// each row's key to its text, to nothing in a table that is a set, or to a hash map of its fields
// in a table whose rows have several.
//
// The file only grows, so it is compacted to its last moment when it opens and whenever it has
// grown well past its size then.

mod legacy;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use xitdb::{
    Core, CoreBufferedFile, CoreMemory, Data, Database, Hasher, MemoryOffsetMap, ReadArrayList,
    ReadCursor, ReadHashMap, Slot, Tag, WriteArrayList, WriteHashMap,
};
pub use xitdb::{Error, Result};

// How much the file grows past twice its compacted size before it is compacted again.
const GROWTH: i64 = 16 << 20;

#[doc(hidden)]
pub type Store = Box<dyn Core + Send>;

#[derive(Clone)]
pub struct Db(Arc<Mutex<Inner>>);

enum Inner {
    // This process writes it: in the file at `path`, or in memory when None.
    Writer {
        path: Option<PathBuf>,
        db: Database<Store>,
        // The file's size when it was last compacted.
        compacted: i64,
    },
    // Another process writes the file at the path. It is opened again for each read, so a read
    // sees the file even after the writer replaced it with a compacted one.
    Reader(PathBuf),
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db, String> {
        let path = path.as_ref().to_path_buf();
        let start = || -> Result<(Database<Store>, i64)> {
            let mut db = open_file(&path)?;
            begin(&db)?;
            let compacted = compact(&path, &mut db)?;
            Ok((db, compacted))
        };
        let (db, compacted) = start().map_err(|error| describe(&path, error))?;
        Ok(Db(Arc::new(Mutex::new(Inner::Writer {
            path: Some(path),
            db,
            compacted,
        }))))
    }

    pub fn open_in_memory() -> Result<Db, String> {
        let start = || -> Result<Database<Store>> {
            let db = Database::new(Box::new(CoreMemory::new()) as Store, Hasher::sha1())?;
            begin(&db)?;
            Ok(db)
        };
        let db = start().map_err(|error| format!("gem's database: {error}"))?;
        Ok(Db(Arc::new(Mutex::new(Inner::Writer {
            path: None,
            db,
            compacted: 0,
        }))))
    }

    // The file another process writes at `path`, to read.
    pub fn open_reader(path: impl AsRef<Path>) -> Db {
        Db(Arc::new(Mutex::new(Inner::Reader(
            path.as_ref().to_path_buf(),
        ))))
    }

    // Reads the last moment. Reads touch a few rows of a local file, so they run on the async
    // runtime's threads.
    pub fn read<T>(&self, read: impl FnOnce(&View<'_>) -> Result<T>) -> Result<T, String> {
        let inner = self.0.lock().unwrap();
        match &*inner {
            Inner::Writer { path, db, .. } => {
                let read = || read(&View(last_moment(db)?));
                read().map_err(|error| describe_at(path.as_deref(), error))
            }
            Inner::Reader(path) => {
                let read = || -> Result<T> {
                    let file = File::open(path)?;
                    let db = Database::new(
                        Box::new(CoreBufferedFile::new(file)) as Store,
                        Hasher::sha1(),
                    )?;
                    read(&View(last_moment(&db)?))
                };
                read().map_err(|error| describe(path, error))
            }
        }
    }

    // Appends a moment with the changes `write` makes, or none when it fails.
    pub fn write<T>(&self, write: impl FnOnce(&mut Tx<'_>) -> Result<T>) -> Result<T, String> {
        let mut inner = self.0.lock().unwrap();
        let Inner::Writer {
            path,
            db,
            compacted,
        } = &mut *inner
        else {
            return Err("gem's database: only the server writes it".into());
        };
        let mut written = None;
        let append = || -> Result<()> {
            let mut history = WriteArrayList::new(&mut db.root_cursor())?;
            let last = history.get_slot(-1)?;
            history.append_context(last, |cursor| {
                written = Some(write(&mut Tx(WriteHashMap::new(cursor)?))?);
                Ok(())
            })
        };
        append().map_err(|error| describe_at(path.as_deref(), error))?;
        if let Some(path) = path
            && db
                .committed_size()
                .is_ok_and(|size| size > 2 * *compacted + GROWTH)
        {
            match compact(path, db) {
                Ok(size) => *compacted = size,
                Err(error) => eprintln!("{}", describe(path, error)),
            }
        }
        Ok(written.expect("a write that succeeds runs `write`"))
    }

    // Copies the rows of the SQLite database gem kept before, at `path`, into this one, at once.
    // Returns whether there was one.
    pub fn import_sqlite(&self, path: impl AsRef<Path>) -> Result<bool, String> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(false);
        }
        let rows = legacy::Sqlite::open(path)?.with(legacy::rows)?;
        self.write(|tx| {
            for (table, key, value) in &rows {
                match value {
                    legacy::Value::Text(text) => tx.put(table, key, text)?,
                    legacy::Value::Member => tx.insert(table, key)?,
                    legacy::Value::Row(fields) => tx.put_row(table, key, fields)?,
                }
            }
            Ok(())
        })?;
        Ok(true)
    }
}

fn open_file(path: &Path) -> Result<Database<Store>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    Database::new(
        Box::new(CoreBufferedFile::new(file)) as Store,
        Hasher::sha1(),
    )
}

// Gives a new database its first moment, with no tables, so every read has one to read.
fn begin(db: &Database<Store>) -> Result<()> {
    let mut history = WriteArrayList::new(&mut db.root_cursor())?;
    if history.count()? == 0 {
        history.append_context(None::<Slot>, |cursor| WriteHashMap::new(cursor).map(drop))?;
    }
    Ok(())
}

fn last_moment(db: &Database<Store>) -> Result<ReadHashMap<'_, Store>> {
    let history = ReadArrayList::new(db.root_cursor())?;
    ReadHashMap::new(history.get_cursor(-1)?.ok_or(Error::KeyNotFound)?)
}

// Replaces the file at `path`, which `db` writes, with one holding only its last moment, and opens
// that in `db`. Returns the new file's size.
fn compact(path: &Path, db: &mut Database<Store>) -> Result<i64> {
    let mut compacting = OsString::from(path);
    compacting.push(".compacting");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&compacting)?;
    let compacted = db.compact(CoreBufferedFile::new(file), &mut MemoryOffsetMap::new())?;
    compacted.into_core().close()?.sync_all()?;
    // Closes the file, so it can be replaced.
    *db = Database::new(Box::new(CoreMemory::new()) as Store, Hasher::sha1())?;
    let renamed = std::fs::rename(&compacting, path);
    *db = open_file(path)?;
    renamed?;
    db.committed_size()
}

fn describe(path: &Path, error: Error) -> String {
    format!("{}: {error}", path.display())
}

fn describe_at(path: Option<&Path>, error: Error) -> String {
    match path {
        Some(path) => describe(path, error),
        None => format!("gem's database: {error}"),
    }
}

// A row's field.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Text(String),
    Int(i64),
}

impl From<&str> for Field {
    fn from(text: &str) -> Field {
        Field::Text(text.into())
    }
}

impl From<&String> for Field {
    fn from(text: &String) -> Field {
        Field::Text(text.clone())
    }
}

impl From<String> for Field {
    fn from(text: String) -> Field {
        Field::Text(text)
    }
}

impl From<i64> for Field {
    fn from(int: i64) -> Field {
        Field::Int(int)
    }
}

impl From<bool> for Field {
    fn from(flag: bool) -> Field {
        Field::Int(flag.into())
    }
}

impl From<&Field> for Data<'static> {
    fn from(field: &Field) -> Data<'static> {
        match field {
            Field::Text(text) => Data::from(text.clone()),
            Field::Int(int) => Data::Int(*int),
        }
    }
}

// The fields of a row in a table whose rows have several. A field it lacks is NULL.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(BTreeMap<String, Field>);

impl Row {
    pub fn text(&self, name: &str) -> Option<String> {
        match self.0.get(name) {
            Some(Field::Text(text)) => Some(text.clone()),
            _ => None,
        }
    }

    // 0 when it lacks the field.
    pub fn int(&self, name: &str) -> i64 {
        match self.0.get(name) {
            Some(Field::Int(int)) => *int,
            _ => 0,
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        self.int(name) != 0
    }
}

// What both a read and a write can read. A table that was never written is empty.
pub trait Read {
    #[doc(hidden)]
    fn moment(&self) -> &ReadHashMap<'_, Store>;

    // The text of the row `key` of `table`.
    fn get(&self, table: &str, key: &str) -> Result<Option<String>> {
        let Some(table) = table_in(self.moment(), table)? else {
            return Ok(None);
        };
        table.get_cursor(key)?.map(|value| text(&value)).transpose()
    }

    fn contains(&self, table: &str, key: &str) -> Result<bool> {
        let Some(table) = table_in(self.moment(), table)? else {
            return Ok(false);
        };
        Ok(table.get_cursor(key)?.is_some())
    }

    // The key and text of each row of `table`.
    fn texts(&self, table: &str) -> Result<Vec<(String, String)>> {
        each_row(self.moment(), table, |key, value| Ok((key, text(value)?)))
    }

    fn keys(&self, table: &str) -> Result<Vec<String>> {
        each_row(self.moment(), table, |key, _| Ok(key))
    }

    fn row(&self, table: &str, key: &str) -> Result<Option<Row>> {
        let Some(table) = table_in(self.moment(), table)? else {
            return Ok(None);
        };
        table.get_cursor(key)?.map(|value| row(value)).transpose()
    }

    // The key and fields of each row of `table`.
    fn rows(&self, table: &str) -> Result<Vec<(String, Row)>> {
        each_row(self.moment(), table, |key, value| {
            Ok((key, row(value.clone())?))
        })
    }
}

fn table_in<'a>(
    moment: &ReadHashMap<'a, Store>,
    name: &str,
) -> Result<Option<ReadHashMap<'a, Store>>> {
    moment.get_cursor(name)?.map(ReadHashMap::new).transpose()
}

fn each_row<T>(
    moment: &ReadHashMap<'_, Store>,
    table: &str,
    mut each: impl FnMut(String, &ReadCursor<'_, Store>) -> Result<T>,
) -> Result<Vec<T>> {
    let Some(table) = table_in(moment, table)? else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for pair in table.iter()? {
        let pair = pair?.read_key_value_pair()?;
        rows.push(each(text(&pair.key_cursor)?, &pair.value_cursor)?);
    }
    Ok(rows)
}

fn text(cursor: &ReadCursor<'_, Store>) -> Result<String> {
    Ok(String::from_utf8_lossy(&cursor.read_bytes(None)?).into_owned())
}

fn row(cursor: ReadCursor<'_, Store>) -> Result<Row> {
    let mut fields = BTreeMap::new();
    for pair in ReadHashMap::new(cursor)?.iter()? {
        let pair = pair?.read_key_value_pair()?;
        let field = match pair.value_cursor.slot()?.tag {
            Tag::Int => Field::Int(pair.value_cursor.read_int()?),
            Tag::Uint => Field::Int(pair.value_cursor.read_uint()? as i64),
            _ => Field::Text(text(&pair.value_cursor)?),
        };
        fields.insert(text(&pair.key_cursor)?, field);
    }
    Ok(Row(fields))
}

// The last moment, to read.
pub struct View<'a>(ReadHashMap<'a, Store>);

impl Read for View<'_> {
    fn moment(&self) -> &ReadHashMap<'_, Store> {
        &self.0
    }
}

// The moment a write appends, to change. Its reads see its changes.
pub struct Tx<'a>(WriteHashMap<'a, Store>);

impl Read for Tx<'_> {
    fn moment(&self) -> &ReadHashMap<'_, Store> {
        &self.0
    }
}

impl<'a> Tx<'a> {
    fn table(&mut self, name: &str) -> Result<WriteHashMap<'a, Store>> {
        WriteHashMap::new(&mut self.0.put_cursor(name)?)
    }

    // Sets the row `key` of `table` to `text`.
    pub fn put(&mut self, table: &str, key: &str, text: &str) -> Result<()> {
        self.table(table)?.put(key, text)
    }

    // Adds `key` to `table`, a set.
    pub fn insert(&mut self, table: &str, key: &str) -> Result<()> {
        self.table(table)?.put(key, Data::None)
    }

    // Returns whether `table` had the row.
    pub fn remove(&mut self, table: &str, key: &str) -> Result<bool> {
        self.table(table)?.remove(key)
    }

    // Replaces the row `key` of `table` with one of `fields`.
    pub fn put_row(&mut self, table: &str, key: &str, fields: &[(&str, Field)]) -> Result<()> {
        let mut table = self.table(table)?;
        table.remove(key)?;
        let mut row = WriteHashMap::new(&mut table.put_cursor(key)?)?;
        for (name, field) in fields {
            row.put(name, field)?;
        }
        Ok(())
    }

    // Sets one field of the row `key` of `table`.
    pub fn set(
        &mut self,
        table: &str,
        key: &str,
        name: &str,
        field: impl Into<Field>,
    ) -> Result<()> {
        let field: Field = field.into();
        let mut row = WriteHashMap::new(&mut self.table(table)?.put_cursor(key)?)?;
        row.put(name, &field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("gem-db-{name}-{}.xitdb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn reads_what_it_wrote() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.read(|db| db.get("names", "a")).unwrap(), None);
        db.write(|tx| {
            tx.put("names", "a", "A")?;
            tx.insert("seen", "x")?;
            // A write reads its own changes.
            assert_eq!(tx.get("names", "a")?.as_deref(), Some("A"));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            db.read(|db| db.get("names", "a")).unwrap().as_deref(),
            Some("A")
        );
        assert_eq!(
            db.read(|db| db.texts("names")).unwrap(),
            [("a".into(), "A".into())]
        );
        assert!(db.read(|db| db.contains("seen", "x")).unwrap());
        assert!(!db.read(|db| db.contains("seen", "y")).unwrap());
        db.write(|tx| tx.remove("seen", "x")).unwrap();
        assert_eq!(db.read(|db| db.keys("seen")).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let db = Db::open_in_memory().unwrap();
        let failed = db.write(|tx| {
            tx.put("names", "a", "A")?;
            Err::<(), _>(Error::KeyNotFound)
        });
        assert!(failed.is_err());
        assert_eq!(
            db.read(|db| db.keys("names")).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn keeps_rows_fields() {
        let db = Db::open_in_memory().unwrap();
        db.write(|tx| {
            tx.put_row(
                "merges",
                "u",
                &[("title", "Merge".into()), ("started_at", 5.into())],
            )?;
            tx.set("merges", "u", "question", true)
        })
        .unwrap();
        let row = db.read(|db| db.row("merges", "u")).unwrap().unwrap();
        assert_eq!(row.text("title").as_deref(), Some("Merge"));
        assert_eq!(row.int("started_at"), 5);
        assert!(row.flag("question"));
        assert_eq!(row.text("thread_id"), None);
        // Putting a row again replaces all its fields.
        db.write(|tx| tx.put_row("merges", "u", &[("title", "Other".into())]))
            .unwrap();
        let rows = db.read(|db| db.rows("merges")).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].1.flag("question"));
    }

    #[test]
    fn survives_reopening_and_compacts() {
        let path = path("reopen");
        {
            let db = Db::open(&path).unwrap();
            for i in 0..100 {
                db.write(|tx| tx.put("proposals", "a", &format!("{{\"n\":{i}}}")))
                    .unwrap();
            }
        }
        let before = std::fs::metadata(&path).unwrap().len();
        let db = Db::open(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < before);
        assert_eq!(
            db.read(|db| db.get("proposals", "a")).unwrap().as_deref(),
            Some("{\"n\":99}")
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn another_process_reads_each_write_and_compaction() {
        let path = path("reader");
        let db = Db::open(&path).unwrap();
        let reader = Db::open_reader(&path);
        db.write(|tx| tx.put("snapshot", "response", "1")).unwrap();
        assert_eq!(
            reader
                .read(|db| db.get("snapshot", "response"))
                .unwrap()
                .as_deref(),
            Some("1")
        );
        {
            let mut inner = db.0.lock().unwrap();
            let Inner::Writer { db: inner, .. } = &mut *inner else {
                unreachable!()
            };
            compact(&path, inner).unwrap();
        }
        db.write(|tx| tx.put("snapshot", "response", "2")).unwrap();
        assert_eq!(
            reader
                .read(|db| db.get("snapshot", "response"))
                .unwrap()
                .as_deref(),
            Some("2")
        );
        assert!(
            reader
                .write(|tx| tx.put("snapshot", "response", "3"))
                .is_err()
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reader_of_no_file_fails() {
        let reader = Db::open_reader(path("missing"));
        assert!(reader.read(|db| db.get("snapshot", "response")).is_err());
    }

    #[test]
    fn imports_a_sqlite_database() {
        let sqlite =
            std::env::temp_dir().join(format!("gem-db-import-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&sqlite);
        legacy::Sqlite::open(&sqlite)
            .unwrap()
            .with(|db| {
                db.execute_batch(
                    "INSERT INTO cluster_names VALUES ('k', 'Renames');
                     INSERT INTO started_conversations VALUES ('c');
                     INSERT INTO investigations VALUES ('i', '{}', 1);
                     INSERT INTO snapshot VALUES (0, '{\"clusters\":[]}');
                     INSERT INTO merges (url, thread_id, title, started_at)
                         VALUES ('https://github.com/o/r/pull/1', 't', 'Merge r#1', 7);
                     INSERT INTO rebases (url, head) VALUES ('https://github.com/o/r/pull/2', 'h');",
                )
            })
            .unwrap();
        let db = Db::open_in_memory().unwrap();
        assert!(db.import_sqlite(&sqlite).unwrap());
        db.read(|db| {
            assert_eq!(db.get("cluster_names", "k")?.as_deref(), Some("Renames"));
            assert!(db.contains("started_conversations", "c")?);
            assert!(db.row("investigations", "i")?.unwrap().flag("closed"));
            assert_eq!(
                db.get("snapshot", "response")?.as_deref(),
                Some("{\"clusters\":[]}")
            );
            let merge = db.row("merges", "https://github.com/o/r/pull/1")?.unwrap();
            assert_eq!(merge.text("thread_id").as_deref(), Some("t"));
            assert_eq!(merge.int("started_at"), 7);
            let rebase = db.row("rebases", "https://github.com/o/r/pull/2")?.unwrap();
            assert_eq!(rebase.text("thread_id"), None);
            assert_eq!(rebase.text("head").as_deref(), Some("h"));
            Ok(())
        })
        .unwrap();
        let _ = std::fs::remove_file(&sqlite);
        assert!(!db.import_sqlite(&sqlite).unwrap());
    }
}
