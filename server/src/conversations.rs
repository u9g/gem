use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
};

use pbjson_types::Timestamp;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{
    SearchedPullRequest, gh, login, proposals,
    proto::{CheckAgent, CheckFailure, Comment, PullRequestCluster},
    t3,
};

// Relative to the server's working directory, like the cluster names.
const HANDLED_FILE: &str = "handled-comments.json";
const CONCURRENT_AGENTS: usize = 4;

// Fetched only for PRs whose updatedAt or failing checks moved, which any new comment, review
// thread or failure does, and again while a failure waits on checks that still run: the whole
// query costs about one rate-limit point per PR, against two for the search of all PRs.
const CONVERSATIONS_QUERY: &str = r#"
query($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on PullRequest {
      url
      reviewThreads(first: 100) {
        nodes { id isResolved comments(last: 1) { nodes { id body url authorAssociation author { login __typename } } } }
      }
      comments(last: 1) { nodes { id body url authorAssociation author { login __typename } } }
      commits(last: 1) { nodes { commit { oid statusCheckRollup { contexts(first: 100) { nodes {
        __typename
        ... on CheckRun { id name status conclusion detailsUrl checkSuite { workflowRun { databaseId } } }
        ... on StatusContext { id context state targetUrl }
      } } } } } }
    }
  }
}
"#;

