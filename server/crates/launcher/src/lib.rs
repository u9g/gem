// What running the binary without a command starts: a watchdog that runs the server as a child
// process, starts it again whenever it exits, and keeps it on the newest commit of main. Every
// minute it fetches origin/main and, when main has moved, fast-forwards, builds, and restarts the
// server onto the new build. It updates only a checkout on main that can fast-forward; a failed
// build keeps the running one.
//
// The launcher itself stays one process for as long as it runs, so whatever started it keeps
// watching the same process, and the environment it was started with, the LiveKit login from
// 1Password included, is handed to every server. It keeps running the build it started as: a
// change to the launcher applies once the launcher is restarted.
//
// Run it from server/, where the .env and state live, e.g. with `cargo run --release` or at
// logon from a scheduled task or a launchd agent. `server serve` runs the server alone.

mod lock;
mod releases;
mod server;
mod update;

use std::{
    env,
    path::Path,
    thread::sleep,
    time::{Duration, Instant},
};

use update::Updates;

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const RESTART_DELAY: Duration = Duration::from_secs(10);

pub fn run() -> ! {
    let _lock = lock::acquire();
    let mut exe = env::current_exe().unwrap();
    // Outside a checkout, e.g. an installed binary, there is nothing to update from.
    let mut updates = Path::new("Cargo.toml").exists().then(Updates::default);
    let mut release = exe.clone();
    if let Some(updates) = &mut updates {
        exe = releases::move_out_of_target(&exe);
        // Commits can land while the launcher is down, and the checkout can be pulled by hand,
        // so catch up before the first start. A build with nothing new to compile takes seconds.
        let _ = updates.fast_forward();
        release = releases::install(&updates.build().unwrap_or_else(|| exe.clone()));
        releases::prune(&[&exe, &release]);
    }

    let mut server = server::start(&release);
    let mut next_check = Instant::now() + CHECK_INTERVAL;
    loop {
        sleep(Duration::from_secs(2));
        if let Ok(Some(status)) = server.try_wait() {
            log(&format!(
                "server exited ({status}); restarting in {RESTART_DELAY:?}"
            ));
            sleep(RESTART_DELAY);
            server = server::start(&release);
        }
        if let Some(updates) = &mut updates
            && Instant::now() >= next_check
        {
            next_check = Instant::now() + CHECK_INTERVAL;
            if let Some(build) = updates.update() {
                log("restarting the server onto the new build");
                server::stop(server);
                release = releases::install(&build);
                releases::prune(&[&exe, &release]);
                server = server::start(&release);
            }
        }
    }
}

fn log(message: &str) {
    eprintln!("launcher: {message}");
}
