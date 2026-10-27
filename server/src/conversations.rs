use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use pbjson_types::Timestamp;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{
    SearchedPullRequest,
    agents::Running,
    gh, login, mcp, proposals,
    proto::{CheckFailure, Comment, PullRequestCluster},
    t3,
    tokens::{Subject, Tokens},
};

// Relative to the server's working directory, like the cluster names.
const HANDLED_FILE: &str = "handled-comments.json";
// Why each agent answering failed checks proposed nothing, by the ID of the checks it answered.
const NO_PROPOSAL_FILE: &str = "no-proposal-reasons.json";
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
        nodes { id isResolved comments(last: 1) { nodes { id body url authorAssociation author { login __typename } path line diffHunk } } }
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
    | .comment |= {id, body, url, author: (.author.login // "ghost"), path: (.path // ""), line: (.line // 0), diffHunk: (.diffHunk // "")}
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

    // The title and prompt of the agent answering it on `pr`.
    fn prompt(&self, pr: &Target) -> (String, String) {
        match self {
            Conversation::Comment { thread_id, comment } => answer(pr, thread_id, comment),
            Conversation::Checks { check_failure } => fix(pr, check_failure),
        }
    }

    // What the agent answering it on the PR at `pr` proposes on.
    fn subject(&self, pr: &str) -> Subject {
        match self {
            Conversation::Comment { thread_id, comment } => Subject::Comment {
                pr: pr.to_string(),
                comment: comment.id.clone(),
                thread: thread_id.clone(),
            },
            Conversation::Checks { check_failure } => Subject::CheckFailure {
                pr: pr.to_string(),
                id: check_failure.id.clone(),
            },
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

// The conversations waiting on the user on each PR, by GraphQL node ID. gh exits non-zero when any
// one node errors, e.g. a PR in a repository behind SAML enforcement the token is not authorized
// for, and then prints the raw response instead of applying --jq, so a chunk that fails is asked for
// one PR at a time: only the PR that errors is lost, not the chunk's others.
async fn fetch(ids: &[&str]) -> Vec<Fetched> {
    let mut fetched = Vec::new();
    for chunk in ids.chunks(100) {
        match fetch_chunk(chunk).await {
            Some(results) => fetched.extend(results),
            None if chunk.len() > 1 => {
                for &id in chunk {
                    fetched.extend(fetch_chunk(&[id]).await.into_iter().flatten());
                }
            }
            None => {}
        }
    }
    fetched
}

// The conversations of the PRs with node IDs `ids`, in one request, or None when it fails.
async fn fetch_chunk(ids: &[&str]) -> Option<Vec<Fetched>> {
    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={CONVERSATIONS_QUERY}"),
        "--jq".to_string(),
        CONVERSATIONS_JQ.to_string(),
    ];
    for id in ids {
        args.extend(["-f".to_string(), format!("ids[]={id}")]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout = gh("conversations", &args).await?;
    Some(
        serde_json::Deserializer::from_slice(&stdout)
            .into_iter::<Fetched>()
            .map_while(Result::ok)
            .collect(),
    )
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
    target: Target,
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
    // By the ID of the conversation they answer, until they stop or the user closes them.
    answering: Mutex<HashMap<String, Answering>>,
    // Persisted to NO_PROPOSAL_FILE.
    no_proposal: Mutex<HashMap<String, String>>,
}

// An agent answering a conversation, from when it is queued.
struct Answering {
    pr: String,
    title: String,
    // None while it is queued.
    thread_id: Option<String>,
    // Seconds since the epoch.
    queued_at: i64,
}

impl Conversations {
    pub async fn load() -> Self {
        let handled = match tokio::fs::read(HANDLED_FILE).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(_) => HashSet::new(),
        };
        let no_proposal = match tokio::fs::read(NO_PROPOSAL_FILE).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(_) => HashMap::new(),
        };
        Self(Arc::new(Inner {
            prs: RwLock::default(),
            handled: Mutex::new(handled.clone()),
            started: Mutex::new(handled),
            agents: Semaphore::new(CONCURRENT_AGENTS),
            answering: Mutex::default(),
            no_proposal: Mutex::new(no_proposal),
        }))
    }

    // The agents answering conversations, by the ID of the conversation each answers. One without
    // a thread is queued.
    pub async fn running(&self) -> Vec<(String, Running)> {
        (self.0.answering.lock().await.iter())
            .map(|(id, answering)| {
                let running = Running {
                    thread_id: answering.thread_id.clone(),
                    title: answering.title.clone(),
                    question: false,
                    started_at: answering.queued_at,
                    pull_request_urls: vec![answering.pr.clone()],
                };
                (id.clone(), running)
            })
            .collect()
    }

    // The thread of the agent answering the conversation `id`, once it started.
    pub async fn thread_of(&self, id: &str) -> Option<String> {
        (self.0.answering.lock().await.get(id)).and_then(|answering| answering.thread_id.clone())
    }

    // Stops showing the agent answering the conversation `id`. A queued one still starts, but does
    // not show.
    pub async fn forget(&self, id: &str) {
        self.0.answering.lock().await.remove(id);
    }

    // Every conversation waiting on the user by ID, with its comment as last fetched when it is one:
    // where in the PR's current diff a review comment now sits.
    pub async fn waiting(&self) -> HashMap<String, Option<Comment>> {
        let prs = self.0.prs.read().await;
        (prs.values().flat_map(|known| &known.waiting))
            .map(|c| {
                let comment = match c {
                    Conversation::Comment { comment, .. } => Some(comment.clone()),
                    Conversation::Checks { .. } => None,
                };
                (c.id().to_string(), comment)
            })
            .collect()
    }

    // Deletes the proposals on the PRs at `urls`, forgets that agents answered their waiting
    // conversations, and queues new agents for them.
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
        {
            let mut no_proposal = self.0.no_proposal.lock().await;
            no_proposal.retain(|id, _| !ids.contains(id));
            let json = serde_json::to_vec_pretty(&*no_proposal).unwrap();
            let _ = tokio::fs::write(NO_PROPOSAL_FILE, json).await;
        }
        self.answer_waiting().await;
    }

    // Each PR's reason its failed checks have no proposal, while they still wait on the user.
    pub async fn fill(&self, clusters: &mut [PullRequestCluster]) {
        let prs = self.0.prs.read().await;
        let no_proposal = self.0.no_proposal.lock().await;
        for pr in clusters.iter_mut().flat_map(|c| c.pull_requests.iter_mut()) {
            pr.no_proposal_reason = (prs.get(&pr.url).into_iter())
                .flat_map(|known| &known.waiting)
                .find_map(|c| no_proposal.get(c.id()))
                .cloned()
                .unwrap_or_default();
        }
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
                            target: Target::new(pr),
                        },
                    );
                }
            }
            // An agent's token goes once what it answers no longer waits on the user, when its
            // proposals no longer show. A PR whose fetch failed keeps its agents' tokens.
            Tokens::here()
                .retain(|subject| match subject {
                    Subject::Investigation { .. } => true,
                    Subject::Comment { pr, .. } | Subject::CheckFailure { pr, .. } => {
                        prs.iter().any(|open| open.url == *pr)
                            && known.get(pr).is_none_or(|known| {
                                known.waiting.iter().any(|c| c.id() == subject.id())
                            })
                    }
                })
                .await;
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

        self.0.handled.lock().await.retain(|id| !stale.contains(id));
        self.answer_waiting().await;
    }

    // Queues an agent for each waiting conversation that no agent was queued for.
    async fn answer_waiting(&self) {
        let known = self.0.prs.read().await;
        let mut handled = self.0.handled.lock().await;
        for pr in known.values() {
            for conversation in &pr.waiting {
                if !handled.insert(conversation.id().to_string()) {
                    continue;
                }
                let (title, _) = conversation.prompt(&pr.target);
                let answering = Answering {
                    pr: pr.target.url.clone(),
                    title,
                    thread_id: None,
                    queued_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |since| since.as_secs() as i64),
                };
                (self.0.answering.lock().await).insert(conversation.id().to_string(), answering);
                let this = self.clone();
                let pr = pr.target.clone();
                let conversation = conversation.clone();
                tokio::spawn(async move {
                    let _permit = this.0.agents.acquire().await.unwrap();
                    {
                        let mut started = this.0.started.lock().await;
                        started.insert(conversation.id().to_string());
                        let json = serde_json::to_vec_pretty(&*started).unwrap();
                        let _ = tokio::fs::write(HANDLED_FILE, json).await;
                    }
                    let id = conversation.id();
                    let (title, prompt) = conversation.prompt(&pr);
                    let subject = conversation.subject(&pr.url);
                    let started = match mcp::agent_config(&subject).await {
                        Ok(config) => t3::start(&title, &prompt, Some(&config)).await,
                        Err(error) => Err(error),
                    };
                    match started {
                        Ok(thread_id) => {
                            if let Some(answering) = this.0.answering.lock().await.get_mut(id) {
                                answering.thread_id = Some(thread_id.clone());
                            }
                            t3::wait(&thread_id).await;
                            if let Conversation::Checks { .. } = &conversation
                                && !proposals::exist_for(id).await
                                && let Some((_, reply)) = t3::show(&thread_id).await
                            {
                                let mut no_proposal = this.0.no_proposal.lock().await;
                                no_proposal.insert(id.to_string(), reply.trim().to_string());
                                let json = serde_json::to_vec_pretty(&*no_proposal).unwrap();
                                let _ = tokio::fs::write(NO_PROPOSAL_FILE, json).await;
                            }
                        }
                        Err(e) => eprintln!("{title}: {e}"),
                    }
                    this.0.answering.lock().await.remove(id);
                });
            }
        }
    }
}

