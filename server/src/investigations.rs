use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pbjson_types::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::proto::{Agent, AgentState, CheckFailure, PullRequestCluster};
use crate::tokens::{Subject, Tokens};
use crate::{login, mcp, proposals, t3};
use gem_github::short;

// One `<id>.json` file per open investigation, relative to the server's working directory.
const DIR: &str = "investigations";
// Where a closed investigation's file moves, so later agents can search what it found.
pub const PAST_DIR: &str = "past-investigations";
// Reading a thread is a local CLI call; only a changed reply reaches GitHub, to render it.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

// An agent gem follows in a file of its own: an investigation, a follow-up, or the answer to the
// user's message about a cluster. Saved as JSON with the names of the `Investigation` message it
// was before every agent was one `Agent`, so files saved then still load.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
struct Investigation {
    id: String,
    title: String,
    // The pull request it started from.
    pull_request_url: String,
    // Every pull request it acts on. Empty in files saved before it was kept: then only
    // `pull_request_url`.
    pull_request_urls: Vec<String>,
    thread_id: String,
    started_at: Option<Timestamp>,
    working: bool,
    // The user has messaged it, so its replies are answers they wait on.
    messaged: bool,
    reply: String,
    reply_html: String,
}

// Held across each read-modify-write of an investigation's file, so a refresh never brings back
// a closed investigation or undoes a message's `working`.
static FILES: Mutex<()> = Mutex::const_new(());

// Approves the rerun first, so the pull request is not held up by the investigation.
pub async fn start(proposal_id: &str) -> Result<(), String> {
    let proposal = proposals::find(proposal_id)
        .await
        .ok_or(format!("no proposal {proposal_id}"))?;
    let failure = (proposal.check_failure.clone())
        .filter(|_| proposal.rerun)
        .ok_or("only a rerun of failed checks can be investigated")?;
    proposals::approve(proposal_id, "", "", "").await?;

    let pr = &proposal.pull_request_url;
    let (repository, number) = short(pr);
    let checks: Vec<&str> = failure.checks.iter().map(|c| c.name.as_str()).collect();
    let title = format!("Investigate {} on {repository}#{number}", checks.join(", "));
    let nanos = now();
    let prompt = prompt(pr, &failure, &proposal.summary);
    let subject = Subject::Investigation {
        id: id(nanos),
        prs: vec![pr.clone()],
    };
    let thread_id = start_proposing(&title, &prompt, &subject).await?;
    track(nanos, title, vec![pr.clone()], thread_id).await;
    Ok(())
}

// Starts the agent an approved proposal hands `follow_up` to, and follows it like an
// investigation, so its replies show in gem and the user can message it.
pub async fn follow_up(title: &str, pr: &str, follow_up: &str) -> Result<(), String> {
    let prompt = format!(
        "{follow_up}

Link {pr} in the description of any pull request you open, so gem groups the two together.

{login} follows this thread in an app that shows only your latest reply, so end each reply \
with where things stand, such as a link to the pull request you opened. {login} may message you \
afterwards in this thread.",
        login = login(),
    );
    let thread_id = t3::start(title, &prompt, None).await?;
    track(now(), title.to_string(), vec![pr.to_string()], thread_id).await;
    Ok(())
}

