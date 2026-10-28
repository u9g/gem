// The server runs from copies in releases/, so cargo can replace its output while they run, and a
// build from another checkout sharing the target directory never swaps the binary underneath.
// Each release is a directory holding the server and the gem-native helper it runs, so a server
// always runs the helper built with it, and finds it beside itself.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use gem_native::EXE_SUFFIX;

use crate::log;

const RELEASES: &str = "releases";
// Agents are handed the path of the build that started them, so older builds stay a while.
const KEPT_RELEASES: usize = 10;

// The server package's two binaries, from one build.
pub struct Build {
    pub server: PathBuf,
    pub native: PathBuf,
}

impl Build {
    // The server at `server`, with the gem-native cargo builds beside it.
    pub fn beside(server: &Path) -> Build {
        Build {
            server: server.to_path_buf(),
            native: server.with_file_name(native_file()),
        }
    }
}

// Windows cannot replace a running executable but can rename one, so a launcher started from
// cargo's output moves itself into releases/ for cargo to build over, with a copy of the gem-native
// beside it. Returns where it now is.
pub fn move_out_of_target(exe: &Path) -> PathBuf {
    if !cfg!(windows) || is_release(exe) {
        return exe.to_path_buf();
    }
    let release = new_release();
    let moved = release.join(server_file());
    match fs::rename(exe, &moved) {
        Ok(()) => {
            let _ = copy(&Build::beside(exe).native, &release.join(native_file()));
            moved
        }
        Err(error) => {
            log(&format!(
                "cannot move {} into {RELEASES}/, so builds fail while the launcher runs: {error}",
                exe.display()
            ));
            let _ = fs::remove_dir(&release);
            exe.to_path_buf()
        }
    }
}

// Copies a build into releases/. A build without gem-native, e.g. one left from before the
// server had it, is installed without it.
pub fn install(build: &Build) -> Build {
    let release = new_release();
    let installed = Build {
        server: release.join(server_file()),
        native: release.join(native_file()),
    };
    copy(&build.server, &installed.server).expect("cannot copy the build into releases/");
    if let Err(error) = copy(&build.native, &installed.native) {
        log(&format!(
            "cannot copy {} into {RELEASES}/: {error}",
            build.native.display()
        ));
    }
    installed
}

// Deletes all but the newest releases, never one in `keep`. One still running, on Windows, stays
// until a later prune.
pub fn prune(keep: &[&Path]) {
    let Ok(entries) = fs::read_dir(RELEASES) else {
        return;
    };
    // A release is the directory holding the server, or the server itself in releases/ as the
    // launcher kept them before gem-native.
    let keep: Vec<PathBuf> = (keep.iter())
        .filter_map(|p| fs::canonicalize(p).ok())
        .flat_map(|p| [p.parent().map(Path::to_path_buf), Some(p)])
        .flatten()
        .collect();
    let mut releases: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    releases.sort_by_key(|release| release.to_string_lossy().into_owned());
    let old = releases.len().saturating_sub(KEPT_RELEASES);
    for release in &releases[..old] {
        if !fs::canonicalize(release).is_ok_and(|release| keep.contains(&release)) {
            remove(release);
        }
    }
}

// The server goes first, so a release whose server is still running keeps its gem-native.
fn remove(release: &Path) {
    if !release.is_dir() {
        let _ = fs::remove_file(release);
    } else if fs::remove_file(release.join(server_file())).is_ok() {
        let _ = fs::remove_dir_all(release);
    }
}

fn is_release(exe: &Path) -> bool {
    let Ok(releases) = fs::canonicalize(RELEASES) else {
        return false;
    };
    let dir = exe.parent().and_then(|dir| fs::canonicalize(dir).ok());
    let parent = dir.as_deref().and_then(Path::parent);
    dir.as_deref() == Some(releases.as_path()) || parent == Some(releases.as_path())
}

// A new directory in releases/, named so the newest sorts last.
fn new_release() -> PathBuf {
    fs::create_dir_all(RELEASES).unwrap();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let release = fs::canonicalize(RELEASES)
        .unwrap()
        .join(format!("server-{millis}"));
    fs::create_dir_all(&release).unwrap();
    release
}

fn server_file() -> String {
    format!("server{EXE_SUFFIX}")
}

pub fn native_file() -> String {
    format!("gem-native{EXE_SUFFIX}")
}

// `fs::copy`, which lust's std does not have yet: the bytes, then the permissions, so a copied
// binary stays executable.
fn copy(from: &Path, to: &Path) -> io::Result<()> {
    fs::write(to, fs::read(from)?)?;
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}
