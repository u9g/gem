// gem-merges, reaching T3 Code through its CLI.

use std::sync::OnceLock;

use gem_db::Db;
use gem_merges::{Host, Merges, Progress};

use crate::proto::{Merge, MergeBy, PullRequestCluster};
use crate::t3;

static MERGES: OnceLock<Merges<Server>> = OnceLock::new();

struct Server;

impl Host for Server {
    async fn start_thread(&self, title: &str, prompt: &str) -> Result<String, String> {
        t3::start(title, prompt).await
    }

    async fn progress(&self, thread_id: &str) -> Option<Progress> {
        let (done, question) = t3::progress(thread_id).await?;
        Some(Progress { done, question })
    }

    fn thread_url(&self, thread_id: &str) -> String {
        t3::url(thread_id).unwrap_or_default()
    }
}

// Also watches the merges the server's last run left.
pub fn init(db: Db) -> Result<(), String> {
    let merges = Merges::new(db, Server)?;
    merges.resume()?;
    let _ = MERGES.set(merges);
    Ok(())
}

pub async fn start(cluster: &PullRequestCluster) -> Result<(), String> {
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
    merges.start(&urls, &admin, name).await
}

pub async fn fill(clusters: &mut [PullRequestCluster]) {
    let Some(merges) = MERGES.get() else { return };
    let merges = merges.all().unwrap_or_else(|error| {
        eprintln!("{error}");
        Default::default()
    });
    for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
        pr.merge = merges.get(&pr.url).map(|merge| Merge {
            thread_url: merge.thread_url.clone(),
            question: merge.question,
        });
    }
}
