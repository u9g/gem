// The server runs from copies in releases/, so cargo can replace its output while they run, and a
// build from another checkout sharing the target directory never swaps the binary underneath.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::log;

const RELEASES: &str = "releases";
// Agents are handed the path of the build that started them, so older builds stay a while.
const KEPT_RELEASES: usize = 10;

// Windows cannot replace a running executable but can rename one, so a launcher started from
// cargo's output moves itself into releases/ for cargo to build over. Returns where it now is.
pub fn move_out_of_target(exe: &Path) -> PathBuf {
    if !cfg!(windows) || is_release(exe) {
        return exe.to_path_buf();
    }
    let release = new_release();
    match fs::rename(exe, &release) {
        Ok(()) => release,
        Err(error) => {
            log(&format!(
                "cannot move {} into {RELEASES}/, so builds fail while the launcher runs: {error}",
                exe.display()
            ));
            exe.to_path_buf()
        }
    }
}

// Copies a build into releases/.
pub fn install(build: &Path) -> PathBuf {
    let release = new_release();
    fs::copy(build, &release).expect("cannot copy the build into releases/");
    release
}

// Deletes all but the newest releases, never one in `keep`. One still running, on Windows, stays
// until a later prune.
pub fn prune(keep: &[&Path]) {
    let Ok(entries) = fs::read_dir(RELEASES) else {
        return;
    };
    let keep: Vec<PathBuf> = keep
        .iter()
        .filter_map(|p| fs::canonicalize(p).ok())
        .collect();
    let mut releases: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    releases.sort();
    let old = releases.len().saturating_sub(KEPT_RELEASES);
    for release in &releases[..old] {
        if !fs::canonicalize(release).is_ok_and(|release| keep.contains(&release)) {
            let _ = fs::remove_file(release);
        }
    }
}

fn is_release(exe: &Path) -> bool {
    let releases = fs::canonicalize(RELEASES).ok();
    releases.is_some() && exe.parent().and_then(|dir| fs::canonicalize(dir).ok()) == releases
}

// A new path in releases/, named so the newest sorts last.
fn new_release() -> PathBuf {
    fs::create_dir_all(RELEASES).unwrap();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    fs::canonicalize(RELEASES)
        .unwrap()
        .join(format!("server-{millis}{}", env::consts::EXE_SUFFIX))
}
