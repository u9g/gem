// One launcher per directory: they would run servers into the same room and state.
//
// The lock is the OS's lock on launcher.lock, which goes when its holder exits however it exits.
// std's File::try_lock is not in lust's std, so gem-native takes it, as `gem-native lock`, and
// holds it until the launcher closes its stdin by exiting.

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
};

// Held until the launcher exits.
pub struct Lock {
    _holder: Child,
}

// `native` is the gem-native to take it with.
pub fn acquire(native: &Path) -> Lock {
    let mut holder = Command::new(native)
        .args(["lock", "launcher.lock"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("cannot run {}: {error}", native.display()));
    let mut answer = String::new();
    let _ = BufReader::new(holder.stdout.take().unwrap()).read_line(&mut answer);
    if answer.trim() != "locked" {
        eprintln!("another launcher is running in this directory");
        std::process::exit(1);
    }
    Lock { _holder: holder }
}