// Starts an agent on the user's message about a cluster, and follows it like an investigation.
// The prompt carries the cluster as gem showed it when the user sent the message. The agent
// proposes what to do on GitHub rather than do it. Returns the investigation's id.
pub async fn ask(cluster: &PullRequestCluster, message: &str) -> Result<String, String> {
    let nanos = now();
    let first = cluster
        .pull_requests
        .first()
        .ok_or("the cluster is empty")?;
    let name = match cluster.name.as_str() {
        "" => &first.title,
        name => name,
    };
    let topic = message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let topic: String = topic.trim().chars().take(60).collect();
    let title = format!("{name}: {topic}");
    let pull_requests: String = (cluster.pull_requests.iter())
        .map(|pr| format!("- {} ({})\n", pr.url, pr.title))
        .collect();
    let prompt = format!(
        "{login} is looking at a cluster of their pull requests in gem, an app that groups pull \
requests linked by stacking or mentions, and sent you a message about it. The cluster, \
\"{name}\", has:

{pull_requests}
Here it is as gem showed it when {login} sent the message, with each pull request's review and \
check state, and the conversations on them still waiting on {login}, with what other agents \
propose answering. The `get_cluster` tool of the `{server}` MCP server, passed any of those URLs, \
has the cluster as gem shows it now. Use `gh` for anything more, such as diffs, comments and check \
logs.

```json
{snapshot}
```

{login}'s message:

{message}

Read and investigate freely, but never push, comment, review, merge, close or rerun anything on \
GitHub yourself, even when {login} asks you to handle something: propose it, and {login} approves \
it in gem. A proposal is about one of the cluster's pull requests, and can:
- change its code, with `patch` and `message`: clone its repository into a temporary directory, \
run `gh pr checkout <number>` in it, edit, and pass what `git diff` prints as `patch`, and the \
matching change to other pull requests of the cluster the same way as `other_patches`. Never \
commit or push.
- reply as {login}, in the first person, with `reply`: the markdown is posted as a top-level \
comment on the pull request. Open with what {login} is doing, then why: {login} reads only the \
first lines before approving.
- hand the work to another agent, with `follow_up`: when the work is too big to review as \
patches, such as matching changes in other repositories with no pull request in the cluster, \
write a prompt for a T3 Code agent. It must stand alone: the repositories (checked out under \
~/code), the branches or pull requests to build on, what to change and why, and how to verify it. The agent must be able to finish without \
anyone else reviewing, merging or releasing something first.
- close the pull request without merging it, with `close_pull_request`. Pair it with a reply \
saying why.

Record each proposal by calling the `propose` tool of the `{server}` MCP server, with `pr` the \
pull request it is about, a `summary` of one line saying what approving does, and its steps. It \
checks the proposal and returns an error with the reason when something is wrong: fix it and call \
it again. gem merges and rebases pull requests itself, so when one should be merged or brought up \
to date with its base branch, say so in your reply. If the message only asks something, answer it \
and propose nothing.

{login} follows this thread in an app that shows only your latest reply, so end each reply with \
where things stand, such as what you proposed. {login} may message you afterwards in this \
thread, and you can record more proposals the same way.",
        login = login(),
        snapshot = mcp::cluster_json(cluster),
        server = mcp::AGENT_NAME,
    );
    let prs: Vec<String> = (cluster.pull_requests.iter())
        .map(|pr| pr.url.clone())
        .collect();
    let subject = Subject::Investigation {
        id: id(nanos),
        prs: prs.clone(),
    };
    let thread_id = start_proposing(&title, &prompt, &subject).await?;
    track(nanos, title, prs, thread_id).await;
    Ok(id(nanos))
}

// Whether `id` is one of these agents.
pub async fn has(id: &str) -> bool {
    tokio::fs::try_exists(path(id)).await.unwrap_or(false)
}

pub async fn message(id: &str, message: &str) -> Result<(), String> {
    let investigation = load()
        .await
        .into_iter()
        .find(|i| i.id == id)
        .ok_or(format!("no agent {id}"))?;
    t3::send(&investigation.thread_id, message).await?;
    let _files = FILES.lock().await;
    if let Some(mut investigation) = load().await.into_iter().find(|i| i.id == id) {
        investigation.working = true;
        investigation.messaged = true;
        save(&investigation).await;
    }
    Ok(())
}

pub async fn close(id: &str) {
    let _files = FILES.lock().await;
    let _ = tokio::fs::create_dir_all(PAST_DIR).await;
    let _ = tokio::fs::rename(path(id), Path::new(PAST_DIR).join(format!("{id}.json"))).await;
    proposals::remove_of_investigation(id).await;
    Tokens::here().revoke(id).await;
}

pub async fn agents() -> Vec<Agent> {
    let mut proposals = proposals::of_investigations().await;
    (load().await.into_iter())
        .map(|investigation| Agent {
            proposals: proposals.remove(&investigation.id).unwrap_or_default(),
            state: match investigation.working {
                true => AgentState::Working,
                false => AgentState::Waiting,
            } as i32,
            pull_request_urls: match investigation.pull_request_urls.is_empty() {
                true => vec![investigation.pull_request_url],
                false => investigation.pull_request_urls,
            },
            id: investigation.id,
            title: investigation.title,
            reply: investigation.reply,
            reply_html: investigation.reply_html,
            started_at: investigation.started_at,
        })
        .collect()
}

