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
    };
    REBASES.lock().await.insert(url.to_string(), rebase);
    tokio::spawn(watch(url.to_string(), thread_id, prs));
    Ok(())
}

pub async fn fill(clusters: &mut [PullRequestCluster]) {
    let rebases = REBASES.lock().await;
    for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
        pr.rebase = rebases.get(&pr.url).cloned();
    }
}

// Search keeps listing the old head until the next refresh, so the pull request stops being
// behind here rather than offer another rebase.
async fn watch(url: String, thread_id: String, prs: Cache<ListOpenPullRequestsResponse>) {
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
    REBASES.lock().await.remove(&url);
    if let Some(response) = &mut *prs.write().await {
        for pr in response
            .clusters
            .iter_mut()
            .flat_map(|c| &mut c.pull_requests)
        {
            if pr.url == url {
                pr.behind = false;
            }
        }
    }
}
