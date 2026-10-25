use std::{collections::BTreeMap, time::Duration};

use rusqlite::{OptionalExtension, params};
use tokio::sync::Mutex;

use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster, Rebase};
use crate::{Cache, db, gh, investigations, t3};

// Reading a thread is a local CLI call.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
// How many reads of a thread can fail in a row, e.g. after it is deleted, before its rebase is
// dropped and the pull request can be rebased again.
const MAX_FAILED_READS: u32 = 30;

// Held from checking for a rebase until its row is written, so two presses of Rebase, from any
// number of frontends, start one agent.
static STARTING: Mutex<()> = Mutex::const_new(());

// Rebases are rows in gem.sqlite until the agent's thread settles or stops, so a pull request
// keeps its one rebase across presses, frontends and server restarts. Pressing Rebase while one
// runs does nothing.
//
// The agent settles its thread when done: T3 Code does not settle threads when they finish.
pub async fn start(url: &str, prs: Cache<ListOpenPullRequestsResponse>) -> Result<(), String> {
    let _starting = STARTING.lock().await;
    let running = db::with(|db| {
        db.query_row("SELECT 1 FROM rebases WHERE url = ?1", [url], |_| Ok(()))
            .optional()
    })?;
    if running.is_some() {
        return Ok(());
    }

    let refs = gh(&[
        "pr",
        "view",
        url,
        "--json",
        "baseRefName,headRefName,headRefOid",
        "--jq",
        r#".baseRefName + " " + .headRefName + " " + .headRefOid"#,
    ])
    .await
    .ok_or(format!("gh: cannot read {url}"))?;
    let refs = String::from_utf8_lossy(&refs);
    let [base, branch, head] = refs.split_whitespace().collect::<Vec<_>>()[..] else {
        return Err(format!("gh: cannot read {url}"));
    };
    let (repository, number) = investigations::short(url);
    // The lease keeps the agent from overwriting a push made after the rebase was asked for.
    let prompt = format!(
        "rebase on {base}: {url}

Its branch {branch} was at {head} when this was asked. If it has moved since, or it is no longer \
behind {base}, stop without pushing. Push with `git push --force-with-lease={branch}:{head}`.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread."
    );
    db::with(|db| {
        db.execute(
            "INSERT INTO rebases (url, head) VALUES (?1, ?2)",
            params![url, head],
        )
    })?;
    let thread_id = match t3::start(&format!("Rebase {repository}#{number}"), &prompt).await {
        Ok(thread_id) => thread_id,
        Err(error) => {
            db::with(|db| db.execute("DELETE FROM rebases WHERE url = ?1", [url]))?;
            return Err(error);
        }
    };
    db::with(|db| {
        db.execute(
            "UPDATE rebases SET thread_id = ?2 WHERE url = ?1",
            params![url, thread_id],
        )
    })?;
    tokio::spawn(watch(url.to_string(), head.to_string(), thread_id, prs));
    Ok(())
}

// Watches the rebases a previous run of the server left. One whose thread never started is
// dropped: its server stopped while starting it.
pub fn resume(prs: &Cache<ListOpenPullRequestsResponse>) -> Result<(), String> {
    let rebases = db::with(|db| {
        db.execute("DELETE FROM rebases WHERE thread_id IS NULL", [])?;
        let mut rows = db.prepare("SELECT url, head, thread_id FROM rebases")?;
        rows.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, String)>>>()
    })?;
    for (url, head, thread_id) in rebases {
        tokio::spawn(watch(url, head, thread_id, prs.clone()));
    }
    Ok(())
}

pub async fn fill(clusters: &mut [PullRequestCluster]) {
    let rebases = db::with(|db| {
        let mut rows = db.prepare("SELECT url, thread_id, question, pushed FROM rebases")?;
        rows.query_map([], |row| {
            let thread_id: Option<String> = row.get(1)?;
            let rebase = Rebase {
                thread_url: (thread_id.as_deref().and_then(t3::url)).unwrap_or_default(),
                question: row.get(2)?,
                pushed: row.get(3)?,
            };
            Ok((row.get(0)?, rebase))
        })?
        .collect::<rusqlite::Result<BTreeMap<String, Rebase>>>()
    });
    let rebases = rebases.unwrap_or_else(|error| {
        eprintln!("{error}");
        BTreeMap::new()
    });
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
    let mut failed_reads = 0;
    loop {
        tokio::time::sleep(REFRESH_INTERVAL).await;
        let Some((done, question)) = t3::progress(&thread_id).await else {
            failed_reads += 1;
            if failed_reads >= MAX_FAILED_READS {
                eprintln!("rebase of {url}: cannot read thread {thread_id}; dropping it");
                break;
            }
            continue;
        };
        failed_reads = 0;
        if done {
            break;
        }
        let _ = db::with(|db| {
            db.execute(
                "UPDATE rebases SET question = ?2 WHERE url = ?1",
                params![url, question],
            )
        });
    }
    if self::head(&url).await.is_some_and(|pushed| pushed != head) {
        let _ = db::with(|db| {
            db.execute(
                "UPDATE rebases SET question = 0, pushed = 1 WHERE url = ?1",
                [&url],
            )
        });
        while listed_head(&prs, &url)
            .await
            .is_some_and(|listed| listed == head)
        {
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    }
    if let Err(error) = db::with(|db| db.execute("DELETE FROM rebases WHERE url = ?1", [&url])) {
        eprintln!("{error}");
    }
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
