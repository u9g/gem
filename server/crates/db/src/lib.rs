// The server's own database, an xitdb file. Its root is a history of moments: each write appends a
// copy of the last moment with its changes, so a reader, even in another process, sees each write
// whole or not at all. A moment is a hash map; each part of the server keeps its state under its
// own key there, in whatever xitdb data structures fit it.
//
// The file only grows, so it is compacted to its last moment when it opens and whenever it has
// grown well past its size then.

use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub use xitdb;
use xitdb::{
    Core, CoreBufferedFile, CoreMemory, Database, Hasher, MemoryOffsetMap, ReadArrayList,
    ReadCursor, ReadHashMap, Slot, WriteArrayList, WriteHashMap,
};
pub use xitdb::{Error, Result};

// How much the file grows past twice its compacted size before it is compacted again.
const GROWTH: i64 = 16 << 20;

// How the database does i/o: to its file, or in memory in tests.
pub type Store = Box<dyn Core + Send>;
pub type Map<'a> = ReadHashMap<'a, Store>;
pub type MapMut<'a> = WriteHashMap<'a, Store>;
pub type Cursor<'a> = ReadCursor<'a, Store>;

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
        let path = Some(path);
        Ok(Db(Arc::new(Mutex::new(Inner::Writer { path, db, compacted }))))
    }

    pub fn open_in_memory() -> Result<Db, String> {
        let start = || -> Result<Database<Store>> {
            let db = Database::new(Box::new(CoreMemory::new()) as Store, Hasher::sha1())?;
            begin(&db)?;
            Ok(db)
        };
        let db = start().map_err(|error| format!("gem's database: {error}"))?;
        Ok(Db(Arc::new(Mutex::new(Inner::Writer { path: None, db, compacted: 0 }))))
    }

    // The file another process writes at `path`, to read.
    pub fn open_reader(path: impl AsRef<Path>) -> Db {
        Db(Arc::new(Mutex::new(Inner::Reader(path.as_ref().to_path_buf()))))
    }

    // Reads the last moment. Reads touch a few keys of a local file, so they run on the async
    // runtime's threads.
    pub fn read<T>(&self, read: impl FnOnce(&Map) -> Result<T>) -> Result<T, String> {
        let inner = self.0.lock().unwrap();
        match &*inner {
            Inner::Writer { path, db, .. } => {
                let read = || read(&last_moment(db)?);
                read().map_err(|error| describe_at(path.as_deref(), error))
            }
            Inner::Reader(path) => {
                let read = || -> Result<T> {
                    let file = Box::new(CoreBufferedFile::new(File::open(path)?)) as Store;
                    read(&last_moment(&Database::new(file, Hasher::sha1())?)?)
                };
                read().map_err(|error| describe(path, error))
            }
        }
    }

    // Appends a moment with the changes `write` makes to the last one, or none when it fails.
    pub fn write<T>(&self, write: impl FnOnce(&mut MapMut) -> Result<T>) -> Result<T, String> {
        let mut inner = self.0.lock().unwrap();
        let Inner::Writer { path, db, compacted } = &mut *inner else {
            return Err("gem's database: only the server writes it".into());
        };
        let mut written = None;
        let append = || -> Result<()> {
            let mut history = WriteArrayList::new(&mut db.root_cursor())?;
            let last = history.get_slot(-1)?;
            history.append_context(last, |cursor| {
                written = Some(write(&mut WriteHashMap::new(cursor)?)?);
                Ok(())
            })
        };
        append().map_err(|error| describe_at(path.as_deref(), error))?;
        if let Some(path) = path
            && db.committed_size().is_ok_and(|size| size > 2 * *compacted + GROWTH)
        {
            match compact(path, db) {
                Ok(size) => *compacted = size,
                Err(error) => eprintln!("{}", describe(path, error)),
            }
        }
        Ok(written.expect("a write that succeeds runs `write`"))
    }
}

fn open_file(path: &Path) -> Result<Database<Store>> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    Database::new(Box::new(CoreBufferedFile::new(file)) as Store, Hasher::sha1())
}

// Gives a new database its first moment, empty, so every read has one to read.
fn begin(db: &Database<Store>) -> Result<()> {
    let mut history = WriteArrayList::new(&mut db.root_cursor())?;
    if history.count()? == 0 {
        history.append_context(None::<Slot>, |cursor| WriteHashMap::new(cursor).map(drop))?;
    }
    Ok(())
}

fn last_moment(db: &Database<Store>) -> Result<Map<'_>> {
    let history = ReadArrayList::new(db.root_cursor())?;
    ReadHashMap::new(history.get_cursor(-1)?.ok_or(Error::KeyNotFound)?)
}

