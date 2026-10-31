// The server's own database, an xitdb file whose root is a `History` of moments: each write
// appends a copy of the last moment with its changes, so a reader, even in another process, sees
// each write whole or not at all. A moment is a hash map; each part of the server keeps its state
// under its own key there, in typed collections of xitdb's data structures.
//
// The layout of a moment has a version, which the server brings up to date with its `Migrations`
// when it opens the file, and which readers in other processes check before they read.
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
pub use xitdb::{Migrated, Migration};
pub use xitdb::{Error, Result};

// How much the file grows past twice its compacted size before it is compacted again.
const GROWTH: i64 = 16 << 20;

pub type Map<'a> = ReadHashMap<'a, DynCore>;
pub type MapMut<'a> = WriteHashMap<'a, DynCore>;
pub type Migrations = xitdb::Migrations<DynCore>;

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
    // sees the file even after the writer replaced it with a compacted one, and it checks the
    // layout is the one `migrations` bring it to.
    Reader { path: PathBuf, migrations: Migrations },
}

impl Db {
    // Opens the file at `path`, or makes it, and brings its layout up to date with `migrations`.
    pub fn open(path: impl AsRef<Path>, migrations: &Migrations) -> Result<(Db, Migrated), String> {
        let path = path.as_ref().to_path_buf();
        let start = || -> Result<(Database<DynCore>, Migrated, i64)> {
            let mut db = Database::open(&path)?.into_dyn();
            let migrated = History::new(&db)?.migrate(migrations)?;
            compact(&path, &mut db)?;
            let compacted = db.committed_size()?;
            Ok((db, migrated, compacted))
        };
        let (db, migrated, compacted) = start().map_err(|error| describe(&path, error))?;
        let path = Some(path);
        Ok((Db(Arc::new(Mutex::new(Inner::Writer { path, db, compacted }))), migrated))
    }

    pub fn open_in_memory() -> Result<Db, String> {
        let db = Database::in_memory().map(Database::into_dyn);
        let db = db.map_err(|error| format!("gem's database: {error}"))?;
        Ok(Db(Arc::new(Mutex::new(Inner::Writer { path: None, db, compacted: 0 }))))
    }

    // The file another process writes at `path`, to read once its layout is the one `migrations`
    // bring it to.
    pub fn open_reader(path: impl AsRef<Path>, migrations: Migrations) -> Db {
        let path = path.as_ref().to_path_buf();
        Db(Arc::new(Mutex::new(Inner::Reader { path, migrations })))
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
            Inner::Reader { path, migrations } => {
                let read = || -> Result<T> {
                    let db = Database::open_read_only(path)?.into_dyn();
                    let history = History::new(&db)?;
                    history.check_version(migrations)?;
                    read(&history.latest()?)
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

#[cfg(test)]
mod tests {
    use xitdb::{Map as Typed, Set};

    use super::*;

    const NAMES: Typed<str, String> = Typed::new("names");
    const SEEN: Set<str> = Set::new("seen");

    fn path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("gem-db-{name}-{}.xitdb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    // Layout 1 has a name, layout 2 also says so, and layout 3 is the same as 2.
    fn migrations(latest: u64) -> Migrations {
        let migrations = [
            Migration::new(1, "name", |moment: &mut MapMut| NAMES.put(moment, "a", "A")),
            Migration::new(2, "say so", |moment: &mut MapMut| moment.put("said", "yes")),
            Migration::new(3, "nothing", |_: &mut MapMut| Ok(())),
        ];
        Migrations::new(migrations.into_iter().filter(|m| m.version <= latest)).unwrap()
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
            NAMES.put(moment, "k", "K")?;
            SEEN.insert(moment, "x")?;
            // A write reads its own changes.
            assert_eq!(moment.get_string("name", None)?.as_deref(), Some("a"));
            Ok(())
        })
        .unwrap();
        db.read(|moment| {
            assert_eq!(moment.get_string("name", None)?.as_deref(), Some("a"));
            assert_eq!(NAMES.get(moment, "k")?.as_deref(), Some("K"));
            assert!(SEEN.contains(moment, "x")?);
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
    fn migrates_once_and_compacts_when_reopened() {
        let path = path("reopen");
        {
            let (db, migrated) = Db::open(&path, &migrations(1)).unwrap();
            assert_eq!(migrated.ran, ["name"]);
            for i in 0..100 {
                db.write(|moment| moment.put("n", &*i.to_string())).unwrap();
            }
        }
        let before = std::fs::metadata(&path).unwrap().len();
        let (db, migrated) = Db::open(&path, &migrations(2)).unwrap();
        // Only the migration it hadn't run yet.
        assert_eq!(migrated.ran, ["say so"]);
        assert!(std::fs::metadata(&path).unwrap().len() < before);
        assert_eq!(get(&db, "n").as_deref(), Some("99"));
        assert_eq!(get(&db, "said").as_deref(), Some("yes"));
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn another_process_reads_each_write_and_compaction() {
        let path = path("reader");
        let (db, _) = Db::open(&path, &migrations(2)).unwrap();
        let reader = Db::open_reader(&path, migrations(2));
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
    fn a_reader_refuses_another_layout() {
        let path = path("layout");
        let (db, _) = Db::open(&path, &migrations(2)).unwrap();
        // A build from before layout 2 doesn't read it, nor does one from after it before the
        // server migrated.
        let older = Db::open_reader(&path, migrations(1));
        assert!(older.read(|moment| moment.get_string("said", None)).is_err());
        let newer = Db::open_reader(&path, migrations(3));
        assert!(newer.read(|moment| moment.get_string("said", None)).is_err());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reader_of_no_file_fails() {
        let reader = Db::open_reader(path("missing"), migrations(2));
        assert!(reader.read(|moment| moment.get_string("snapshot", None)).is_err());
    }
}
