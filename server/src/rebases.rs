// gem-rebases, reaching GitHub through gh, T3 Code through its CLI, and the listed head through
// the server's cache.

use std::sync::OnceLock;

use gem_db::Db;
use gem_rebases::{Host, Progress, Rebases, Refs};

use crate::agents::Running;
use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster};
use crate::{Cache, gh, t3};

static REBASES: OnceLock<Rebases<Server>> = OnceLock::new();

struct Server {
    prs: Cache<ListOpenPullRequestsResponse>,
}

impl Host for Server {
    fn refs(&self, url: &str) -> Option<Refs> {
        let refs = gh("refs", &[
            "pr",
            "view",
            url,
            "--json",
            "baseRefName,headRefName,headRefOid",
            "--jq",
            r#".baseRefName + " " + .headRefName + " " + .headRefOid"#,
        ])?;
        let refs = String::from_utf8_lossy(&refs);
        let [base, branch, head] = refs.split_whitespace().collect::<Vec<_>>()[..] else {
            return None;
        };
        Some(Refs {
            base: base.to_string(),
            branch: branch.to_string(),
            head: head.to_string(),
        })
    }

    fn start_thread(&self, title: &str, prompt: &str) -> Result<String, String> {
        t3::start(title, prompt, None)
    }

    fn progress(&self, thread_id: &str) -> Option<Progress> {
        let (done, question) = t3::progress(thread_id)?;
        Some(Progress { done, question })
    }

    fn listed_head(&self, url: &str) -> Option<String> {
        let prs = self.prs.read().unwrap();
        (prs.iter().flat_map(|r| &r.clusters))
            .flat_map(|c| &c.pull_requests)
            .find(|pr| pr.url == url)
            .map(|pr| pr.head_oid.clone())
    }
}

// Also watches the rebases the server's last run left.
pub fn init(db: Db, prs: Cache<ListOpenPullRequestsResponse>) -> Result<(), String> {
    let rebases = Rebases::new(db, Server { prs })?;
    rebases.resume()?;
    let _ = REBASES.set(rebases);
    Ok(())
}

pub fn start(url: &str) -> Result<(), String> {
    let (repository, number) = gem_github::short(url);
    let rebases = REBASES.get().ok_or("rebases are not ready")?;
    rebases.start(&[url.to_string()], &format!("{repository}#{number}"))
}

pub fn start_cluster(cluster: &PullRequestCluster) -> Result<(), String> {
    let urls: Vec<String> = (cluster.pull_requests.iter())
        .map(|pr| pr.url.clone())
        .collect();
    let name = match cluster.name.as_str() {
        "" => cluster.pull_requests[0].title.as_str(),
        name => name,
    };
    let rebases = REBASES.get().ok_or("rebases are not ready")?;
    rebases.start(&urls, name)
}

pub fn fill(clusters: &mut [PullRequestCluster]) {
    let rebases = all();
    for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
        let rebase = rebases.get(&pr.url);
        pr.rebasing = rebase.is_some_and(|rebase| !rebase.pushed);
        pr.rebased = rebase.is_some_and(|rebase| rebase.pushed);
    }
}

// Each agent still rebasing, once however many pull requests it rebases. One that pushed is done:
// its pull requests show `rebased` until they are listed with their new heads.
pub fn running() -> Vec<Running> {
    let mut running: Vec<Running> = Vec::new();
    for (url, rebase) in all().into_iter().filter(|(_, rebase)| !rebase.pushed) {
        match running.iter_mut().find(|r| r.thread_id == rebase.thread_id) {
            Some(agent) => agent.pull_request_urls.push(url),
            None => running.push(Running {
                thread_id: rebase.thread_id,
                // Empty for one started before its title was kept.
                title: match rebase.title.as_str() {
                    "" => "Rebase".to_string(),
                    _ => rebase.title,
                },
                question: rebase.question,
                started_at: rebase.started_at,
                pull_request_urls: vec![url],
            }),
        }
    }
    running
}

pub fn forget(thread_id: &str) -> Result<(), String> {
    REBASES.get().ok_or("rebases are not ready")?.forget(thread_id)
}

fn all() -> std::collections::BTreeMap<String, gem_rebases::Rebase> {
    let Some(rebases) = REBASES.get() else {
        return Default::default();
    };
    rebases.all().unwrap_or_else(|error| {
        eprintln!("{error}");
        Default::default()
    })
}
