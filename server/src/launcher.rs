// What running the binary without a command starts: it runs the server as a child process, starts
// it again whenever it exits, and keeps it on the newest commit of main. Every minute it fetches
// origin/main and, when main has moved, fast-forwards, builds, and restarts onto the new build,
// launcher included. It updates only a checkout on main that can fast-forward; a failed build
// keeps the running one.
//
// Run it from server/, where the .env and state live, e.g. with `cargo run --release` or at
// logon from a scheduled task or a launchd agent. `server serve` runs the server alone.
//
// Builds run from copies in releases/, so cargo can replace its output while they run, and a
// build from another checkout sharing the target directory never swaps the binary underneath.

use std::{
    env,
    fs::{self, File},
    io::BufRead,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const RESTART_DELAY: Duration = Duration::from_secs(10);
// How long a stopped server gets to leave its room before it is killed.
#[cfg(unix)]
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASES: &str = "releases";
// Agents are handed the path of the build that started them, so older builds stay a while.
const KEPT_RELEASES: usize = 10;

pub fn run() -> ! {
    let lock = lock();
    let mut launcher = Launcher {
        lock: Some(lock),
        // Outside a checkout, e.g. an installed binary, there is nothing to update from.
        checkout: Path::new("Cargo.toml").exists(),
        failed: None,
    };
    let exe = env::current_exe().unwrap();
    if launcher.checkout {
        if !is_release(&exe) {
            log(&format!("running from a copy of {}", exe.display()));
            launcher.hand_off(&install(&exe));
        }
        prune(&exe);
        // Commits can land while the launcher is down, and the checkout can be pulled by hand,
        // so catch up before the first start. A build with nothing new to compile takes seconds.
        let _ = launcher.fast_forward();
        if let Some(build) = launcher.build()
            && fs::read(&build).ok() != fs::read(&exe).ok()
        {
            launcher.hand_off(&install(&build));
        }
    }

    let mut server = start(&exe);
    let mut next_check = Instant::now() + CHECK_INTERVAL;
    loop {
        sleep(Duration::from_secs(2));
        if let Ok(Some(status)) = server.try_wait() {
            log(&format!(
                "server exited ({status}); restarting in {RESTART_DELAY:?}"
            ));
            sleep(RESTART_DELAY);
            server = start(&exe);
        }
        if launcher.checkout && Instant::now() >= next_check {
            next_check = Instant::now() + CHECK_INTERVAL;
            if let Some(build) = launcher.update() {
                log("restarting onto the new build");
                stop(server);
                launcher.hand_off(&install(&build));
            }
        }
    }
}

struct Launcher {
    lock: Option<File>,
    checkout: bool,
    // The origin/main commit that last failed to fast-forward or build, so it is not retried
    // every minute.
    failed: Option<String>,
}

impl Launcher {
    // Builds origin/main when it has moved. Returns the build to restart onto.
    fn update(&mut self) -> Option<PathBuf> {
        if !self.fast_forward()? {
            return None;
        }
        self.build()
    }

    // Returns whether HEAD moved.
    fn fast_forward(&mut self) -> Option<bool> {
        if git(&["rev-parse", "--abbrev-ref", "HEAD"])? != "main" {
            return Some(false);
        }
        git(&["fetch", "--quiet", "origin", "main"])?;
        let head = git(&["rev-parse", "HEAD"])?;
        let latest = git(&["rev-parse", "origin/main"])?;
        if head == latest || self.failed.as_ref() == Some(&latest) {
            return Some(false);
        }
        log(&format!("updating {} to {}", &head[..7], &latest[..7]));
        if git(&["merge", "--ff-only", "--quiet", "origin/main"]).is_none() {
            log("cannot fast-forward main; staying on the running build");
            self.failed = Some(latest);
            return Some(false);
        }
        Some(true)
    }

    // Builds the checkout and returns the path cargo built the binary at.
    fn build(&mut self) -> Option<PathBuf> {
        log(&format!(
            "building {}",
            git(&["rev-parse", "--short", "HEAD"])?
        ));
        // Diagnostics still render to stderr; stdout carries the messages naming the binary.
        let mut cargo = Command::new("cargo")
            .args([
                "build",
                "--release",
                "--message-format=json-render-diagnostics",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut executable = None;
        for line in std::io::BufReader::new(cargo.stdout.take().unwrap()).lines() {
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line.unwrap_or_default())
            else {
                continue;
            };
            if let Some(path) = message["executable"].as_str() {
                executable = Some(PathBuf::from(path));
            }
        }
        if !cargo.wait().is_ok_and(|status| status.success()) || executable.is_none() {
            log("build failed; staying on the running build");
            self.failed = git(&["rev-parse", "origin/main"]);
            return None;
        }
        executable
    }

    // Replaces this process with `exe`, which takes the lock once this process lets it go.
    fn hand_off(&mut self, exe: &Path) -> ! {
        drop(self.lock.take());
        let mut command = Command::new(exe);
        command.args(env::args_os().skip(1));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            panic!("cannot run {}: {}", exe.display(), command.exec());
        }
        // Windows has no exec, so the new launcher runs on after this one exits. It inherits the
        // console and output this one was given.
        #[cfg(windows)]
        {
            command.spawn().expect("cannot run the new build");
            std::process::exit(0);
        }
    }
}

// One launcher per directory: they would run servers into the same room and state.
fn lock() -> File {
    let file = File::create("launcher.lock").expect("cannot create launcher.lock");
    // A launcher handing off lets go just before its successor starts.
    for _ in 0..30 {
        if file.try_lock().is_ok() {
            return file;
        }
        sleep(Duration::from_secs(1));
    }
    eprintln!("another launcher is running in this directory");
    std::process::exit(1);
}

fn start(exe: &Path) -> Child {
    match git(&["rev-parse", "--short", "HEAD"]) {
        Some(head) => log(&format!("starting the server at {head}")),
        None => log("starting the server"),
    }
    let mut command = Command::new(exe);
    command.arg("serve");
    // The server finds T3 Code's CLI and scratch threads under $HOME, which Windows only has in
    // shells like Git Bash.
    if env::var_os("HOME").is_none()
        && let Some(profile) = env::var_os("USERPROFILE")
    {
        command.env("HOME", profile);
    }
    command.spawn().expect("cannot start the server")
}

fn stop(mut server: Child) {
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

fn is_release(exe: &Path) -> bool {
    let releases = fs::canonicalize(RELEASES).ok();
    releases.is_some() && exe.parent().and_then(|dir| fs::canonicalize(dir).ok()) == releases
}

// Copies a build into releases/, named so the newest sorts last.
fn install(build: &Path) -> PathBuf {
    fs::create_dir_all(RELEASES).unwrap();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let release = fs::canonicalize(RELEASES)
        .unwrap()
        .join(format!("server-{millis}{}", env::consts::EXE_SUFFIX));
    fs::copy(build, &release).expect("cannot copy the build into releases/");
    release
}

// Deletes all but the newest releases. One still running, on Windows, stays until a later prune.
fn prune(exe: &Path) {
    let Ok(entries) = fs::read_dir(RELEASES) else {
        return;
    };
    let mut releases: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    releases.sort();
    let old = releases.len().saturating_sub(KEPT_RELEASES);
    for release in &releases[..old] {
        if fs::canonicalize(release).ok() != fs::canonicalize(exe).ok() {
            let _ = fs::remove_file(release);
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn log(message: &str) {
    eprintln!("launcher: {message}");
}
