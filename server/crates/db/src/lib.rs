// The server's own database, an xitdb file whose root is a `History` of moments: each write
// appends a copy of the last moment with its changes, so a reader, even in another process, sees
// each write whole or not at all. A moment is a hash map; each part of the server keeps its state
// under its own key there, in typed collections of xitdb's data structures.
//
// The layout of a moment has a version, which the server brings up to date with its `Migrations`
// when it opens the file, and which readers in other processes check before they read.
//
// The file only grows, so it is compacted to its last moment when it opens and whenever it has
// grown well past its size then. Compacting replaces the file, so one process writes it: the server
// holds a lock on a file beside it for as long as it has it open.
//
// A file can also be replicated: each write's bytes are all a copy that has the writes before
// needs, since the file only grows, and a copy of another file, or of this one before it was
// compacted, is sent all of it.

use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

pub use xitdb;
use xitdb::{
    CoreBufferedFile, Database, DynCore, History, MemoryOffsetMap, ReadHashMap, WriteHashMap,
};
pub use xitdb::{Migrated, Migration, ReplicaState};
pub use xitdb::{Error, Result};

// The server's database, in its working directory, with its state.
pub const FILE: &str = "gem.xitdb";

// How much gem.xitdb grows past twice its compacted size before it is compacted again.
const GROWTH: i64 = 16 << 20;

pub type Map<'a> = ReadHashMap<'a, DynCore>;
pub type MapMut<'a> = WriteHashMap<'a, DynCore>;
pub type Migrations = xitdb::Migrations<DynCore>;

#[derive(Clone)]
pub struct Db(Arc<Mutex<Inner>>);

// The committed bytes from `start` to `end` of the file with `file_id`, for a replica whose
// committed size is `start`, or for any replica, which they replace, when `start` is 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub file_id: u64,
    pub start: i64,
    pub end: i64,
    pub bytes: Vec<u8>,
}

enum Inner {
    // This process writes it: in the file at `path`, or in memory when None.
    Writer {
        path: Option<PathBuf>,
        db: Database<DynCore>,
        // The file's size when it was last compacted, or last failed to be.
        compacted: i64,
        // How much it grows past twice that before it is compacted again.
        growth: i64,
        // Held for as long as the file is open: the lock on `<path>.lock`.
        _lock: Option<File>,
    },
    // Another process writes the file at the path. It is opened again for each read, so a read
    // sees the file even after the writer replaced it with a compacted one, and it checks the
    // layout is the one `migrations` bring it to.
    Reader { path: PathBuf, migrations: Migrations },
}

impl Db {
    // Opens the file at `path`, or makes it, and brings its layout up to date with `migrations`.
    pub fn open(path: impl AsRef<Path>, migrations: &Migrations) -> Result<(Db, Migrated), String> {
        Db::open_growing(path, migrations, GROWTH)
    }

    // Like `open`, compacting the file once it grows `growth` bytes past twice its compacted size.
    pub fn open_growing(
        path: impl AsRef<Path>,
        migrations: &Migrations,
        growth: i64,
    ) -> Result<(Db, Migrated), String> {
        let path = path.as_ref().to_path_buf();
        let mut lock_path = OsString::from(&path);
        lock_path.push(".lock");
        let lock = File::create(&lock_path).map_err(|error| format!("{lock_path:?}: {error}"))?;
        if lock.try_lock().is_err() {
            return Err(format!("{}: another server has it open", path.display()));
        }
        let start = || -> Result<(Database<DynCore>, Migrated)> {
            let db = Database::open(&path)?.into_dyn();
            let migrated = History::new(&db)?.migrate(migrations)?;
            Ok((db, migrated))
        };
        let (mut db, migrated) = start().map_err(|error| describe(&path, error))?;
        // The file is whole either way, so a server starts on it even when it can't compact it.
        compact_or_log(&path, &mut db);
        let compacted = db.committed_size().map_err(|error| describe(&path, error))?;
        let (path, _lock) = (Some(path), Some(lock));
        let inner = Inner::Writer { path, db, compacted, growth, _lock };
        Ok((Db(Arc::new(Mutex::new(inner))), migrated))
    }

    pub fn open_in_memory() -> Result<Db, String> {
        let db = Database::in_memory().map(Database::into_dyn);
        let db = db.map_err(|error| format!("gem's database: {error}"))?;
        let inner = Inner::Writer { path: None, db, compacted: 0, growth: GROWTH, _lock: None };
        Ok(Db(Arc::new(Mutex::new(inner))))
    }

