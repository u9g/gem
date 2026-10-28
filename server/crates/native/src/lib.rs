// Finds gem-native, the helper binary that does what lust cannot compile: it joins the LiveKit
// room (WebRTC) and opens SQLite (C), and the server talks to it over its stdin and stdout, one
// line of JSON a message. Cargo builds it with the server (the `native` feature, on by default);
// lust dev builds leave it out and use the one Cargo built.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub const NAME: &str = "gem-native";

// GEM_NATIVE when set, else gem-native beside the running binary (or beside its directory, for a
// test binary in target/*/deps), else gem-native on PATH.
pub fn path() -> PathBuf {
    if let Some(path) = env::var_os("GEM_NATIVE") {
        return PathBuf::from(path);
    }
    let file = format!("{NAME}{}", env::consts::EXE_SUFFIX);
    if let Ok(exe) = env::current_exe() {
        let dirs = [exe.parent(), exe.parent().and_then(Path::parent)];
        for dir in dirs.into_iter().flatten() {
            let candidate = dir.join(&file);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from(file)
}

pub fn command(args: &[&str]) -> Command {
    let mut command = Command::new(path());
    command.args(args);
    command
}