// What an agent needs to know about the PR it answers on.
#[derive(Clone)]
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

// The title and prompt of the agent answering `comment`.
fn answer(pr: &Target, thread_id: &Option<String>, comment: &Comment) -> (String, String) {
    let (where_, reply_kind, resolve) = match thread_id {
        Some(thread) => (
            format!("in the review thread {thread}"),
            "a reply in the review thread",
            "\n- resolve the review thread, with `resolve_thread`.",
        ),
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
- change the code, with `patch` and `message`: clone {full_name} into a temporary directory, run \
`gh pr checkout {number}` in it, edit, and pass what `git diff` prints as `patch`. Never commit or \
push.
- change other pull requests of this one's cluster, with `other_patches`: when the change belongs \
on them, alone or with `patch`, such as in a pull request that produces what this one consumes, \
or the same fix in another repository's port of this pull request. Before declining a concern this \
pull request cannot fix, check whether another pull request of the cluster causes it. Make each \
patch the same way, against its own pull request. The `get_cluster` tool of the `{server}` MCP \
server lists the cluster's pull requests.
- reply as {login}, in the first person, with `reply`: the markdown is posted as {reply_kind}. \
Open with what {login} is doing about the comment, then why: {login} reads only the first lines \
before approving, and they must not concede a suggestion the reply goes on to decline. Pin each \
link to the commit you read the lines at, so the lines it highlights are the ones you \
describe.{resolve}
- hand the work to another agent, with `follow_up`: when the right fix is too big to review as \
patches, such as matching changes in other repositories with no pull request in this cluster, \
write a prompt for a T3 Code agent. It must stand alone: the repositories (checked out under \
~/code), the branches or pull requests to build on, what to change, and how to verify it. \
Approving starts that agent, so pair it with a reply saying the work follows. Prefer `patch` and \
`other_patches` when the change fits this pull request and its cluster's. The agent must be able \
to finish without anyone else reviewing, merging or releasing something first: when the fix waits \
on that, reply saying what it waits on instead.
- close the pull request without merging it, with `close_pull_request`: when the conversation \
settles that it should not land, such as being superseded or no longer wanted. Pair it with a \
reply saying why.

Record each proposal by calling the `propose` tool of the `{server}` MCP server, with `pr` {url}, \
a `summary` of one line saying what approving does, and its steps. Approving a proposal runs all \
its steps. Call it once per alternative when more than one answer is reasonable. It checks the \
proposal and returns an error with the reason when something is wrong: fix it and call it again. \
If the comment needs nothing from {login}, propose nothing.

If answering well needs something these steps cannot do, call `propose` with `request_tool` \
instead of steps: markdown describing the missing tool (what it would do and why this \
conversation needs it), with a `summary` of \"<tool name>: <one line>\".

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
        server = mcp::AGENT_NAME,
    );
    let title = format!(
        "Answer {} on {}#{}",
        comment.author, pr.full_name, pr.number
    );
    (title, prompt)
}

