// gem-merges, reaching T3 Code through its CLI.

use std::sync::OnceLock;

use gem_db::Db;
use gem_merges::{Host, Merges, Progress};

use crate::agents::Running;
use gem_proto::{MergeBy, PullRequestCluster};

static MERGES: OnceLock<Merges<Server>> = OnceLock::new();

struct Server;

impl Host for Server {
    fn start_thread(&self, title: &str, prompt: &str) -> Result<String, String> {
        gem_t3::start(title, prompt, None)
    }

    fn progress(&self, thread_id: &str) -> Option<Progress> {
        let (done, question) = gem_t3::progress(thread_id)?;
        Some(Progress { done, question })
    }
}

// Also watches the merges the server's last run left.
pub fn init(db: Db) -> Result<(), String> {
    let merges = Merges::new(db, Server)?;
    merges.resume()?;
    let _ = MERGES.set(merges);
    Ok(())
}

pub fn start(cluster: &PullRequestCluster) -> Result<(), String> {
    let urls: Vec<String> = cluster
        .pull_requests
        .iter()
        .map(|pr| pr.url.clone())
        .collect();
    let name = match cluster.name.as_str() {
        "" => cluster.pull_requests[0].title.as_str(),
        name => name,
    };
    let admin: Vec<String> = (cluster.pull_requests.iter())
        .filter(|pr| pr.merge_by() == MergeBy::Admin)
        .map(|pr| pr.url.clone())
        .collect();
    let merges = MERGES.get().ok_or("merges are not ready")?;
    merges.start(&urls, &admin, name)
}

pub fn fill(clusters: &mut [PullRequestCluster]) {
    let merges = all();
    for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
        pr.merging = merges.contains_key(&pr.url);
    }
}

// Each agent merging, once however many pull requests it merges.
pub fn running() -> Vec<Running> {
    let mut running: Vec<Running> = Vec::new();
    for (url, merge) in all() {
        match running.iter_mut().find(|r| r.thread_id == merge.thread_id) {
            Some(agent) => agent.pull_request_urls.push(url),
            None => running.push(Running {
                thread_id: merge.thread_id,
                // Empty for one started before its title was kept.
                title: match merge.title.as_str() {
                    "" => "Merge".to_string(),
                    _ => merge.title,
                },
                question: merge.question,
                started_at: merge.started_at,
                pull_request_urls: vec![url],
            }),
        }
    }
    running
}

pub fn forget(thread_id: &str) -> Result<(), String> {
    MERGES
        .get()
        .ok_or("merges are not ready")?
        .forget(thread_id)
}

fn all() -> std::collections::BTreeMap<String, gem_merges::Merge> {
    let Some(merges) = MERGES.get() else {
        return Default::default();
    };
    merges.all().unwrap_or_else(|error| {
        eprintln!("{error}");
        Default::default()
    })
}
