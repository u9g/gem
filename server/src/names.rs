use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Condvar, Mutex, RwLock},
    thread,
};

use gem_db::params;
use gem_log::warn;

use crate::{
    proto::{PullRequest, PullRequestCluster},
    t3,
};

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
    // How many more threads may be asked at once, up to CONCURRENT_THREADS.
    threads: (Mutex<usize>, Condvar),
}

// A thread being asked; dropping it lets the next one be asked.
struct Permit<'a>(&'a (Mutex<usize>, Condvar));

impl<'a> Permit<'a> {
    fn acquire(threads: &'a (Mutex<usize>, Condvar)) -> Permit<'a> {
        let free = threads.0.lock().unwrap();
        let mut free = threads.1.wait_while(free, |free| *free == 0).unwrap();
        *free -= 1;
        Permit(threads)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() += 1;
        self.0.1.notify_one();
    }
}

impl Names {
    pub fn load() -> Result<Self, String> {
        let known = crate::db().with(|db| {
            db.prepare("SELECT key, name FROM cluster_names")?
                .query_map(params![], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        Ok(Self(Arc::new(Inner {
            known: RwLock::new(known),
            requested: Mutex::default(),
            threads: (Mutex::new(CONCURRENT_THREADS), Condvar::new()),
        })))
    }

    pub fn fill(&self, clusters: &mut [PullRequestCluster]) {
        let known = self.0.known.read().unwrap();
        for cluster in clusters {
            if let Some(name) = known.get(&key(&cluster.pull_requests)) {
                cluster.name = name.clone();
            }
        }
    }

    pub fn request_missing(&self, clusters: &[PullRequestCluster]) {
        let known = self.0.known.read().unwrap();
        let mut requested = self.0.requested.lock().unwrap();
        for cluster in clusters {
            let key = key(&cluster.pull_requests);
            if known.contains_key(&key) || !requested.insert(key.clone()) {
                continue;
            }
            let names = self.clone();
            let prs = cluster.pull_requests.clone();
            thread::spawn(move || {
                let _permit = Permit::acquire(&names.0.threads);
                if let Some(name) = ask_t3(&prs) {
                    let saved = crate::db().with(|db| {
                        db.execute(
                            "INSERT OR REPLACE INTO cluster_names (key, name) VALUES (?1, ?2)",
                            params![key, name],
                        )
                    });
                    if let Err(error) = saved {
                        warn!("cannot save a cluster's name", error = error);
                    }
                    names.0.known.write().unwrap().insert(key, name);
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

fn ask_t3(prs: &[PullRequest]) -> Option<String> {
    let list: Vec<String> = prs
        .iter()
        .map(|pr| format!("- {}#{}: {}", full_name(pr), pr.number, pr.title))
        .collect();
    let first = &prs[0];
    // A cloud agent starts on one repository, and T3 Code picks none for a cluster spanning
    // several, so the thread fails before the agent runs. Naming reads no code: any of the
    // cluster's repositories will do.
    t3::ask(
        &format!("Name PR cluster {}#{}", full_name(first), first.number),
        &format!(
            "{PROMPT}\n\nRun on the {} repository.\n\n{}",
            full_name(first),
            list.join("\n")
        ),
    )
}

fn full_name(pr: &PullRequest) -> String {
    let repository = pr.repository.clone().unwrap_or_default();
    format!("{}/{}", repository.owner, repository.name)
}
