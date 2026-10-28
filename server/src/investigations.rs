use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gem_db::{Db, OptionalExtension, params};
use gem_json::{Error, FromJson, ToJson, Value, json};
use gem_log::{info, warn};
use gem_proto::Timestamp;

use crate::proto::{Agent, AgentState, CheckFailure, PullRequestCluster};
use crate::tokens::{Subject, Tokens};
use crate::{login, mcp, proposals, t3};
use gem_github::short;

// The most investigations a search returns, so its answer fits in an agent's context.
const SEARCH_LIMIT: usize = 10;
// Reading a thread is a local CLI call; only a changed reply reaches GitHub, to render it.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
// T3 Code does not settle threads when they finish, and unsettles one when it is sent a message,
// so an agent the user can message settles its thread after each answer.
const SETTLE: &str = "When you are done, call the t3_thread_organize tool with action \"settle\" \
and no threadId, to settle this thread. A message from the user unsettles it: settle it again \
each time you have answered.";

// An agent gem follows: an investigation, a follow-up, or the answer to the user's message about a
// cluster. Saved as JSON with the names of the `Investigation` message it was before every agent
// was one `Agent`, so ones saved then still load.
#[derive(Clone, Default)]
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
    // The failed checks it started from, with their log excerpts, as its prompt lists them, so
    // search finds it by the test or error even when its reply names neither. Empty for an agent
    // not started from failed checks.
    failed_checks: String,
    reply: String,
    reply_html: String,
}

// Every field, in camelCase.
impl ToJson for Investigation {
    fn to_json(&self) -> Value {
        json!({
            "id": (self.id),
            "title": (self.title),
            "pullRequestUrl": (self.pull_request_url),
            "pullRequestUrls": (self.pull_request_urls),
            "threadId": (self.thread_id),
            "startedAt": (self.started_at),
            "working": (self.working),
            "messaged": (self.messaged),
            "failedChecks": (self.failed_checks),
            "reply": (self.reply),
            "replyHtml": (self.reply_html),
        })
    }
}

// A missing field is its default.
impl FromJson for Investigation {
    fn from_json(value: &Value) -> Result<Investigation, Error> {
        if !value.is_object() {
            return Err(Error::new(format!(
                "expected an investigation, found {}",
                value.kind()
            )));
        }
        Ok(Investigation {
            id: value.field_or_default("id")?,
            title: value.field_or_default("title")?,
            pull_request_url: value.field_or_default("pullRequestUrl")?,
            pull_request_urls: value.field_or_default("pullRequestUrls")?,
            thread_id: value.field_or_default("threadId")?,
            started_at: value.field_or_default("startedAt")?,
            working: value.field_or_default("working")?,
            messaged: value.field_or_default("messaged")?,
            failed_checks: value.field_or_default("failedChecks")?,
            reply: value.field_or_default("reply")?,
            reply_html: value.field_or_default("replyHtml")?,
        })
    }
}

// Approves the rerun first, so the pull request is not held up by the investigation.
pub fn start(proposal_id: &str) -> Result<(), String> {
    let proposal = proposals::find(proposal_id).ok_or(format!("no proposal {proposal_id}"))?;
    let failure = (proposal.check_failure.clone())
        .filter(|_| proposal.rerun)
        .ok_or("only a rerun of failed checks can be investigated")?;
    proposals::approve(proposal_id, "", "", "")?;

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
    let thread_id = start_proposing(&title, &prompt, &subject)?;
    track(nanos, title, vec![pr.clone()], listed(&failure), thread_id);
    Ok(())
}

// Starts the agent an approved proposal hands `follow_up` to, and follows it like an
// investigation, so its replies show in gem and the user can message it.
pub fn follow_up(title: &str, pr: &str, follow_up: &str) -> Result<(), String> {
    let prompt = format!(
        "{follow_up}

Link {pr} in the description of any pull request you open, so gem groups the two together.

{login} follows this thread in an app that shows only your latest reply, so end each reply \
with where things stand, such as a link to the pull request you opened. {login} may message you \
afterwards in this thread.

{SETTLE}",
        login = login(),
    );
    let thread_id = t3::start(title, &prompt, None)?;
    track(
        now(),
        title.to_string(),
        vec![pr.to_string()],
        String::new(),
        thread_id,
    );
    Ok(())
}

