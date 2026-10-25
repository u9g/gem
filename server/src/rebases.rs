use std::{collections::BTreeMap, time::Duration};

use tokio::sync::Mutex;

use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster, Rebase};
use crate::{Cache, gh, investigations, t3};

// Reading a thread is a local CLI call.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

// By pull request URL, until the agent's thread settles or stops.
static REBASES: Mutex<BTreeMap<String, Rebase>> = Mutex::const_new(BTreeMap::new());

// The agent settles its thread when done: T3 Code does not settle threads when they finish.
pub async fn start(url: &str, prs: Cache<ListOpenPullRequestsResponse>) -> Result<(), String> {
    let base = gh(&[
        "pr",
        "view",
        url,
        "--json",
        "baseRefName",
        "--jq",
        ".baseRefName",
    ])
    .await
    .ok_or(format!("gh: cannot read {url}"))?;
    let base = String::from_utf8_lossy(&base).trim().to_string();
    let head = head(url).await.ok_or(format!("gh: cannot read {url}"))?;
    let (repository, number) = investigations::short(url);
    let prompt = format!(
        "rebase on {base}: {url}

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread."
    );
    let thread_id = t3::start(&format!("Rebase {repository}#{number}"), &prompt).await?;
    let rebase = Rebase {
        thread_url: t3::url(&thread_id).unwrap_or_default(),
        question: false,
        pushed: false,
    };
    REBASES.lock().await.insert(url.to_string(), rebase);
    tokio::spawn(watch(url.to_string(), head, thread_id, prs));
    Ok(())
}

pub async fn fill(clusters: &mut [PullRequestCluster]) {
    let rebases = REBASES.lock().await;
    for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
        pr.rebase = rebases.get(&pr.url).cloned();
    }
}

// Search keeps listing the old head, with its checks and how far behind it is, until its next
// refresh, so a pushed rebase lasts until the pull request is listed with another head.
async fn watch(
    url: String,
    head: String,
    thread_id: String,
    prs: Cache<ListOpenPullRequestsResponse>,
) {
    loop {
        tokio::time::sleep(REFRESH_INTERVAL).await;
        let Some((done, question)) = t3::progress(&thread_id).await else {
            continue;
        };
        if done {
            break;
        }
        if let Some(rebase) = REBASES.lock().await.get_mut(&url) {
            rebase.question = question;
        }
    }
    if self::head(&url).await.is_some_and(|pushed| pushed != head) {
        if let Some(rebase) = REBASES.lock().await.get_mut(&url) {
            rebase.question = false;
            rebase.pushed = true;
        }
        while listed_head(&prs, &url)
            .await
            .is_some_and(|listed| listed == head)
        {
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    }
    REBASES.lock().await.remove(&url);
}

async fn head(url: &str) -> Option<String> {
    let head = gh(&[
        "pr",
        "view",
        url,
        "--json",
        "headRefOid",
        "--jq",
        ".headRefOid",
    ])
    .await?;
    Some(String::from_utf8_lossy(&head).trim().to_string())
}

// None once the pull request is no longer listed.
async fn listed_head(prs: &Cache<ListOpenPullRequestsResponse>, url: &str) -> Option<String> {
    let prs = prs.read().await;
    (prs.iter().flat_map(|r| &r.clusters))
        .flat_map(|c| &c.pull_requests)
        .find(|pr| pr.url == url)
        .map(|pr| pr.head_oid.clone())
}