// The title and prompt of the agent answering `failure`.
fn fix(pr: &Target, failure: &CheckFailure) -> (String, String) {
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
- Read what earlier investigations found: the `search_investigations` tool of the `{server}` MCP \
server finds them by the failing test's name or words of the error. Ones that fail the same way \
may already know the root cause, or a fix that did not hold.
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

A proposal answers the checks you give `excerpts` for, each with the check's ID and the log lines \
that show why it failed (the error itself, at most 10, no setup or timestamps): {login} reads your \
answer next to them. Answer checks together when they fail for the same reason, and separately \
when they need different answers, such as a fix for one and a rerun for another. A proposal can:
- fix the code, with `patch` and `message`: clone {full_name} into a temporary directory, run \
`gh pr checkout {number}` in it, edit, and pass what `git diff` prints as `patch`. Run the failing \
check's command locally when you can, to show the fix works. Never commit or push.
- change other pull requests of this one's cluster, with `other_patches`: when the fix belongs on \
them, alone or with `patch`, such as in a pull request this one depends on or that produces what \
this one consumes. Make each patch the same way, against its own pull request. The \
`get_cluster` tool of the `{server}` MCP server lists the cluster's pull requests.
- rerun the failed jobs, with `rerun`: when the failure is flaky or comes from outside the pull \
request, such as a network error, a broken runner or a test that fails the same way on the base \
branch, and running it again is likely to pass. Only GitHub Actions runs can be rerun.
- hand the work to another agent, with `follow_up`: when the right fix is too big to review as \
patches, such as matching changes in other repositories with no pull request in this cluster, or \
when the base branch already fixed it and the pull request needs a rebase onto it. Write a \
prompt for a T3 Code agent. It must stand alone: the repositories (checked out under ~/code), the \
branches or pull requests to build on, what to change and why (such as the commit that fixed \
it), and how to verify it. The agent must be able to finish without anyone else reviewing, merging or releasing \
something first: when the fix waits on that, such as another pull request that needs review or a \
dependency that has not been released, propose nothing for that check.

Record each proposal by calling the `propose` tool of the `{server}` MCP server, with `pr` {url}, \
a `summary` of one line saying what approving does, linking the pull request of each earlier \
investigation into the same failure, its `excerpts`, and its steps. Call it once per answer, and \
once per alternative when more than one answer is reasonable. It checks the proposal and returns \
an error with the reason when something is wrong: fix it and call it again. If the failure needs \
nothing from {login}, propose nothing.

If answering well needs something these steps cannot do, call `propose` with its `excerpts` and \
`request_tool` instead of steps: markdown describing the missing tool (what it would do and why \
this failure needs it), with a `summary` of \"<tool name>: <one line>\".

Never push, comment, rerun or change anything on GitHub yourself: {login} approves proposals later.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread. Then reply with one line saying what you proposed, or, when you proposed \
nothing, why, with the pull request of each earlier investigation into the same failure: {login} \
reads it next to the failed checks.",
        head = failure.head_oid,
        login = login(),
        url = pr.url,
        title = pr.title,
        full_name = pr.full_name,
        number = pr.number,
        server = mcp::AGENT_NAME,
    );
    let title = format!("Fix checks on {}#{}", pr.full_name, pr.number);
    (title, prompt)
}
