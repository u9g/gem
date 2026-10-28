// The server, as the launcher's child process.

use std::{
    env,
    ffi::OsString,
    process::{Child, Command},
};
#[cfg(unix)]
use std::{
    thread::sleep,
    time::{Duration, Instant},
};

use crate::{log, releases::Build, update::git};

// How long a stopped server gets to leave its room before it is killed.
#[cfg(unix)]
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

pub fn start(release: &Build, dotenv: &[OsString]) -> Child {
    match git(&["rev-parse", "--short", "HEAD"]) {
        Some(head) => log(&format!("starting the server at {head}")),
        None => log("starting the server"),
    }
    let mut command = Command::new(&release.server);
    command.arg("serve");
    // What the launcher read from 1Password, which the server would otherwise ask for again.
    gem_env::apply(&mut command);
    // The server reads these from .env itself, so it sees an edit made since the launcher started.
    for name in dotenv {
        command.env_remove(name);
    }
    // The server finds T3 Code's CLI and scratch threads under $HOME, which Windows only has in
    // shells like Git Bash.
    if env::var_os("HOME").is_none()
        && let Some(profile) = env::var_os("USERPROFILE")
    {
        command.env("HOME", profile);
    }
    // The server runs the gem-native built with it. One installed without it finds its own.
    if release.native.is_file() {
        command.env("GEM_NATIVE", &release.native);
    }
    command.spawn().expect("cannot start the server")
}

pub fn stop(mut server: Child) {
    // SIGTERM lets the server leave its room, so frontends see it go offline at once. Windows
    // has no signal to send another process, so it is killed.
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &server.id().to_string()])
            .status();
        let deadline = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = server.try_wait() {
                return;
            }
            sleep(Duration::from_millis(200));
        }
    }
    let _ = server.kill();
    let _ = server.wait();
}