// An unresolved review thread's latest reply, or the PR's latest top-level comment, from a bot or
// a member of the PR's organization. It waits on the user unless it is the user's own. The head
// commit's failed checks wait on the user once none of its checks still run.
const CONVERSATIONS_JQ: &str = r#".data.nodes[]
| .commits.nodes[0].commit.oid as $head
| [.commits.nodes[0].commit.statusCheckRollup.contexts.nodes[]? | if .__typename == "CheckRun" then
    {id, name, conclusion: (.conclusion // ""), url: (.detailsUrl // ""), workflow_run_id: (.checkSuite.workflowRun.databaseId // 0), done: (.status == "COMPLETED")}
  else
    {id, name: .context, conclusion: .state, url: (.targetUrl // ""), workflow_run_id: 0, done: (.state != "PENDING" and .state != "EXPECTED")}
  end] as $checks
| [$checks[] | select(.conclusion | IN("FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ERROR")) | del(.done)] as $failed
| {
  url,
  settling: (($failed | length) > 0 and any($checks[]; .done | not)),
  waiting: ([
    (.reviewThreads.nodes[] | select(.isResolved | not) | {thread_id: .id, comment: .comments.nodes[0]}),
    (.comments.nodes[] | {thread_id: null, comment: .})
    | select(.comment.author.__typename == "Bot" or .comment.authorAssociation == "MEMBER" or .comment.authorAssociation == "OWNER")
    | .comment |= {id, body, url, author: (.author.login // "ghost")}
  ] + [
    select(($failed | length) > 0 and all($checks[]; .done))
    | {check_failure: {id: ([$head] + ([$failed[].id] | sort) | join(",")), head_oid: $head, checks: $failed}}
  ])
}"#;

#[derive(Deserialize)]
struct Fetched {
    url: String,
    // A check failed while others still run.
    settling: bool,
    waiting: Vec<Conversation>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum Conversation {
    // In the PR's top-level comments when `thread_id` is None.
    Comment {
        thread_id: Option<String>,
        comment: Comment,
    },
    Checks {
        check_failure: CheckFailure,
    },
}

impl Conversation {
    // What its proposals answer: they show only while this is still waiting on the user.
    fn id(&self) -> &str {
        match self {
            Conversation::Comment { comment, .. } => &comment.id,
            Conversation::Checks { check_failure } => &check_failure.id,
        }
    }
}

// The failed checks of the PR with node ID `pr`, while `id` is still their ID.
pub async fn check_failure(pr: &str, id: &str) -> Option<CheckFailure> {
    fetch(&[pr])
        .await
        .into_iter()
        .flat_map(|fetched| fetched.waiting)
        .find_map(|conversation| match conversation {
            Conversation::Checks { check_failure } if check_failure.id == id => Some(check_failure),
            _ => None,
        })
}

// The conversations waiting on the user on each PR, by GraphQL node ID.
async fn fetch(ids: &[&str]) -> Vec<Fetched> {
    let mut fetched = Vec::new();
    for chunk in ids.chunks(100) {
        let mut args = vec![
            "api".to_string(),
            "graphql".to_string(),
            "-f".to_string(),
            format!("query={CONVERSATIONS_QUERY}"),
            "--jq".to_string(),
            CONVERSATIONS_JQ.to_string(),
        ];
        for id in chunk {
            args.extend(["-f".to_string(), format!("ids[]={id}")]);
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let Some(stdout) = gh(&args).await else {
            continue;
        };
        for result in serde_json::Deserializer::from_slice(&stdout).into_iter::<Fetched>() {
            let Ok(result) = result else { break };
            fetched.push(result);
        }
    }
    fetched
}

#[derive(Clone)]
pub struct Conversations(Arc<Inner>);

// A PR's conversations as last fetched.
struct Known {
    // The PR's when the conversations were fetched.
    updated_at: Timestamp,
    checks_failing: bool,
    settling: bool,
    waiting: Vec<Conversation>,
}

struct Inner {
    // By PR URL.
    prs: RwLock<HashMap<String, Known>>,
    // Conversations an agent was queued for.
    handled: Mutex<HashSet<String>>,
    // Conversations an agent was started for: the persisted part of `handled`. Never cleared, so a
    // restart answers conversations that were only queued, and not the others.
    started: Mutex<HashSet<String>>,
    agents: Semaphore,
    // By the ID of the failed checks they answer, with their PR's URL, until they stop.
    check_agents: Mutex<HashMap<String, (String, CheckAgent)>>,
}

impl Conversations {
    pub async fn load() -> Self {
        let handled = match tokio::fs::read(HANDLED_FILE).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(_) => HashSet::new(),
        };
        Self(Arc::new(Inner {
            prs: RwLock::default(),
            handled: Mutex::new(handled.clone()),
            started: Mutex::new(handled),
            agents: Semaphore::new(CONCURRENT_AGENTS),
            check_agents: Mutex::default(),
        }))
    }

    // A running agent wins over a queued one, on a PR with failures from more than one head.
    pub async fn fill(&self, clusters: &mut [PullRequestCluster]) {
        let agents = self.0.check_agents.lock().await;
        for pr in clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
            pr.check_agent = (agents.values())
                .filter(|(url, _)| *url == pr.url)
                .map(|(_, agent)| agent)
                .min_by_key(|agent| agent.queued)
                .cloned();
        }
    }

    // IDs of every conversation waiting on the user.
    pub async fn waiting(&self) -> HashSet<String> {
        let prs = self.0.prs.read().await;
        prs.values()
            .flat_map(|known| known.waiting.iter().map(|c| c.id().to_string()))
            .collect()
    }

    // Deletes the proposals on the PRs at `urls` and forgets that agents answered their waiting
    // conversations, so the next refresh answers them again.
    pub async fn restart(&self, urls: &[String]) {
        proposals::remove_on(urls).await;
        let ids: HashSet<String> = {
            let prs = self.0.prs.read().await;
            urls.iter()
                .filter_map(|url| prs.get(url))
                .flat_map(|known| known.waiting.iter().map(|c| c.id().to_string()))
                .collect()
        };
        {
            let mut started = self.0.started.lock().await;
            started.retain(|id| !ids.contains(id));
            let json = serde_json::to_vec_pretty(&*started).unwrap();
            let _ = tokio::fs::write(HANDLED_FILE, json).await;
        }
        self.0.handled.lock().await.retain(|id| !ids.contains(id));
    }

    pub async fn refresh(&self, prs: &[SearchedPullRequest]) {
        let changed: Vec<&SearchedPullRequest> = {
            let known = self.0.prs.read().await;
            prs.iter()
                .filter(|pr| {
                    known.get(&pr.url).is_none_or(|k| {
                        k.updated_at != pr.updated_at
                            || k.checks_failing != pr.checks_failing
                            || k.settling
                    })
                })
                .collect()
        };
        let ids: Vec<&str> = changed.iter().map(|pr| pr.id.as_str()).collect();
        let mut fetched = HashMap::new();
        for mut result in fetch(&ids).await {
            result.waiting.retain(|c| {
                !matches!(c, Conversation::Comment { comment, .. } if comment.author == login())
            });
            fetched.insert(result.url.clone(), result);
        }

        {
            let mut known = self.0.prs.write().await;
            known.retain(|url, _| prs.iter().any(|pr| &pr.url == url));
            for pr in &changed {
                if let Some(result) = fetched.remove(&pr.url) {
                    known.insert(
                        pr.url.clone(),
                        Known {
                            updated_at: pr.updated_at.clone(),
                            checks_failing: pr.checks_failing,
                            settling: result.settling,
                            waiting: result.waiting,
                        },
                    );
                }
            }
        }

        let heads = prs
            .iter()
            .map(|pr| (pr.url.as_str(), pr.head_oid.as_str()))
            .collect();
        let stale = proposals::remove_stale(&heads).await;
        if !stale.is_empty() {
            let mut started = self.0.started.lock().await;
            started.retain(|id| !stale.contains(id));
            let json = serde_json::to_vec_pretty(&*started).unwrap();
            let _ = tokio::fs::write(HANDLED_FILE, json).await;
        }

        let known = self.0.prs.read().await;
        let mut handled = self.0.handled.lock().await;
        handled.retain(|id| !stale.contains(id));
        for pr in prs {
            let Some(Known { waiting, .. }) = known.get(&pr.url) else {
                continue;
            };
            for conversation in waiting {
                if !handled.insert(conversation.id().to_string()) {
                    continue;
                }
                if let Conversation::Checks { check_failure } = conversation {
                    let agent = CheckAgent {
                        queued: true,
                        thread_url: String::new(),
                    };
                    (self.0.check_agents.lock().await)
                        .insert(check_failure.id.clone(), (pr.url.clone(), agent));
                }
                let this = self.clone();
                let pr = Target::new(pr);
                let conversation = conversation.clone();
                tokio::spawn(async move {
                    let _permit = this.0.agents.acquire().await.unwrap();
                    {
                        let mut started = this.0.started.lock().await;
                        started.insert(conversation.id().to_string());
                        let json = serde_json::to_vec_pretty(&*started).unwrap();
                        let _ = tokio::fs::write(HANDLED_FILE, json).await;
                    }
                    match &conversation {
                        Conversation::Comment { thread_id, comment } => {
                            answer(&pr, thread_id, comment).await
                        }
                        Conversation::Checks { check_failure } => {
                            fix(&pr, check_failure, &this.0.check_agents).await;
                            this.0.check_agents.lock().await.remove(&check_failure.id);
                        }
                    }
                });
            }
        }
    }
}

// What an agent needs to know about the PR it answers on.
struct Target {
    url: String,
    full_name: String,
    number: u32,
    title: String,
}

impl Target {
    fn new(pr: &SearchedPullRequest) -> Self {
        Self {
            url: pr.url.clone(),
            full_name: format!("{}/{}", pr.repository.owner, pr.repository.name),
            number: pr.number,
            title: pr.title.clone(),
        }
    }
}

async fn answer(pr: &Target, thread_id: &Option<String>, comment: &Comment) {
    let exe = env::current_exe().unwrap();
    let dir = env::current_dir().unwrap().join(proposals::DIR);
    let mut propose = format!(
        "{} propose --dir {} --pr {} --comment {}",
        exe.display(),
        dir.display(),
        pr.url,
        comment.id
    );
    let (where_, reply_kind, resolve) = match thread_id {
        Some(thread) => {
            propose += &format!(" --thread {thread}");
            (
                format!("in the review thread {thread}"),
                "a reply in the review thread",
                "\n- resolve the review thread, with --resolve-thread.",
            )
        }
        None => (
            "in the pull request's top-level comments".to_string(),
            "a top-level comment that quotes their comment above it",
            "",
        ),
    };
    let quoted: String = comment
        .body
        .lines()
        .map(|line| format!("> {line}\n"))
        .collect();

    let prompt = format!(
        "{author} is waiting on {login} {where_} on the pull request {url} (\"{title}\"). Their latest \
comment ({comment_url}):

{quoted}
Read the whole conversation and the pull request with `gh`, then work out how {login} should answer \
and propose it. A proposal can:
- change the code, with --patch and --message: clone {full_name} into a temporary directory, run \
`gh pr checkout {number}` in it, edit, and save `git diff` to a file. Never commit or push.
- reply as {login}, in the first person, with --reply: write the markdown to a file. It is posted as \
{reply_kind}. Open with what {login} is doing about the comment, then why: {login} reads only the \
first lines before approving, and they must not concede a suggestion the reply goes on to decline. \
Pin each link to the commit you read the lines at, so the lines it highlights are the ones you \
describe.{resolve}
- hand the work to another agent, with --follow-up: when the right fix is too big to review as \
one patch on this pull request, such as matching changes in other repositories, write a prompt \
for a T3 Code agent to a markdown file. It must stand alone: the repositories (checked out under \
~/code), the branches or pull requests to build on, what to change, and how to verify it. \
Approving starts that agent, so pair it with a reply saying the work follows. Prefer --patch \
when the change fits this pull request. The agent must be able to finish without anyone else \
reviewing, merging or releasing something first: when the fix waits on that, reply saying what \
it waits on instead.
- close the pull request without merging it, with --close-pull-request: when the conversation \
settles that it should not land, such as being superseded or no longer wanted. Pair it with a \
reply saying why.

Record each proposal with:

{propose} --summary \"<one line saying what approving does>\" [--patch <diff file> --message \"<commit message>\"] [--reply <markdown file>] [--follow-up <markdown file>]{resolve_flag} [--close-pull-request]

Approving a proposal runs its steps in that order. Run the command once per alternative when more \
than one answer is reasonable. It checks the proposal and exits non-zero with the reason when \
something is wrong: fix it and run it again. If the comment needs nothing from {login}, propose \
nothing.

If answering well needs something these steps cannot do, describe the missing tool in a markdown \
file (what it would do and why this conversation needs it) and record it with:

{propose} --summary \"<tool name: one line>\" --request-tool <markdown file>

Never push, comment, review or resolve anything on GitHub yourself: {login} approves proposals later.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread. Then reply with one line saying what you proposed.",
        author = comment.author,
        login = login(),
        url = pr.url,
        title = pr.title,
        comment_url = comment.url,
        full_name = pr.full_name,
        number = pr.number,
        resolve_flag = if resolve.is_empty() { "" } else { " [--resolve-thread]" },
    );
    t3::ask(
        &format!(
            "Answer {} on {}#{}",
            comment.author, pr.full_name, pr.number
        ),
        &prompt,
    )
    .await;
}

async fn fix(
    pr: &Target,
    failure: &CheckFailure,
    agents: &Mutex<HashMap<String, (String, CheckAgent)>>,
) {
    if let Some((_, agent)) = agents.lock().await.get_mut(&failure.id) {
        agent.queued = false;
    }
    let exe = env::current_exe().unwrap();
    let dir = env::current_dir().unwrap().join(proposals::DIR);
    let propose = format!(
        "{} propose --dir {} --pr {} --check-failure {}",
        exe.display(),
        dir.display(),
        pr.url,
        failure.id
    );
    let checks: String = failure
        .checks
        .iter()
        .map(|check| {
            let run = match check.workflow_run_id {
                0 => String::new(),
                id => format!(", GitHub Actions run {id}"),
            };
            format!(
                "- {} (check {}, {}{run}): {}\n",
                check.name, check.id, check.conclusion, check.url
            )
        })
        .collect();

    let prompt = format!(
        "Checks failed on {head}, the head commit of {login}'s pull request {url} (\"{title}\"):

{checks}
Find the root cause of each failure before you choose an answer:
- Read why it failed: `gh run view <run> --log-failed -R {full_name}` prints a GitHub Actions \
run's failed steps.
- Check whether this pull request caused it: compare the failing test or step with what \
`gh pr diff {number} -R {full_name}` changes.
- Check whether it fails without this pull request: `gh run list -R {full_name} --workflow \
<workflow> -L 50` lists the workflow's recent runs on the base branch and other pull requests. \
Read the failed ones to see whether they fail the same test in the same way.
- Check whether the base branch already fixed it: in a clone of {full_name}, read \
`git log <merge base>..origin/<base branch>` for commits that touch the failing test or the code \
it runs.
Then work out what {login} should do about it and propose it.

A proposal answers the checks you pass --excerpt <check> <file> for, with the log lines that show \
why that check failed (the error itself, at most 10, no setup or timestamps) saved to the file: \
{login} reads your answer next to them. Answer checks together when they fail for the same \
reason, and separately when they need different answers, such as a fix for one and a rerun for \
another. A proposal can:
- fix the code, with --patch and --message: clone {full_name} into a temporary directory, run \
`gh pr checkout {number}` in it, edit, and save `git diff` to a file. Run the failing check's \
command locally when you can, to show the fix works. Never commit or push.
- rerun the failed jobs, with --rerun: when the failure is flaky or comes from outside the pull \
request, such as a network error, a broken runner or a test that fails the same way on the base \
branch, and running it again is likely to pass. Only GitHub Actions runs can be rerun.
- hand the work to another agent, with --follow-up: when the right fix is too big to review as \
one patch on this pull request, such as matching changes in other repositories, or when the base \
branch already fixed it and the pull request needs a rebase onto it. Write a prompt for a T3 Code \
agent to a markdown file. It must stand alone: the repositories (checked out under ~/code), the \
branches or pull requests to build on, what to change and why (such as the commit that fixed it), \
and how to verify it. The agent must be able to finish without anyone else reviewing, merging \
or releasing something first: when the fix waits on that, such as another pull request that \
needs review or a dependency that has not been released, propose nothing for that check.

Record each proposal with:

{propose} --summary \"<one line saying what approving does>\" --excerpt <check> <file>... [--patch <diff file> --message \"<commit message>\"] [--follow-up <markdown file>] [--rerun]

Run the command once per answer, and once per alternative when more than one answer is \
reasonable. It checks the \
proposal and exits non-zero with the reason when something is wrong: fix it and run it again. If \
the failure needs nothing from {login}, propose nothing.

If answering well needs something these steps cannot do, describe the missing tool in a markdown \
file (what it would do and why this failure needs it) and record it with:

{propose} --summary \"<tool name: one line>\" --excerpt <check> <file>... --request-tool <markdown file>

Never push, comment, rerun or change anything on GitHub yourself: {login} approves proposals later.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread. Then reply with one line saying what you proposed.",
        head = failure.head_oid,
        login = login(),
        url = pr.url,
        title = pr.title,
        full_name = pr.full_name,
        number = pr.number,
    );
    let title = format!("Fix checks on {}#{}", pr.full_name, pr.number);
    let thread_id = match t3::start(&title, &prompt).await {
        Ok(thread_id) => thread_id,
        Err(e) => return eprintln!("{title}: {e}"),
    };
    if let Some((_, agent)) = agents.lock().await.get_mut(&failure.id) {
        agent.thread_url = t3::url(&thread_id).unwrap_or_default();
    }
    t3::wait(&thread_id).await;
}
