// gem-native: what the server needs that lust cannot compile, as a helper the server runs and talks
// to over stdin and stdout, one line of JSON a message. Built by Cargo only (the `native` feature).
//
//     gem-native sqlite <path>   a SQLite database (see crates/db/src/sqlite.rs)
//     gem-native room            the server's place in the LiveKit room (see crates/room)
//
// Each exits when its stdin closes, so it never outlives the server that started it.

mod room;
mod sqlite;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["sqlite", path] => sqlite::serve(path),
        ["room"] => room::serve(),
        _ => {
            eprintln!("usage: gem-native sqlite <path> | gem-native room");
            std::process::exit(2);
        }
    }
}
