use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pbjson_types::Timestamp;
use tokio::sync::Mutex;

use crate::proto::{CheckFailure, Investigation, ListOpenPullRequestsResponse, PullRequestCluster};
use crate::{login, mcp, proposals, t3};
use gem_github::short;

// One `<id>.json` file per open investigation, relative to the server's working directory.
const DIR: &str = "investigations";
// Reading a thread is a local CLI call; only a changed reply reaches GitHub, to render it.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

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
    proposals::approve(proposal_id, "", "").await?;

    let pr = &proposal.pull_request_url;
    let (repository, number) = short(pr);
    let checks: Vec<&str> = failure.checks.iter().map(|c| c.name.as_str()).collect();
    let title = format!("Investigate {} on {repository}#{number}", checks.join(", "));
    let nanos = now();
    let prompt = prompt(&id(nanos), pr, &failure, &proposal.summary);
    let thread_id = t3::start(&title, &prompt).await?;
    track(nanos, title, pr, Some(failure), proposal.summary, thread_id).await;
    Ok(())
}

// Starts the agent an approved proposal hands `follow_up` to, and follows it like an
// investigation, so its replies show in gem and the user can message it.
pub async fn follow_up(title: &str, pr: &str, follow_up: &str) -> Result<(), String> {
    let prompt = format!(
        "{follow_up}

{login} follows this thread in an app that shows only your latest reply, so end each reply \
with where things stand, such as a link to the pull request you opened. {login} may message you \
afterwards in this thread.",
        login = login(),
    );
    let thread_id = t3::start(title, &prompt).await?;
    track(now(), title.to_string(), pr, None, String::new(), thread_id).await;
    Ok(())
}

// Starts an agent on the user's message about a cluster, and follows it like an investigation.
// The prompt carries the cluster, as the agent may run out of reach of gem's MCP server, such as
// in Claude Cloud. Returns the investigation's id.
pub async fn ask(cluster: &PullRequestCluster, message: &str) -> Result<String, String> {
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
propose answering. Where gem's MCP server is available, its `get_cluster` tool, passed any of \
those URLs, has the cluster as gem shows it now. Use `gh` for anything more, such as diffs, \
comments and check logs.

```json
{snapshot}
```

{login}'s message:

{message}

{login} follows this thread in an app that shows only your latest reply, so end each reply with \
where things stand, such as a link to anything you opened. {login} may message you afterwards in \
this thread.",
        login = login(),
        snapshot = mcp::cluster_json(cluster),
    );
    let thread_id = t3::start(&title, &prompt).await?;
    let nanos = now();
    track(nanos, title, &first.url, None, String::new(), thread_id).await;
    Ok(id(nanos))
}

pub async fn message(id: &str, message: &str) -> Result<(), String> {
    let investigation = load()
        .await
        .into_iter()
        .find(|i| i.id == id)
        .ok_or(format!("no investigation {id}"))?;
    t3::send(&investigation.thread_id, message).await?;
    let _files = FILES.lock().await;
    if let Some(mut investigation) = load().await.into_iter().find(|i| i.id == id) {
        investigation.working = true;
        save(&investigation).await;
    }
    Ok(())
}

pub async fn close(id: &str) {
    let _files = FILES.lock().await;
    let _ = tokio::fs::remove_file(path(id)).await;
    proposals::remove_of_investigation(id).await;
}

pub async fn fill(response: &mut ListOpenPullRequestsResponse) {
    let mut proposals = proposals::of_investigations().await;
    let mut investigations = load().await;
    for investigation in &mut investigations {
        investigation.proposals = proposals.remove(&investigation.id).unwrap_or_default();
    }
    investigations
        .sort_by_key(|i| std::cmp::Reverse(i.started_at.as_ref().map(|t| (t.seconds, t.nanos))));
    response.investigations = investigations;
}

// Keeps each investigation's `working` and reply up to date with its thread.
pub fn watch() {
    tokio::spawn(async move {
        loop {
            for investigation in load().await {
                let Some((working, reply)) = t3::show(&investigation.thread_id).await else {
                    continue;
                };
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

// Saves an investigation started at `nanos` since the epoch.
async fn track(
    nanos: u128,
    title: String,
    pr: &str,
    check_failure: Option<CheckFailure>,
    rerun_summary: String,
    thread_id: String,
) {
    let _files = FILES.lock().await;
    save(&Investigation {
        id: id(nanos),
        title,
        pull_request_url: pr.to_string(),
        check_failure,
        rerun_summary,
        thread_id,
        started_at: Some(Timestamp {
            seconds: (nanos / 1_000_000_000) as i64,
            nanos: (nanos % 1_000_000_000) as i32,
        }),
        working: true,
        reply: String::new(),
        reply_html: String::new(),
        proposals: Vec::new(),
    })
    .await;
}

fn prompt(id: &str, pr: &str, failure: &CheckFailure, rerun_summary: &str) -> String {
    let exe = env::current_exe().unwrap();
    let dir = env::current_dir().unwrap().join(proposals::DIR);
    let propose = format!(
        "{} propose --dir {} --pr {pr} --investigation {id}",
        exe.display(),
        dir.display(),
    );
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
same way, in a few short paragraphs of markdown.

Then propose what {login} should do about it. A proposal can:
- hand the fix to another agent, with --follow-up: such as a pull request that fixes the test or \
the code on the base branch. Write a prompt for a T3 Code agent to a markdown file. It must stand \
alone: the repositories (checked out under ~/code), the branch to build on, what to change and \
why, how to verify it, and that it opens a pull request. The agent must be able to finish without \
anyone else reviewing, merging or releasing something first.
- change the code of {pr}, with --patch and --message, when the pull request itself causes it: \
clone the repository into a temporary directory, run `gh pr checkout <number>` in it, edit, and \
save `git diff` to a file. Never commit or push.

Record each proposal with:

{propose} --summary \"<one line saying what approving does>\" [--follow-up <markdown file>] [--patch <diff file> --message \"<commit message>\"]

It checks the proposal and exits non-zero with the reason when something is wrong: fix it and run \
it again. Run it once per proposal. If the failure needs nothing from {login}, propose nothing and \
say why in your reply.

If answering well needs something these steps cannot do, describe the missing tool in a markdown \
file (what it would do and why this failure needs it) and record it with:

{propose} --summary \"<tool name: one line>\" --request-tool <markdown file>

{login} may message you afterwards in this thread. Answer them the same way: your latest reply \
says where things stand, and you can record more proposals with the same command.

Never push, comment, rerun or change anything on GitHub yourself: {login} approves proposals \
later.",
        head = failure.head_oid,
        login = login(),
    )
}

fn path(id: &str) -> PathBuf {
    Path::new(DIR).join(format!("{id}.json"))
}

async fn load() -> Vec<Investigation> {
    let mut investigations = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(DIR).await else {
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
