// gem-native: what the server needs that lust cannot compile, as a helper the server runs and talks
// to over stdin and stdout, one line of JSON a message. Built by Cargo only (the `native` feature).
//
//     gem-native sqlite <path>   a SQLite database (see crates/db/src/sqlite.rs)
//     gem-native room            the server's place in the LiveKit room (see crates/room)
//     gem-native lock <path>     the OS's lock on a file, for the launcher (see crates/launcher)
//
// Each exits when its stdin closes, so it never outlives the server that started it.

mod room;
mod sqlite;

use std::{
    fs::File,
    io::{Read, Write},
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["sqlite", path] => sqlite::serve(path),
        ["room"] => room::serve(),
        ["lock"] | ["lock", ..] if args.len() == 2 => lock(&args[1]),
        _ => {
            eprintln!("usage: gem-native sqlite <path> | gem-native room");
            std::process::exit(2);
        }
    }
}

// Prints `locked` and holds the lock on `path` until stdin closes, or prints `busy` when another
// process holds it.
fn lock(path: &str) {
    let file = File::create(path).unwrap_or_else(|error| panic!("cannot create {path}: {error}"));
    let mut stdout = std::io::stdout();
    if file.try_lock().is_err() {
        let _ = writeln!(stdout, "busy");
        return;
    }
    let _ = writeln!(stdout, "locked").and_then(|()| stdout.flush());
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
    drop(file);
}