    // A panic in a read or a write leaves the file whole, since xitdb discards the transaction it
    // was in, so the database stays usable.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
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
        let inner = self.lock();
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
        self.transact(write, false).map(|(written, _)| written)
    }

    // Like `write`, and also returns the bytes the moment appended, or None when it appended
    // none. They are read before the file is compacted, so a replica that applies them has the
    // moment either way.
    pub fn write_replicated<T>(
        &self,
        write: impl FnOnce(&mut MapMut) -> Result<T>,
    ) -> Result<(T, Option<Delta>), String> {
        self.transact(write, true)
    }

    fn transact<T>(
        &self,
        write: impl FnOnce(&mut MapMut) -> Result<T>,
        replicated: bool,
    ) -> Result<(T, Option<Delta>), String> {
        let mut inner = self.lock();
        let Inner::Writer { path, db, compacted, growth, .. } = &mut *inner else {
            return Err("gem's database: only the server writes it".into());
        };
        let describe = |error| describe_at(path.as_deref(), error);
        let start = db.committed_size().map_err(describe)?;
        let written = History::new(db).and_then(|history| history.transact(write));
        let written = written.map_err(describe)?;
        let end = db.committed_size().map_err(describe)?;
        let delta = match replicated && end > start {
            true => {
                let mut bytes = Vec::with_capacity((end - start) as usize);
                db.read_committed(start, end, &mut bytes).map_err(describe)?;
                Some(Delta { file_id: db.header().file_id, start, end, bytes })
            }
            false => None,
        };
        if let Some(path) = path
            && end > 2 * *compacted + *growth
        {
            compact_or_log(path, db);
            // After a failure too, so the next try waits for the file to grow again.
            *compacted = db.committed_size().unwrap_or(end);
        }
        Ok((written, delta))
    }

    // The bytes a replica in `replica` lacks: from its committed size when it is a copy of this
    // file up to there, and otherwise, or for no replica, all of it.
    pub fn replicate(&self, replica: Option<ReplicaState>) -> Result<Delta, String> {
        let inner = self.lock();
        let Inner::Writer { path, db, .. } = &*inner else {
            return Err("gem's database: only the server replicates it".into());
        };
        let read = || {
            let start = match replica {
                Some(replica) => db.replication_start(replica)?,
                None => 0,
            };
            let end = db.committed_size()?;
            let mut bytes = Vec::with_capacity((end - start) as usize);
            db.read_committed(start, end, &mut bytes)?;
            Ok(Delta { file_id: db.header().file_id, start, end, bytes })
        };
        read().map_err(|error| describe_at(path.as_deref(), error))
    }
}

// Replaces the file at `path`, which `db` writes, with one holding only its last moment, or logs
// why it could not. When it failed after `db` moved to the compacted copy, the server stops: its
// writes would go to that copy, which neither readers nor its next start see. The file at `path`
// still has every write until then.
fn compact_or_log(path: &Path, db: &mut Database<DynCore>) {
    let open = |file| Box::new(CoreBufferedFile::new(file)) as DynCore;
    match db.compact_file(path, &mut MemoryOffsetMap::new(), open) {
        Ok(()) => {}
        Err(error @ Error::CompactFileReopenFailed { .. }) => panic!("{}", describe(path, error)),
        Err(error) => eprintln!("cannot compact {}", describe(path, error)),
    }
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
            compact_or_log(&path, inner);
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
    fn one_server_writes_the_file_at_once() {
        let path = path("lock");
        let (db, _) = Db::open(&path, &migrations(1)).unwrap();
        let error = Db::open(&path, &migrations(1)).err().unwrap();
        assert!(error.contains("another server has it open"), "{error}");
        drop(db);
        Db::open(&path, &migrations(1)).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_panic_in_a_write_leaves_the_database_usable() {
        let db = Db::open_in_memory().unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.write(|moment| -> Result<()> {
                moment.put("name", "a")?;
                panic!("in the write");
            })
        }));
        assert!(panicked.is_err());
        assert_eq!(get(&db, "name"), None);
        db.write(|moment| moment.put("name", "b")).unwrap();
        assert_eq!(get(&db, "name").as_deref(), Some("b"));
    }

    // A replica as xitdb keeps one, applying what `write_replicated` and `replicate` return.
    fn apply(replica: &Database<xitdb::CoreMemory>, delta: &Delta) {
        let length = delta.end - delta.start;
        replica.apply_replicated(delta.start, &mut delta.bytes.as_slice(), length).unwrap();
    }

    #[test]
    fn a_replica_follows_each_write_and_is_replaced_after_a_compaction() {
        let path = path("replica");
        let (db, _) = Db::open_growing(&path, &migrations(1), 4096).unwrap();
        let replica = Database::in_memory().unwrap();
        let full = db.replicate(None).unwrap();
        assert_eq!(full.start, 0);
        apply(&replica, &full);
        let ((), delta) = db.write_replicated(|moment| moment.put("n", "1")).unwrap();
        let delta = delta.unwrap();
        assert_eq!((delta.file_id, delta.start), (full.file_id, full.end));
        apply(&replica, &delta);
        let state = replica.replica_state().unwrap();
        // Up to date, it lacks nothing; an unchanged write appends nothing.
        assert!(db.replicate(Some(state)).unwrap().bytes.is_empty());
        assert_eq!(db.write_replicated(|moment| moment.get_string("n", None)).unwrap().1, None);
        // Past the growth, the file is compacted, with a file ID of its own, after the bytes of
        // the write that grew it are read.
        let mut last = delta;
        for i in 0..1000 {
            let n = i.to_string();
            let ((), delta) = db.write_replicated(|moment| moment.put("n", &*n)).unwrap();
            let delta = delta.unwrap();
            assert_eq!(delta.start, last.end);
            last = delta;
            if db.replicate(None).unwrap().file_id != full.file_id {
                break;
            }
        }
        let copy = db.replicate(Some(state)).unwrap();
        assert_ne!(copy.file_id, full.file_id);
        assert_eq!(copy.start, 0);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reader_of_no_file_fails() {
        let reader = Db::open_reader(path("missing"), migrations(2));
        assert!(reader.read(|moment| moment.get_string("snapshot", None)).is_err());
    }
}
