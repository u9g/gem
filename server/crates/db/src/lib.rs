// The server's own database, an xitdb file whose root is a `History` of moments: each write
// appends a copy of the last moment with its changes, so a reader, even in another process, sees
// each write whole or not at all. A moment is a hash map; each part of the server keeps its state
// under its own key there, in whatever xitdb data structures fit it.
//
// The file only grows, so it is compacted to its last moment when it opens and whenever it has
// grown well past its size then.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub use xitdb;
use xitdb::{
    CoreBufferedFile, Database, DynCore, History, MemoryOffsetMap, ReadHashMap, WriteHashMap,
};
pub use xitdb::{Error, Result};

// How much the file grows past twice its compacted size before it is compacted again.
const GROWTH: i64 = 16 << 20;

pub type Map<'a> = ReadHashMap<'a, DynCore>;
pub type MapMut<'a> = WriteHashMap<'a, DynCore>;

#[derive(Clone)]
pub struct Db(Arc<Mutex<Inner>>);

enum Inner {
    // This process writes it: in the file at `path`, or in memory when None.
    Writer {
        path: Option<PathBuf>,
        db: Database<DynCore>,
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
        let start = || -> Result<(Database<DynCore>, i64)> {
            let mut db = Database::open(&path)?.into_dyn();
            History::new(&db)?;
            compact(&path, &mut db)?;
            let compacted = db.committed_size()?;
            Ok((db, compacted))
        };
        let (db, compacted) = start().map_err(|error| describe(&path, error))?;
        let path = Some(path);
        Ok(Db(Arc::new(Mutex::new(Inner::Writer { path, db, compacted }))))
    }

    pub fn open_in_memory() -> Result<Db, String> {
        let db = Database::in_memory().map(Database::into_dyn);
        let db = db.map_err(|error| format!("gem's database: {error}"))?;
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
                let read = || read(&History::new(db)?.latest()?);
                read().map_err(|error| describe_at(path.as_deref(), error))
            }
            Inner::Reader(path) => {
                let read = || -> Result<T> {
                    let db = Database::open_read_only(path)?.into_dyn();
                    read(&History::new(&db)?.latest()?)
                };
                read().map_err(|error| describe(path, error))
            }
        }
    }

    // Appends a moment with the changes `write` makes to the last one, or none when it fails or
    // changes nothing.
    pub fn write<T>(&self, write: impl FnOnce(&mut MapMut) -> Result<T>) -> Result<T, String> {
        let mut inner = self.0.lock().unwrap();
        let Inner::Writer { path, db, compacted } = &mut *inner else {
            return Err("gem's database: only the server writes it".into());
        };
        let written = History::new(db).and_then(|history| history.transact(write));
        let written = written.map_err(|error| describe_at(path.as_deref(), error))?;
        if let Some(path) = path
            && db.committed_size().is_ok_and(|size| size > 2 * *compacted + GROWTH)
        {
            match compact(path, db).and_then(|()| db.committed_size()) {
                Ok(size) => *compacted = size,
                Err(error) => eprintln!("{}", describe(path, error)),
            }
        }
        Ok(written)
    }
}

// Replaces the file at `path`, which `db` writes, with one holding only its last moment.
fn compact(path: &Path, db: &mut Database<DynCore>) -> Result<()> {
    let open = |file| Box::new(CoreBufferedFile::new(file)) as DynCore;
    db.compact_file(path, &mut MemoryOffsetMap::new(), open)
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

// Each key in the hash map at `key` in `map`, with its text.
pub fn texts(map: &Map, key: &str) -> Result<Vec<(String, String)>> {
    let Some(map) = map.get_map(key)? else {
        return Ok(Vec::new());
    };
    let pairs = map.entries()?.map(|pair| {
        let pair = pair?;
        Ok((pair.key_cursor.read_string(None)?, pair.value_cursor.read_string(None)?))
    });
    pairs.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("gem-db-{name}-{}.xitdb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn get(db: &Db, key: &str) -> Option<String> {
        db.read(|moment| moment.get_string(key, None)).unwrap()
    }

    #[test]
    fn reads_what_it_wrote() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(get(&db, "name"), None);
        db.write(|moment| {
            moment.put("name", "a")?;
            moment.put_map("names")?.put("k", "A")?;
            moment.put_set("seen")?.put("x")?;
            // A write reads its own changes.
            assert_eq!(moment.get_string("name", None)?.as_deref(), Some("a"));
            Ok(())
        })
        .unwrap();
        db.read(|moment| {
            assert_eq!(moment.get_string("name", None)?.as_deref(), Some("a"));
            assert_eq!(texts(moment, "names")?, [("k".into(), "A".into())]);
            assert!(moment.get_set("seen")?.unwrap().contains("x")?);
            assert!(moment.get_map("missing")?.is_none());
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
        assert_eq!(get(&db, "name"), None);
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
        assert_eq!(get(&db, "n").as_deref(), Some("99"));
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn another_process_reads_each_write_and_compaction() {
        let path = path("reader");
        let db = Db::open(&path).unwrap();
        let reader = Db::open_reader(&path);
        db.write(|moment| moment.put("snapshot", "1")).unwrap();
        assert_eq!(get(&reader, "snapshot").as_deref(), Some("1"));
        {
            let mut inner = db.0.lock().unwrap();
            let Inner::Writer { db: inner, .. } = &mut *inner else { unreachable!() };
            compact(&path, inner).unwrap();
        }
        db.write(|moment| moment.put("snapshot", "2")).unwrap();
        assert_eq!(get(&reader, "snapshot").as_deref(), Some("2"));
        assert!(reader.write(|moment| moment.put("snapshot", "3")).is_err());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reader_of_no_file_fails() {
        let reader = Db::open_reader(path("missing"));
        assert!(reader.read(|moment| moment.get_string("snapshot", None)).is_err());
    }
}
