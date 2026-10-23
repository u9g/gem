use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    sync::Arc,
};

use serde::Deserialize;
use tokio::{
    process::Command,
    sync::{Mutex, RwLock, Semaphore},
};

use crate::proto::{PullRequest, PullRequestCluster};

// Relative to the server's working directory, which must be inside the `gem` T3 Code project:
// naming threads are started in the project containing ".".
const NAMES_FILE: &str = "cluster-names.json";
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

impl Names {
    pub async fn load() -> Self {
        let known = match tokio::fs::read(NAMES_FILE).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(_) => HashMap::new(),
        };
        Self(Arc::new(Inner {
            known: RwLock::new(known),
            requested: Mutex::default(),
            threads: Semaphore::new(CONCURRENT_THREADS),
        }))
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
                    let mut known = names.0.known.write().await;
                    known.insert(key, name);
                    let _ =
                        tokio::fs::write(NAMES_FILE, serde_json::to_vec_pretty(&*known).unwrap())
                            .await;
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

#[derive(Deserialize)]
struct Run {
    #[serde(rename = "threadId")]
    thread_id: String,
    status: String,
    reply: Option<String>,
}

#[derive(Deserialize)]
struct Shown {
    thread: ShownThread,
}

#[derive(Deserialize)]
struct ShownThread {
    settled: bool,
}

async fn ask_t3(prs: &[PullRequest]) -> Option<String> {
    let list: Vec<String> = prs
        .iter()
        .map(|pr| format!("- {}#{}: {}", pr.repository, pr.number, pr.title))
        .collect();
    let first = &prs[0];
    let output = t3(&[
        "thread",
        "start",
        ".",
        "--title",
        &format!("Name PR cluster {}#{}", first.repository, first.number),
        "--wait",
        "--json",
        &format!("{PROMPT}\n\n{}", list.join("\n")),
    ])
    .await?;
    let run: Run = serde_json::from_slice(&output).ok()?;

    let shown: Option<Shown> = t3(&["thread", "show", "--json", &run.thread_id])
        .await
        .and_then(|output| serde_json::from_slice(&output).ok());
    if !shown.is_some_and(|shown| shown.thread.settled) {
        eprintln!("naming thread {} did not settle", run.thread_id);
    }

    let name = run.reply?.trim().to_string();
    (run.status == "completed" && !name.is_empty()).then_some(name)
}

// T3 Code installs its CLI here and keeps it pointing at the running app.
async fn t3(args: &[&str]) -> Option<Vec<u8>> {
    let cli = PathBuf::from(env::var("HOME").ok()?).join(".t3/bin/t3");
    Some(Command::new(cli).args(args).output().await.ok()?.stdout)
}
