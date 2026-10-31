// The versions of the layout of gem's state in its database, each made by a migration from the one
// before. The server migrates the file when it opens it; the `gem` MCP server, reading it from
// another process, reads only the layout this build knows.
//
// A migration never changes once it has shipped; a change to the layout is a new one, with the
// next version.

use gem_db::{MapMut, Migration, Migrations};

pub fn migrations() -> Migrations {
    up_to(u64::MAX)
}

// The migrations up to layout `latest`.
fn up_to(latest: u64) -> Migrations {
    let index_by_thread = |moment: &mut MapMut| {
        gem_merges::rebuild_index(moment)?;
        gem_rebases::rebuild_index(moment).map(drop)
    };
    // The clusters as gem last showed them, as JSON, which `server mcp` read before it read
    // view.xitdb.
    let drop_snapshot = |moment: &mut MapMut| moment.remove("snapshot").map(drop);
    let migrations = [
        Migration::new(1, "index merges and rebases by thread", index_by_thread),
        Migration::new(2, "drop the clusters' snapshot", drop_snapshot),
    ];
    let migrations = migrations.into_iter().filter(|migration| migration.version <= latest);
    Migrations::new(migrations).expect("migrations are in order")
}

#[cfg(test)]
mod tests {
    use std::{future::pending, path::Path};

    use gem_merges::{Merges, Progress};
    use gem_rebases::{Rebases, Refs};

    use super::*;

    struct NoHost;

    impl gem_merges::Host for NoHost {
        async fn start_thread(&self, _: &str, _: &str) -> Result<String, String> {
            pending().await
        }
        async fn progress(&self, _: &str) -> Option<Progress> {
            pending().await
        }
    }

    impl gem_rebases::Host for NoHost {
        async fn refs(&self, _: &str) -> Option<Refs> {
            pending().await
        }
        async fn start_thread(&self, _: &str, _: &str) -> Result<String, String> {
            pending().await
        }
        async fn progress(&self, _: &str) -> Option<gem_rebases::Progress> {
            pending().await
        }
        async fn listed_head(&self, _: &str) -> Option<String> {
            pending().await
        }
    }

    // A gem.sqlite from the last server that kept it, imported and migrated as a server start does,
    // reads through the merges' and rebases' records and their thread indexes.
    #[test]
    fn an_imported_sqlite_file_migrates_to_this_layout() {
        let dir = crate::tokens::tests::temp_dir("layout");
        let (sqlite, xitdb) = (dir.join("gem.sqlite"), dir.join(gem_db::FILE));
        let rows = "
            INSERT INTO merges (url, thread_id, question, title, started_at)
                VALUES ('https://github.com/o/r/pull/1', 'm', 1, 'Merge r#1', 7);
            INSERT INTO rebases (url, head, thread_id, pushed, title, started_at)
                VALUES ('https://github.com/o/r/pull/2', 'abc', 'r', 1, 'Rebase r#2', 8);";
        let sql = format!("{}{rows}", gem_sqlite_import::SCHEMA);
        rusqlite::Connection::open(&sqlite).unwrap().execute_batch(&sql).unwrap();
        gem_sqlite_import::import(&sqlite, &xitdb).unwrap();

        let (db, migrated) = gem_db::Db::open(&xitdb, &migrations()).unwrap();
        assert_eq!((migrated.from, migrated.to), (0, 2));
        let merges = Merges::new(db.clone(), NoHost).unwrap();
        let merge = &merges.all().unwrap()["https://github.com/o/r/pull/1"];
        assert_eq!(merge.thread_id.as_deref(), Some("m"));
        let fields = (merge.title.as_str(), merge.started_at, merge.question);
        assert_eq!(fields, ("Merge r#1", 7, true));
        let rebases = Rebases::new(db.clone(), NoHost).unwrap();
        let rebase = &rebases.all().unwrap()["https://github.com/o/r/pull/2"];
        assert_eq!(rebase.thread_id.as_deref(), Some("r"));
        assert!(rebase.pushed && !rebase.question);
        // Forgetting a thread finds its entries through the index the migration built.
        merges.forget("m").unwrap();
        rebases.forget("r").unwrap();
        assert!(merges.all().unwrap().is_empty() && rebases.all().unwrap().is_empty());

        // A reader of this build reads the migrated file.
        drop(db);
        let reader = gem_db::Db::open_reader(&xitdb, migrations());
        assert!(reader.read(|moment| moment.get_string("snapshot", None)).is_ok());
        let _ = std::fs::remove_dir_all(Path::new(&dir));
    }

    // A server from before view.xitdb kept the clusters' JSON in gem.xitdb, which layout 2 drops.
    #[test]
    fn layout_2_drops_the_snapshot() {
        let dir = crate::tokens::tests::temp_dir("snapshot");
        let path = dir.join(gem_db::FILE);
        let (db, _) = gem_db::Db::open(&path, &up_to(1)).unwrap();
        db.write(|moment| moment.put("snapshot", r#"{"clusters":[]}"#)).unwrap();
        drop(db);
        let (db, migrated) = gem_db::Db::open(&path, &migrations()).unwrap();
        assert_eq!(migrated.ran, ["drop the clusters' snapshot"]);
        assert_eq!(db.read(|moment| moment.get_string("snapshot", None)).unwrap(), None);
        drop(db);
        let _ = std::fs::remove_dir_all(Path::new(&dir));
    }
}
