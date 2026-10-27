use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use gem_db::{Db, rusqlite::params};
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{
    proto::{PullRequest, PullRequestCluster},
    t3,
};

// Naming threads are started in the project containing the server's working directory, which
// must be inside the `gem` T3 Code project.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE cluster_names (
        -- See `key`.
        key TEXT PRIMARY KEY,
        name TEXT NOT NULL
    );
"];
const CONCURRENT_THREADS: usize = 4;

const PROMPT: &str = "Give this cluster of related pull requests a short name (2-5 words) \
describing the shared work.

First call the t3_thread_organize tool with action \"settle\" and no threadId, to settle this \
thread. Use no other tools. Then reply with only the name, nothing else.";

// A cluster's name belongs to its exact PRs and their titles: a PR joining or leaving the
// cluster, or any title changing, makes a new key and asks a new thread for a name.
#[derive(Clone)]
pub struct Names(Arc<Inner>);

struct Inner {
    known: RwLock<HashMap<String, String>>,
    // Never cleared, so a failed thread is not retried every refresh; a restart retries it.
    requested: Mutex<HashSet<String>>,
    threads: Semaphore,
}

pub fn migrate(db: &Db) -> Result<(), String> {
    db.migrate("cluster_names", MIGRATIONS)
}

impl Names {
    pub fn load() -> Result<Self, String> {
        let known = crate::db().with(|db| {
            db.prepare("SELECT key, name FROM cluster_names")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        Ok(Self(Arc::new(Inner {
            known: RwLock::new(known),
            requested: Mutex::default(),
            threads: Semaphore::new(CONCURRENT_THREADS),
        })))
    }

    pub async fn fill(&self, clusters: &mut [PullRequestCluster]) {
        let known = self.0.known.read().await;
        for cluster in clusters {
            if let Some(name) = known.get(&key(&cluster.pull_requests)) {
                cluster.name = name.clone();
            }
        }
    }

    pub async fn request_missing(&self, clusters: &[PullRequestCluster]) {
        let known = self.0.known.read().await;
        let mut requested = self.0.requested.lock().await;
        for cluster in clusters {
            let key = key(&cluster.pull_requests);
            if known.contains_key(&key) || !requested.insert(key.clone()) {
                continue;
            }
            let names = self.clone();
            let prs = cluster.pull_requests.clone();
            tokio::spawn(async move {
                let _permit = names.0.threads.acquire().await.unwrap();
                if let Some(name) = ask_t3(&prs).await {
                    let saved = crate::db().with(|db| {
                        db.execute(
                            "INSERT OR REPLACE INTO cluster_names (key, name) VALUES (?1, ?2)",
                            params![key, name],
                        )
                    });
                    if let Err(error) = saved {
                        tracing::warn!(%error, "cannot save a cluster's name");
                    }
                    names.0.known.write().await.insert(key, name);
                }
            });
        }
    }
}

fn key(prs: &[PullRequest]) -> String {
    let mut lines: Vec<String> = prs
        .iter()
        .map(|pr| format!("{} {}", pr.url, pr.title))
        .collect();
    lines.sort();
    lines.join("\n")
}

async fn ask_t3(prs: &[PullRequest]) -> Option<String> {
    let list: Vec<String> = prs
        .iter()
        .map(|pr| format!("- {}#{}: {}", full_name(pr), pr.number, pr.title))
        .collect();
    let first = &prs[0];
    t3::ask(
        &format!("Name PR cluster {}#{}", full_name(first), first.number),
        &format!("{PROMPT}\n\n{}", list.join("\n")),
    )
    .await
}

fn full_name(pr: &PullRequest) -> String {
    let repository = pr.repository.clone().unwrap_or_default();
    format!("{}/{}", repository.owner, repository.name)
}