// Starts an agent on the user's message about a cluster, and follows it like an investigation.
// The prompt carries the cluster as gem showed it when the user sent the message. The agent
// proposes what to do on GitHub rather than do it. Returns the investigation's id.
pub fn ask(cluster: &PullRequestCluster, message: &str) -> Result<String, String> {
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
- open a new pull request, with `open_pull_request`: such as a fix on a base branch. Make its \
`patch` the same way against the latest `origin/<base branch>`, and link the pull request the \
proposal is about in its description.
- reply as {login}, in the first person, with `reply`: the markdown is posted as a top-level \
comment on the pull request. Open with what {login} is doing, then why: {login} reads only the \
first lines before approving.
- hand the work to another agent, with `follow_up`: when the work is too big to review as \
patches or one new pull request, such as matching changes in other repositories with no pull \
request in the cluster, write a prompt for a T3 Code agent. It must stand alone: the repositories (checked out under \
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
thread, and you can record more proposals the same way.

{SETTLE}",
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
    let thread_id = start_proposing(&title, &prompt, &subject)?;
    track(nanos, title, prs, String::new(), thread_id);
    Ok(id(nanos))
}

// Archives a finished agent that answered `failure` on `pr`, which is never an open investigation,
// so later agents find what it found too.
pub fn archive(title: &str, pr: &str, failure: &CheckFailure, reply: &str) {
    let nanos = now();
    let investigation = Investigation {
        id: id(nanos),
        title: title.to_string(),
        pull_request_url: pr.to_string(),
        pull_request_urls: vec![pr.to_string()],
        failed_checks: listed(failure),
        started_at: Some(timestamp(nanos)),
        reply: reply.to_string(),
        ..Default::default()
    };
    insert(&investigation, true);
}

// Whether `id` is one of these agents.
pub fn has(id: &str) -> bool {
    load().iter().any(|investigation| investigation.id == id)
}

// Marked before sending, so the watcher never closes an agent that has a message it has not seen.
pub fn message(id: &str, message: &str) -> Result<(), String> {
    let mut thread_id = None;
    update(id, |investigation| {
        investigation.working = true;
        investigation.messaged = true;
        thread_id = Some(investigation.thread_id.clone());
        false
    });
    t3::send(&thread_id.ok_or(format!("no agent {id}"))?, message)
}

pub fn close(id: &str) {
    update(id, |_| true);
    closed(id);
}

// What goes with the investigation `id` once it is closed.
fn closed(id: &str) {
    proposals::remove_of_investigation(id);
    Tokens::here().revoke(id);
}

pub fn agents() -> Vec<Agent> {
    let mut proposals = proposals::of_investigations();
    (load().into_iter())
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
    std::thread::spawn(move || {
        loop {
            for investigation in load() {
                let Some((working, reply)) = t3::show(&investigation.thread_id) else {
                    continue;
                };
                // Read after `show`: an agent records its proposals before it stops working.
                if !working
                    && !investigation.messaged
                    && !proposals::of_investigations().contains_key(&investigation.id)
                {
                    // Rechecked as it closes: the user may have messaged it since it was loaded. The
                    // archive keeps what it found, which may have come since the last refresh.
                    let finished = update(&investigation.id, |current| {
                        if current.messaged {
                            return false;
                        }
                        current.reply = reply;
                        true
                    });
                    if finished {
                        closed(&investigation.id);
                        info!(
                            "finished investigation closed",
                            investigation = investigation.id
                        );
                    }
                    continue;
                }
                if working == investigation.working && reply == investigation.reply {
                    continue;
                }
                let reply_html = if reply == investigation.reply {
                    investigation.reply_html.clone()
                } else {
                    gem_github::render(&investigation.pull_request_url, &reply).unwrap_or_default()
                };
                update(&investigation.id, |current| {
                    current.working = working;
                    current.reply = reply;
                    current.reply_html = reply_html;
                    false
                });
            }
            std::thread::sleep(REFRESH_INTERVAL);
        }
    });
}

// Starts an agent that proposes on `subject` through `gem-agent`. Returns the thread's id.
fn start_proposing(title: &str, prompt: &str, subject: &Subject) -> Result<String, String> {
    let config = mcp::agent_config(subject)?;
    let started = t3::start(title, prompt, Some(&config));
    if started.is_err() {
        Tokens::here().revoke(subject.id());
    }
    started
}

fn now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn timestamp(nanos: u128) -> Timestamp {
    Timestamp {
        seconds: (nanos / 1_000_000_000) as i64,
        nanos: (nanos % 1_000_000_000) as i32,
    }
}

// An investigation started at `nanos` since the epoch.
fn id(nanos: u128) -> String {
    format!("{nanos:x}")
}

// Saves an investigation started at `nanos` since the epoch, on the pull requests `prs`, the one it
// started from first.
fn track(nanos: u128, title: String, prs: Vec<String>, failed_checks: String, thread_id: String) {
    let investigation = Investigation {
        id: id(nanos),
        title,
        pull_request_url: prs[0].clone(),
        pull_request_urls: prs,
        failed_checks,
        thread_id,
        started_at: Some(timestamp(nanos)),
        working: true,
        ..Default::default()
    };
    insert(&investigation, false);
}

// Each of `failure`'s checks, with its run and log excerpt, as a markdown list.
fn listed(failure: &CheckFailure) -> String {
    failure
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
        .collect()
}

fn prompt(pr: &str, failure: &CheckFailure, rerun_summary: &str) -> String {
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
- open a pull request with the fix, with `open_pull_request`: such as one that fixes the test or \
the code on the base branch. Clone the repository into a temporary directory, check out the \
latest `origin/<base branch>`, edit, verify the fix, and pass what `git diff` prints as its \
`patch`, with a title and a description that says what fails and why, and links {pr}, where the \
test failed. Never commit, push or open it yourself.
- hand the fix to another agent, with `follow_up`: only when it is too big for one \
`open_pull_request`, such as matching changes across repositories. Write a prompt for a T3 Code \
agent. It must stand alone: the repositories (checked out under ~/code), the branch to build on, \
what to change and why, how to verify it, and that it opens a pull request with the fix whose \
description links {pr}, where the test failed. The agent must be able to finish without anyone \
else reviewing, merging or releasing something first.
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
later.

{SETTLE}",
        checks = listed(failure),
        head = failure.head_oid,
        login = login(),
        server = mcp::AGENT_NAME,
    )
}

