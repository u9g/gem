// Moves the state servers kept in files, in their working directory, before they kept it in the
// database. Each file goes once imported, so a later start imports nothing.

use std::path::Path;

use gem_db::{
    Db,
    rusqlite::{Connection, params},
};
use serde_json::Value;

pub fn import(db: &Db) -> Result<(), String> {
    let json = |path: &str| -> Option<Value> {
        let value = serde_json::from_slice(&std::fs::read(path).ok()?).ok();
        if value.is_none() {
            tracing::warn!(path, "cannot import: not JSON");
        }
        value
    };
    if let Some(Value::Object(names)) = json("cluster-names.json") {
        db.with(|db| {
            for (key, name) in &names {
                db.execute(
                    "INSERT OR IGNORE INTO cluster_names (key, name) VALUES (?1, ?2)",
                    params![key, name.as_str()],
                )?;
            }
            Ok(())
        })?;
        remove("cluster-names.json");
    }
    if let Some(Value::Array(ids)) = json("handled-comments.json") {
        db.with(|db| {
            for id in ids.iter().filter_map(Value::as_str) {
                db.execute(
                    "INSERT OR IGNORE INTO started_conversations (id) VALUES (?1)",
                    [id],
                )?;
            }
            Ok(())
        })?;
        remove("handled-comments.json");
    }
    if let Some(Value::Object(reasons)) = json("no-proposal-reasons.json") {
        db.with(|db| {
            for (id, reason) in &reasons {
                db.execute(
                    "INSERT OR IGNORE INTO no_proposal_reasons (id, reason) VALUES (?1, ?2)",
                    params![id, reason.as_str()],
                )?;
            }
            Ok(())
        })?;
        remove("no-proposal-reasons.json");
    }
    import_dir(db, "proposals", |db, id, json| {
        db.execute(
            "INSERT OR IGNORE INTO proposals (id, proposal) VALUES (?1, ?2)",
            params![id, json],
        )
    })?;
    for (dir, closed) in [("investigations", false), ("past-investigations", true)] {
        import_dir(db, dir, |db, id, json| {
            db.execute(
                "INSERT OR IGNORE INTO investigations (id, investigation, closed) \
                 VALUES (?1, ?2, ?3)",
                params![id, json, closed],
            )
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
        db.with(|db| {
            db.execute(
                "INSERT OR IGNORE INTO agent_tokens (token, subject) VALUES (?1, ?2)",
                params![token, subject],
            )
        })?;
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
    insert: impl Fn(&Connection, &str, &str) -> gem_db::rusqlite::Result<usize>,
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
        db.with(|db| insert(db, id, &json))?;
        remove(&path);
    }
    if let Err(error) = std::fs::remove_dir(dir) {
        tracing::warn!(dir, %error, "cannot remove imported directory");
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