// Keeps each investigation's `working` and reply up to date with its thread. An investigation the
// user never messaged is open only while its agent works or its proposals wait on the user.
pub fn watch() {
    tokio::spawn(async move {
        loop {
            for investigation in load().await {
                let Some((working, reply)) = t3::show(&investigation.thread_id).await else {
                    continue;
                };
                // Read after `show`: an agent records its proposals before it stops working.
                if !working
                    && !investigation.messaged
                    && !proposals::of_investigations()
                        .await
                        .contains_key(&investigation.id)
                {
                    close(&investigation.id).await;
                    tracing::info!(investigation = %investigation.id, "finished investigation closed");
                    continue;
                }
                if working == investigation.working && reply == investigation.reply {
                    continue;
                }
                let reply_html = if reply == investigation.reply {
                    investigation.reply_html.clone()
                } else {
                    gem_github::render(&investigation.pull_request_url, &reply)
                        .await
                        .unwrap_or_default()
                };
                let _files = FILES.lock().await;
                if let Some(mut current) =
                    load().await.into_iter().find(|i| i.id == investigation.id)
                {
                    current.working = working;
                    current.reply = reply;
                    current.reply_html = reply_html;
                    save(&current).await;
                }
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

// Starts an agent that proposes on `subject` through `gem-agent`. Returns the thread's id.
async fn start_proposing(title: &str, prompt: &str, subject: &Subject) -> Result<String, String> {
    let config = mcp::agent_config(subject).await?;
    let started = t3::start(title, prompt, Some(&config)).await;
    if started.is_err() {
        Tokens::here().revoke(subject.id()).await;
    }
    started
}

fn now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

// An investigation started at `nanos` since the epoch.
fn id(nanos: u128) -> String {
    format!("{nanos:x}")
}

// Saves an investigation started at `nanos` since the epoch, on the pull requests `prs`, the one it
// started from first.
async fn track(nanos: u128, title: String, prs: Vec<String>, thread_id: String) {
    let _files = FILES.lock().await;
    save(&Investigation {
        id: id(nanos),
        title,
        pull_request_url: prs[0].clone(),
        pull_request_urls: prs,
        thread_id,
        started_at: Some(Timestamp {
            seconds: (nanos / 1_000_000_000) as i64,
            nanos: (nanos % 1_000_000_000) as i32,
        }),
        working: true,
        ..Default::default()
    })
    .await;
}

fn prompt(pr: &str, failure: &CheckFailure, rerun_summary: &str) -> String {
    let checks: String = failure
        .checks
        .iter()
        .map(|check| {
            let run = match check.workflow_run_id {
                0 => String::new(),
                id => format!(", GitHub Actions run {id}"),
            };
            let excerpt = match check.excerpt.trim_end() {
                "" => String::new(),
                excerpt => format!("\n\n```\n{excerpt}\n```\n"),
            };
            format!(
                "- {} (check {}, {}{run}): {}{excerpt}\n",
                check.name, check.id, check.conclusion, check.url
            )
        })
        .collect();
    format!(
        "Checks failed on {head}, the head commit of {login}'s pull request {pr}:

{checks}
Another agent proposed rerunning them: \"{rerun_summary}\". {login} reran them so the pull request \
is not held up, but does not take \"flaky\" for an answer. Find the root cause:
- Read what earlier investigations found: the `search_investigations` tool of the `{server}` MCP \
server finds them by the failing test's name or words of the error. Ones that fail the same way \
may already know the root cause, or a fix that did not hold.
- Read why it failed: `gh run view <run> --log-failed -R <owner>/<repo>` prints a GitHub Actions \
run's failed steps.
- Find how often it fails, and where: `gh run list -R <owner>/<repo> --workflow <workflow> -L 100` \
lists the workflow's recent runs on the base branch and other pull requests. Read the failed ones \
to see whether they fail the same test in the same way, and since when.
- Read the failing test and the code it runs, in a clone under a temporary directory, and work out \
what makes it fail only sometimes: timing, ordering, shared state, the runner's environment, a \
dependency's version. Reproduce it locally when you can.
- Check whether the base branch already fixed it: read `git log <merge base>..origin/<base branch>` \
for commits that touch the failing test or the code it runs.

Your reply is what {login} reads about this investigation, in an app that shows only your latest \
reply: say what the root cause is and the evidence for it, such as links to the runs that fail the \
same way, in a few short paragraphs of markdown. Link the pull request of each earlier \
investigation into the same failure, with what it found.

Then propose what {login} should do about it. A proposal can:
- hand the fix to another agent, with `follow_up`: such as a pull request that fixes the test or \
the code on the base branch. Write a prompt for a T3 Code agent. It must stand alone: the \
repositories (checked out under ~/code), the branch to build on, what to change and \
why, how to verify it, and that it opens a pull request with the fix whose description links \
{pr}, where the test failed. The agent must be able to finish without anyone else reviewing, \
merging or releasing something first.
- change the code of {pr}, with `patch` and `message`, when the pull request itself causes it: \
clone the repository into a temporary directory, run `gh pr checkout <number>` in it, edit, and \
pass what `git diff` prints as `patch`. Never commit or push.
- change the code of another pull request in {pr}'s cluster, with `other_patches`, when that pull \
request causes it, such as one {pr} is stacked on: make the patch the same way. The `get_cluster` \
tool of the `{server}` MCP server lists the cluster's pull requests.

Record each proposal by calling the `propose` tool of the `{server}` MCP server, with `pr` {pr}, a \
`summary` of one line saying what approving does, and its steps. It checks the proposal and \
returns an error with the reason when something is wrong: fix it and call it again. Call it once \
per proposal. If the failure needs nothing from {login}, propose nothing and say why in your reply.

If answering well needs something these steps cannot do, call `propose` with `request_tool` \
instead of steps: markdown describing the missing tool (what it would do and why this failure \
needs it), with a `summary` of \"<tool name>: <one line>\".

{login} may message you afterwards in this thread. Answer them the same way: your latest reply \
says where things stand, and you can record more proposals the same way.

Never push, comment, rerun or change anything on GitHub yourself: {login} approves proposals \
later.",
        head = failure.head_oid,
        login = login(),
        server = mcp::AGENT_NAME,
    )
}

// The closed investigations in `dir` whose title or reply has every word of `query`, ignoring case,
// newest first, as JSON.
pub async fn search(dir: &Path, query: &str) -> String {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<Investigation> = (load_from(dir).await.into_iter())
        .filter(|investigation| {
            let text = format!("{}\n{}", investigation.title, investigation.reply).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .collect();
    found.sort_by_key(|investigation| std::cmp::Reverse(investigation.id.clone()));
    let found: Vec<serde_json::Value> = (found.into_iter())
        .map(|investigation| {
            serde_json::json!({
                "title": investigation.title,
                "pullRequestUrls": match investigation.pull_request_urls.is_empty() {
                    true => vec![investigation.pull_request_url],
                    false => investigation.pull_request_urls,
                },
                "startedAt": investigation.started_at,
                "reply": investigation.reply,
            })
        })
        .collect();
    serde_json::to_string_pretty(&found).unwrap()
}

fn path(id: &str) -> PathBuf {
    Path::new(DIR).join(format!("{id}.json"))
}

async fn load() -> Vec<Investigation> {
    load_from(Path::new(DIR)).await
}

async fn load_from(dir: &Path) -> Vec<Investigation> {
    let mut investigations = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return investigations;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(json) = tokio::fs::read(entry.path()).await
            && let Ok(investigation) = serde_json::from_slice(&json)
        {
            investigations.push(investigation);
        }
    }
    investigations
}

async fn save(investigation: &Investigation) {
    let _ = tokio::fs::create_dir_all(DIR).await;
    let json = serde_json::to_vec_pretty(investigation).unwrap();
    let _ = tokio::fs::write(path(&investigation.id), json).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_file_saved_as_an_investigation_message() {
        let json = r#"{
            "id": "18f",
            "title": "Investigate test on gem#1",
            "pullRequestUrl": "https://github.com/u9g/gem/pull/1",
            "checkFailure": {"id": "c", "headOid": "abc", "checks": []},
            "rerunSummary": "flaky",
            "threadId": "t",
            "startedAt": "2026-10-26T12:00:00Z",
            "working": true
        }"#;
        let investigation: Investigation = serde_json::from_str(json).unwrap();
        assert_eq!(investigation.pull_request_url, "https://github.com/u9g/gem/pull/1");
        assert!(investigation.pull_request_urls.is_empty());
        assert_eq!(investigation.thread_id, "t");
        assert!(investigation.working);
        assert_eq!(investigation.started_at.unwrap().seconds, 1_793_016_000);
    }

    #[tokio::test]
    async fn search_finds_closed_investigations_with_every_word_newest_first() {
        let dir = crate::tokens::tests::temp_dir("search");
        let write = |id: &str, title: &str, reply: &str| {
            let investigation = Investigation {
                id: id.into(),
                title: title.into(),
                pull_request_url: format!("https://github.com/o/r/pull/{id}"),
                reply: reply.into(),
                ..Default::default()
            };
            let json = serde_json::to_vec(&investigation).unwrap();
            std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
        };
        write("1", "Investigate test on r#1", "test_join times out: the room closes first.");
        write("2", "Investigate lint on r#2", "A new clippy lint.");
        write("3", "Investigate test on r#3", "TEST_JOIN times out again.");

        let found: serde_json::Value =
            serde_json::from_str(&search(&dir, "test_join timeS").await).unwrap();
        let urls: Vec<&str> = (found.as_array().unwrap().iter())
            .map(|f| f["pullRequestUrls"][0].as_str().unwrap())
            .collect();
        assert_eq!(urls, ["https://github.com/o/r/pull/3", "https://github.com/o/r/pull/1"]);
    }
}