// The newest `SEARCH_LIMIT` closed investigations whose title, failed checks or reply has every
// word of `query`, ignoring case, as JSON, with how many older ones match too.
pub fn search(db: &Db, query: &str) -> String {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<Investigation> = (load_from(db, true).into_iter())
        .filter(|investigation| {
            let text = format!(
                "{}\n{}\n{}",
                investigation.title, investigation.failed_checks, investigation.reply
            )
            .to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .collect();
    found.sort_by_key(|investigation| std::cmp::Reverse(investigation.id.clone()));
    let older = found.len().saturating_sub(SEARCH_LIMIT);
    let found: Vec<Value> = (found.into_iter().take(SEARCH_LIMIT))
        .map(|investigation| {
            let pull_request_urls = match investigation.pull_request_urls.is_empty() {
                true => vec![investigation.pull_request_url],
                false => investigation.pull_request_urls,
            };
            json!({
                "title": (investigation.title),
                "pullRequestUrls": pull_request_urls,
                "startedAt": (investigation.started_at),
                "failedChecks": (investigation.failed_checks),
                "reply": (investigation.reply),
            })
        })
        .collect();
    let found = json!({ "investigations": found, "olderMatches": older });
    gem_json::to_string_pretty(&found)
}

// The open investigations.
fn load() -> Vec<Investigation> {
    load_from(crate::db(), false)
}

fn load_from(db: &Db, closed: bool) -> Vec<Investigation> {
    let investigations = db.with(|db| {
        db.prepare("SELECT investigation FROM investigations WHERE closed = ?1")?
            .query_map(params![closed], |row| row.get::<String>(0))?
            .collect::<Result<Vec<_>, _>>()
    });
    match investigations {
        Ok(investigations) => (investigations.iter())
            .filter_map(|json| gem_json::from_str(json).ok())
            .collect(),
        Err(error) => {
            warn!("cannot read investigations", error = error);
            Vec::new()
        }
    }
}

fn insert(investigation: &Investigation, closed: bool) {
    let saved = crate::db().with(|db| {
        db.execute(
            "INSERT INTO investigations (id, investigation, closed) VALUES (?1, ?2, ?3)",
            params![investigation.id, gem_json::to_string(investigation), closed],
        )
    });
    if let Err(error) = saved {
        warn!(
            "cannot save investigation",
            investigation = investigation.id,
            error = error
        );
    }
}

// Changes the investigation `id` while it is open, and closes it when `change` returns true, at
// once, so a refresh never brings back a closed investigation or undoes a message's `working`.
// Returns whether it closed it.
fn update(id: &str, change: impl FnOnce(&mut Investigation) -> bool) -> bool {
    let updated = crate::db().with(|db| {
        let tx = db.unchecked_transaction()?;
        let json: Option<String> = tx
            .query_row(
                "SELECT investigation FROM investigations WHERE id = ?1 AND closed = 0",
                params![id],
                |row| row.get::<String>(0),
            )
            .optional()?;
        let Some(mut investigation) = json.and_then(|json| gem_json::from_str(&json).ok()) else {
            return Ok(false);
        };
        let close = change(&mut investigation);
        tx.execute(
            "UPDATE investigations SET investigation = ?2, closed = ?3 WHERE id = ?1",
            params![id, gem_json::to_string(&investigation), close],
        )?;
        tx.commit()?;
        Ok(close)
    });
    updated.unwrap_or_else(|error| {
        warn!(
            "cannot save investigation",
            investigation = id,
            error = error
        );
        false
    })
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
        let investigation: Investigation = gem_json::from_str(json).unwrap();
        assert_eq!(
            investigation.pull_request_url,
            "https://github.com/u9g/gem/pull/1"
        );
        assert!(investigation.pull_request_urls.is_empty());
        assert_eq!(investigation.thread_id, "t");
        assert!(investigation.working);
        assert_eq!(investigation.started_at.unwrap().seconds, 1_793_016_000);
    }

    #[test]
    fn search_finds_closed_investigations_with_every_word_newest_first() {
        let db = crate::tokens::tests::db();
        let write = |id: &str, title: &str, failed_checks: &str, reply: &str, closed: bool| {
            let investigation = Investigation {
                id: id.into(),
                title: title.into(),
                pull_request_url: format!("https://github.com/o/r/pull/{id}"),
                failed_checks: failed_checks.into(),
                reply: reply.into(),
                ..Default::default()
            };
            let json = gem_json::to_string(&investigation);
            db.with(|db| {
                db.execute(
                    "INSERT INTO investigations (id, investigation, closed) VALUES (?1, ?2, ?3)",
                    params![id, json, closed],
                )
            })
            .unwrap();
        };
        write(
            "1",
            "Investigate test on r#1",
            "",
            "test_join times out: the room closes first.",
            true,
        );
        write(
            "2",
            "Investigate lint on r#2",
            "",
            "A new clippy lint.",
            true,
        );
        write(
            "3",
            "Investigate test on r#3",
            "TEST_JOIN: timed out",
            "The room closes first.",
            true,
        );
        write(
            "4",
            "Investigate test on r#4",
            "",
            "test_join still times out.",
            false,
        );
        let query = "test_join time";

        let found = gem_json::parse(&search(&db, query)).unwrap();
        let urls: Vec<&str> = (found["investigations"].as_array().unwrap().iter())
            .map(|f| f["pullRequestUrls"][0].as_str().unwrap())
            .collect();
        assert_eq!(
            urls,
            [
                "https://github.com/o/r/pull/3",
                "https://github.com/o/r/pull/1"
            ]
        );
    }
}
