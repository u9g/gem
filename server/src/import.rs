// Moves the state servers kept in their working directory before they kept it in the database:
// in files, and then in a SQLite database. Each goes once imported, so a later start imports
// nothing.

use std::path::Path;

use gem_db::{Db, Read, Tx};
use serde_json::Value;

// The SQLite database, and its journal, kept once imported in case the import missed something.
const SQLITE: &str = "gem.sqlite";
const SQLITE_JOURNAL: &str = "gem.sqlite-journal";

pub fn import(db: &Db) -> Result<(), String> {
    if db.import_sqlite(SQLITE)? {
        for file in [SQLITE, SQLITE_JOURNAL] {
            if let Err(error) = std::fs::rename(file, format!("{file}.imported"))
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!("cannot move {file} aside once imported: {error}"));
            }
        }
    }
    let json = |path: &str| -> Option<Value> {
        let value = serde_json::from_slice(&std::fs::read(path).ok()?).ok();
        if value.is_none() {
            tracing::warn!(path, "cannot import: not JSON");
        }
        value
    };
    if let Some(Value::Object(names)) = json("cluster-names.json") {
        db.write(|tx| {
            for (key, name) in &names {
                if let Some(name) = name.as_str() {
                    put_new(tx, "cluster_names", key, name)?;
                }
            }
            Ok(())
        })?;
        remove("cluster-names.json");
    }
    if let Some(Value::Array(ids)) = json("handled-comments.json") {
        db.write(|tx| {
            for id in ids.iter().filter_map(Value::as_str) {
                tx.insert("started_conversations", id)?;
            }
            Ok(())
        })?;
        remove("handled-comments.json");
    }
    if let Some(Value::Object(reasons)) = json("no-proposal-reasons.json") {
        db.write(|tx| {
            for (id, reason) in &reasons {
                if let Some(reason) = reason.as_str() {
                    put_new(tx, "no_proposal_reasons", id, reason)?;
                }
            }
            Ok(())
        })?;
        remove("no-proposal-reasons.json");
    }
    import_dir(db, "proposals", |tx, id, json| put_new(tx, "proposals", id, json))?;
    for (dir, closed) in [("investigations", false), ("past-investigations", true)] {
        import_dir(db, dir, |tx, id, json| {
            if tx.contains("investigations", id)? {
                return Ok(());
            }
            let fields = [("investigation", json.into()), ("closed", closed.into())];
            tx.put_row("investigations", id, &fields)
        })?;
    }
    // Each token's --mcp-config stays a file, beside where its subject was.
    let tokens = std::fs::read_dir(crate::tokens::DIR).into_iter().flatten();
    for entry in tokens.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(token) = name.strip_suffix(".json").filter(|t| !t.ends_with(".mcp")) else {
            continue;
        };
        let Ok(subject) = std::fs::read_to_string(&path) else {
            continue;
        };
        db.write(|tx| put_new(tx, "agent_tokens", token, &subject))?;
        remove(&path);
    }
    // The server saves a new snapshot within seconds of starting.
    remove("clusters.json");
    remove("clusters.json.partial");
    Ok(())
}

// Imports each `<id>.json` file in `dir`, then removes the directory.
fn import_dir(
    db: &Db,
    dir: &str,
    insert: impl Fn(&mut Tx, &str, &str) -> gem_db::Result<()>,
) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
            continue;
        };
        let Ok(json) = std::fs::read_to_string(&path) else {
            continue;
        };
        db.write(|tx| insert(tx, id, &json))?;
        remove(&path);
    }
    if let Err(error) = std::fs::remove_dir(dir) {
        tracing::warn!(dir, %error, "cannot remove imported directory");
    }
    Ok(())
}

// Sets the row `key` of `table` to `text`, unless it has one.
fn put_new(tx: &mut Tx, table: &str, key: &str, text: &str) -> gem_db::Result<()> {
    if !tx.contains(table, key)? {
        tx.put(table, key, text)?;
    }
    Ok(())
}

fn remove(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "cannot remove imported file");
    }
}

