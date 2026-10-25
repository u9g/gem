use std::{collections::HashMap, sync::Arc, time::Duration};

use reqwest::{
    Client, StatusCode,
    header::{ETAG, IF_NONE_MATCH},
};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinSet,
};

use crate::{SearchedPullRequest, gh};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
// GitHub allows 900 REST points a minute and documents no exemption for a 304.
const REQUESTS_PER_MINUTE: u32 = 600;
// GitHub allows 100 concurrent requests.
const CONCURRENT_REQUESTS: usize = 50;

// Notices GitHub changing what gem shows of the open pull requests, between refreshes. Each poll
// asks for every watched response again with its last ETag; GitHub answers 304 Not Modified
// while it is unchanged, which costs none of the hourly rate limit.
#[derive(Clone, Default)]
pub struct Changes(Arc<Inner>);

#[derive(Default)]
struct Inner {
    // REST paths under https://api.github.com/.
    paths: Mutex<Vec<String>>,
    changed: Notify,
}

impl Changes {
    // Each PR's own response changes with pushes, its title, comments, reviews and closing; its
    // head commit's check runs and statuses with its checks; and its base branch with `behind`.
    pub async fn watch(&self, prs: &[SearchedPullRequest]) {
        let mut paths: Vec<String> = prs
            .iter()
            .flat_map(|pr| {
                let repo = format!("repos/{}/{}", pr.repository.owner, pr.repository.name);
                [
                    format!("{repo}/pulls/{}", pr.number),
                    format!("{repo}/commits/{}/check-runs?per_page=100", pr.head_oid),
                    format!("{repo}/commits/{}/status", pr.head_oid),
                    format!("{repo}/git/ref/heads/{}", pr.base),
                ]
            })
            .collect();
        paths.sort();
        paths.dedup();
        *self.0.paths.lock().await = paths;
    }

    // Resolves once a watched response changed since the last call returned.
    pub async fn changed(&self) {
        self.0.changed.notified().await;
    }

    pub fn poll(&self) {
        let changes = self.clone();
        tokio::spawn(async move {
            // reqwest's TLS takes the process's default crypto provider, and ring is the one
            // LiveKit's SDK builds on.
            let _ = rustls::crypto::ring::default_provider().install_default();
            let client = Client::new();
            let mut etags: HashMap<String, String> = HashMap::new();
            loop {
                let paths = changes.0.paths.lock().await.clone();
                let interval = Duration::from_secs(60) * paths.len() as u32 / REQUESTS_PER_MINUTE;
                tokio::time::sleep(interval.max(POLL_INTERVAL)).await;
                let Some(token) = gh(&["auth", "token"]).await else {
                    continue;
                };
                let token = String::from_utf8_lossy(&token).trim().to_string();
                etags.retain(|path, _| paths.contains(path));
                let mut changed = false;
                for chunk in paths.chunks(CONCURRENT_REQUESTS) {
                    let mut requests = JoinSet::new();
                    for path in chunk {
                        let mut request = client
                            .get(format!("https://api.github.com/{path}"))
                            .bearer_auth(&token)
                            .header("user-agent", "gem");
                        if let Some(etag) = etags.get(path) {
                            request = request.header(IF_NONE_MATCH, etag);
                        }
                        let path = path.clone();
                        requests.spawn(async move { (path, request.send().await) });
                    }
                    while let Some(result) = requests.join_next().await {
                        let Ok((path, Ok(response))) = result else {
                            continue;
                        };
                        if response.status() != StatusCode::OK {
                            continue;
                        }
                        let Some(etag) = response.headers().get(ETAG) else {
                            continue;
                        };
                        let etag = String::from_utf8_lossy(etag.as_bytes()).into_owned();
                        // A response first seen is already in the refresh that watched it.
                        changed |= etags
                            .insert(path, etag.clone())
                            .is_some_and(|old| old != etag);
                    }
                }
                if changed {
                    changes.0.changed.notify_one();
                }
            }
        });
    }
}
