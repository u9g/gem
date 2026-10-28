use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
// GitHub allows 900 REST points a minute and documents no exemption for a 304.
const REQUESTS_PER_MINUTE: u32 = 600;
// GitHub allows 100 concurrent requests.
const CONCURRENT_REQUESTS: usize = 50;

// Notices GitHub changing the REST responses it watches. Each poll asks for every one again with
// its last ETag; GitHub answers 304 Not Modified while it is unchanged, which costs none of the
// hourly rate limit. The requests are made with curl, which every platform the server runs on has.
#[derive(Clone, Default)]
pub struct Changes(Arc<Inner>);

#[derive(Default)]
struct Inner {
    // REST paths under https://api.github.com/.
    paths: Mutex<Vec<String>>,
    // Whether a watched response changed since `changed` last returned.
    changed: Mutex<bool>,
    notify: Condvar,
}

impl Changes {
    // Watches `paths`, REST paths under https://api.github.com/, instead of the ones before.
    pub fn watch(&self, mut paths: Vec<String>) {
        paths.sort();
        paths.dedup();
        *self.0.paths.lock().unwrap() = paths;
    }

    // Waits until a watched response changed since the last call returned true, or `timeout`
    // passes. Whether one changed.
    pub fn changed(&self, timeout: Duration) -> bool {
        let changed = self.0.changed.lock().unwrap();
        let (mut changed, _) = (self.0.notify)
            .wait_timeout_while(changed, timeout, |changed| !*changed)
            .unwrap();
        std::mem::replace(&mut *changed, false)
    }

    fn notify(&self) {
        *self.0.changed.lock().unwrap() = true;
        self.0.notify.notify_one();
    }

    pub fn poll(&self) {
        let changes = self.clone();
        thread::spawn(move || {
            let mut etags: HashMap<String, String> = HashMap::new();
            loop {
                let paths = changes.0.paths.lock().unwrap().clone();
                let interval = Duration::from_secs(60) * paths.len() as u32 / REQUESTS_PER_MINUTE;
                thread::sleep(interval.max(POLL_INTERVAL));
                let Some(token) = token() else {
                    continue;
                };
                etags.retain(|path, _| paths.contains(path));
                let mut changed = false;
                for chunk in paths.chunks(CONCURRENT_REQUESTS) {
                    let responses: Vec<(String, Option<String>)> = thread::scope(|scope| {
                        let requests: Vec<_> = (chunk.iter())
                            .map(|path| {
                                let (token, etag) = (&token, etags.get(path));
                                scope.spawn(move || (path.clone(), etag_of(path, token, etag)))
                            })
                            .collect();
                        requests.into_iter().filter_map(|r| r.join().ok()).collect()
                    });
                    for (path, etag) in responses {
                        let Some(etag) = etag else {
                            continue;
                        };
                        // A response first seen is already in the refresh that watched it.
                        changed |= etags
                            .insert(path, etag.clone())
                            .is_some_and(|old| old != etag);
                    }
                }
                if changed {
                    changes.notify();
                }
            }
        });
    }
}

// The ETag of GitHub's 200 for `path`, asked for with `etag` as If-None-Match. None for anything
// else, a 304 Not Modified included.
fn etag_of(path: &str, token: &str, etag: Option<&String>) -> Option<String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let url = format!("https://api.github.com/{path}");
    // The headers go on stdin, so the token is not in the process list.
    let mut child = Command::new("curl")
        .args(["--silent", "--max-time", "30", "--output", null, "--dump-header", "-", "--header", "@-", &url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut headers = format!("Authorization: Bearer {token}\nUser-Agent: gem\n");
    if let Some(etag) = etag {
        headers.push_str(&format!("If-None-Match: {etag}\n"));
    }
    let mut stdin = child.stdin.take()?;
    let written = stdin.write_all(headers.as_bytes());
    drop(stdin);
    let output = child.wait_with_output().ok()?;
    written.ok()?;
    parse_etag(&String::from_utf8_lossy(&output.stdout))
}

// The ETag of a 200 response's headers.
fn parse_etag(headers: &str) -> Option<String> {
    let mut lines = headers.lines();
    let status = lines.next()?.split_whitespace().nth(1)?;
    if status != "200" {
        return None;
    }
    lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("etag"))
        .map(|(_, value)| value.trim().to_string())
}

// The token gh is authenticated with, asked for every poll so a new login is picked up.
fn token() -> Option<String> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    (output.status.success()).then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_etag_of_a_200() {
        let ok = "HTTP/2 200 \r\ncontent-type: application/json\r\nETag: W/\"abc\"\r\n\r\n";
        assert_eq!(parse_etag(ok), Some("W/\"abc\"".to_string()));
        assert_eq!(parse_etag("HTTP/2 304 \r\netag: \"abc\"\r\n\r\n"), None);
        assert_eq!(parse_etag(""), None);
    }

    #[test]
    fn a_change_wakes_who_waits_and_is_seen_once() {
        let changes = Changes::default();
        assert!(!changes.changed(Duration::from_millis(10)));
        let notifier = changes.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            notifier.notify();
        });
        assert!(changes.changed(Duration::from_secs(5)));
        assert!(!changes.changed(Duration::from_millis(10)));
    }
}
