// gem-rebases, reaching GitHub through gh, T3 Code through its CLI, and the listed head through
// the server's cache.

use std::sync::OnceLock;

use gem_db::Db;
use gem_rebases::{Host, Progress, Rebases, Refs};

use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster, Rebase};
use crate::{Cache, gh, t3};

static REBASES: OnceLock<Rebases<Server>> = OnceLock::new();

struct Server {
    prs: Cache<ListOpenPullRequestsResponse>,
}

impl Host for Server {
    async fn refs(&self, url: &str) -> Option<Refs> {
        let refs = gh(&[
            "pr",
            "view",
            url,
            "--json",
            "baseRefName,headRefName,headRefOid",
            "--jq",
            r#".baseRefName + " " + .headRefName + " " + .headRefOid"#,
        ])
        .await?;
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

    async fn start_thread(&self, title: &str, prompt: &str) -> Result<String, String> {
        t3::start(title, prompt, None).await
    }

    async fn progress(&self, thread_id: &str) -> Option<Progress> {
        let (done, question) = t3::progress(thread_id).await?;
        Some(Progress { done, question })
    }

    async fn listed_head(&self, url: &str) -> Option<String> {
        let prs = self.prs.read().await;
        (prs.iter().flat_map(|r| &r.clusters))
            .flat_map(|c| &c.pull_requests)
            .find(|pr| pr.url == url)
            .map(|pr| pr.head_oid.clone())
    }

    fn thread_url(&self, thread_id: &str) -> String {
        t3::url(thread_id).unwrap_or_default()
    }
}

// Also watches the rebases the server's last run left.
pub fn init(db: Db, prs: Cache<ListOpenPullRequestsResponse>) -> Result<(), String> {
    let rebases = Rebases::new(db, Server { prs })?;
    rebases.resume()?;
    let _ = REBASES.set(rebases);
    Ok(())
}

pub async fn start(url: &str) -> Result<(), String> {
    let (repository, number) = gem_github::short(url);
    let rebases = REBASES.get().ok_or("rebases are not ready")?;
    rebases.start(url, &format!("{repository}#{number}")).await
}

pub async fn fill(clusters: &mut [PullRequestCluster]) {
    let Some(rebases) = REBASES.get() else { return };
    let rebases = rebases.all().unwrap_or_else(|error| {
        eprintln!("{error}");
        Default::default()
    });
    for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
        pr.rebase = rebases.get(&pr.url).map(|rebase| Rebase {
            thread_url: rebase.thread_url.clone(),
            question: rebase.question,
            pushed: rebase.pushed,
        });
    }
}