// Replaces the file at `path`, which `db` writes, with one holding only its last moment, and opens
// that in `db`. Returns the new file's size.
fn compact(path: &Path, db: &mut Database<Store>) -> Result<i64> {
    let mut compacting = OsString::from(path);
    compacting.push(".compacting");
    let file =
        OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&compacting)?;
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

// The bytes at `cursor`, as text.
pub fn text(cursor: &Cursor) -> Result<String> {
    Ok(String::from_utf8_lossy(&cursor.read_bytes(None)?).into_owned())
}

// The text at `key` in `map`.
pub fn get_text(map: &Map, key: &str) -> Result<Option<String>> {
    map.get_cursor(key)?.map(|value| text(&value)).transpose()
}

// The hash map at `key` in `map`, or None when there is none yet.
pub fn get_map<'a>(map: &Map<'a>, key: &str) -> Result<Option<Map<'a>>> {
    map.get_cursor(key)?.map(ReadHashMap::new).transpose()
}

// The hash map at `key` in `map`, made empty when there is none yet, to change.
pub fn map_mut<'a>(map: &mut MapMut<'a>, key: &str) -> Result<MapMut<'a>> {
    WriteHashMap::new(&mut map.put_cursor(key)?)
}

// Each key in the hash map or hash set at `cursor`, with a cursor to its value.
pub fn entries<'a>(cursor: &Cursor<'a>) -> Result<Vec<(String, Cursor<'a>)>> {
    let mut entries = Vec::new();
    for pair in cursor.iter()? {
        let pair = pair?.read_key_value_pair()?;
        entries.push((text(&pair.key_cursor)?, pair.value_cursor));
    }
    Ok(entries)
}

// Each key in the hash map at `key` in `map`, with its text.
pub fn texts(map: &Map, key: &str) -> Result<Vec<(String, String)>> {
    let Some(map) = map.get_cursor(key)? else {
        return Ok(Vec::new());
    };
    let entries = entries(&map)?.into_iter();
    entries.map(|(key, value)| Ok((key, text(&value)?))).collect()
}

#[cfg(test)]
mod tests {
    use xitdb::{ReadHashSet, WriteHashSet};

    use super::*;

    fn path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("gem-db-{name}-{}.xitdb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn reads_what_it_wrote() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.read(|moment| get_text(moment, "name")).unwrap(), None);
        db.write(|moment| {
            moment.put("name", "a")?;
            map_mut(moment, "names")?.put("k", "A")?;
            WriteHashSet::new(&mut moment.put_cursor("seen")?)?.put("x")?;
            // A write reads its own changes.
            assert_eq!(get_text(moment, "name")?.as_deref(), Some("a"));
            Ok(())
        })
        .unwrap();
        db.read(|moment| {
            assert_eq!(get_text(moment, "name")?.as_deref(), Some("a"));
            assert_eq!(texts(moment, "names")?, [("k".into(), "A".into())]);
            assert!(ReadHashSet::new(moment.get_cursor("seen")?.unwrap())?.contains("x")?);
            assert!(get_map(moment, "missing")?.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let db = Db::open_in_memory().unwrap();
        let failed = db.write(|moment| {
            moment.put("name", "a")?;
            Err::<(), _>(Error::KeyNotFound)
        });
        assert!(failed.is_err());
        assert_eq!(db.read(|moment| get_text(moment, "name")).unwrap(), None);
    }

    #[test]
    fn survives_reopening_and_compacts() {
        let path = path("reopen");
        {
            let db = Db::open(&path).unwrap();
            for i in 0..100 {
                db.write(|moment| moment.put("n", &*i.to_string())).unwrap();
            }
        }
        let before = std::fs::metadata(&path).unwrap().len();
        let db = Db::open(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < before);
        assert_eq!(db.read(|moment| get_text(moment, "n")).unwrap().as_deref(), Some("99"));
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn another_process_reads_each_write_and_compaction() {
        let path = path("reader");
        let db = Db::open(&path).unwrap();
        let reader = Db::open_reader(&path);
        let snapshot = |db: &Db| db.read(|moment| get_text(moment, "snapshot")).unwrap();
        db.write(|moment| moment.put("snapshot", "1")).unwrap();
        assert_eq!(snapshot(&reader).as_deref(), Some("1"));
        {
            let mut inner = db.0.lock().unwrap();
            let Inner::Writer { db: inner, .. } = &mut *inner else { unreachable!() };
            compact(&path, inner).unwrap();
        }
        db.write(|moment| moment.put("snapshot", "2")).unwrap();
        assert_eq!(snapshot(&reader).as_deref(), Some("2"));
        assert!(reader.write(|moment| moment.put("snapshot", "3")).is_err());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reader_of_no_file_fails() {
        let reader = Db::open_reader(path("missing"));
        assert!(reader.read(|moment| get_text(moment, "snapshot")).is_err());
    }
}
