// One launcher per directory: they would run servers into the same room and state.

use std::fs::File;

// Held until the launcher exits.
pub fn acquire() -> File {
    let file = File::create("launcher.lock").expect("cannot create launcher.lock");
    if file.try_lock().is_err() {
        eprintln!("another launcher is running in this directory");
        std::process::exit(1);
    }
    file
}
