// The frontends' requests, carried over a room in the user's own LiveKit project instead of a
// port: nothing listens on the network. LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET name
// the project and GEM_ROOM the room, so several servers can share a project.
//
// The room itself is WebRTC, which lust cannot compile, so `gem-native room` joins it (see
// src/bin/native/room.rs) and passes each request to this over its stdout, one line of JSON each,
// and takes the replies and snapshots on its stdin. It is in the room only while a frontend is, as
// every connected participant costs the project's connection minutes, and leaves when the server
// exits.

use std::{
    io::{BufRead, BufReader, Write},
    process::{ChildStdin, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use gem_json::{Value, json};
use gem_log::{error, warn};

// How long to wait before running gem-native again after it exits.
const RESTART_DELAY: Duration = Duration::from_secs(2);

// The latest snapshot, which every frontend in the room is sent when it changes and when it joins.
#[derive(Clone, Default)]
pub struct Snapshots(Arc<(Mutex<(u64, String)>, Condvar)>);

impl Snapshots {
    pub fn new() -> Snapshots {
        Snapshots::default()
    }

    pub fn current(&self) -> String {
        self.0.0.lock().unwrap().1.clone()
    }

    pub fn send(&self, snapshot: String) {
        let mut current = self.0.0.lock().unwrap();
        current.0 += 1;
        current.1 = snapshot;
        self.0.1.notify_all();
    }

    // The snapshot once its version is past `seen`, or None after `timeout`.
    fn next(&self, seen: u64, timeout: Duration) -> Option<(u64, String)> {
        let current = self.0.0.lock().unwrap();
        let (current, _) = (self.0.1)
            .wait_timeout_while(current, timeout, |(version, _)| *version == seen)
            .unwrap();
        (current.0 != seen).then(|| current.clone())
    }
}

type Stdin = Arc<Mutex<ChildStdin>>;

fn write(stdin: &Stdin, message: &Value) -> bool {
    let mut stdin = stdin.lock().unwrap();
    writeln!(stdin, "{message}").and_then(|()| stdin.flush()).is_ok()
}

// Answers the frontends' requests with `handler`, which is given each request's payload and
// returns the reply, or an error code and message for the frontend to see, each on its own
// thread. Sends each new snapshot to the frontends. Never returns.
pub fn serve<H>(handler: H, snapshots: Snapshots) -> !
where
    H: Fn(String) -> Result<String, (u32, String)> + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    loop {
        if let Err(message) = run(&handler, &snapshots) {
            error!("the room helper failed", error = message);
        }
        thread::sleep(RESTART_DELAY);
    }
}

fn run<H>(handler: &Arc<H>, snapshots: &Snapshots) -> Result<(), String>
where
    H: Fn(String) -> Result<String, (u32, String)> + Send + Sync + 'static,
{
    let mut child = gem_native::command(&["room"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", gem_native::path().display()))?;
    let stdin: Stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
    let stdout = child.stdout.take().unwrap();
    let alive = Arc::new(AtomicBool::new(true));

    // gem-native keeps the latest snapshot and sends it to each frontend as it joins.
    let forwarding = {
        let (stdin, snapshots, alive) = (stdin.clone(), snapshots.clone(), alive.clone());
        thread::spawn(move || {
            let mut seen = 0;
            while alive.load(Ordering::Relaxed) {
                let Some((version, snapshot)) = snapshots.next(seen, Duration::from_secs(1)) else {
                    continue;
                };
                seen = version;
                // Empty until the server has filled its first snapshot.
                if !snapshot.is_empty() && !write(&stdin, &json!({"snapshot": snapshot})) {
                    return;
                }
            }
        })
    };

    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let message = match gem_json::parse(&line) {
            Ok(message) => message,
            Err(error) => {
                warn!("unreadable message from the room helper", error = error, line = line);
                continue;
            }
        };
        let request = &message["request"];
        let (Some(id), Some(payload)) = (request["id"].as_str(), request["payload"].as_str()) else {
            continue;
        };
        let (id, payload) = (id.to_string(), payload.to_string());
        let (handler, stdin) = (handler.clone(), stdin.clone());
        thread::spawn(move || {
            let reply = match handler(payload) {
                Ok(ok) => json!({"reply": {"id": id, "ok": ok}}),
                Err((code, message)) => json!({"reply": {"id": id, "code": code, "message": message}}),
            };
            write(&stdin, &reply);
        });
    }
    alive.store(false, Ordering::Relaxed);
    let _ = forwarding.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    Err(format!("gem-native room exited: {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_wakes_who_waits_for_it() {
        let snapshots = Snapshots::new();
        assert_eq!(snapshots.next(0, Duration::from_millis(10)), None);
        let sender = snapshots.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            sender.send("a".to_string());
        });
        assert_eq!(snapshots.next(0, Duration::from_secs(5)), Some((1, "a".to_string())));
        assert_eq!(snapshots.current(), "a");
    }
}
