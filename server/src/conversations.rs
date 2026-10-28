use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Condvar, Mutex, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use gem_db::{SqlValue, params};
use gem_json::{Error, FromJson, Value};
use gem_log::warn;
use gem_proto::Timestamp;

use crate::{
    SearchedPullRequest,
    agents::Running,
    gh, investigations, login, mcp, proposals,
    tokens::{Subject, Tokens},
};
use gem_proto::{CheckFailure, Comment, PullRequestCluster};

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

struct Fetched {
    url: String,
    // A check failed while others still run.
    settling: bool,
    waiting: Vec<Conversation>,
}

impl FromJson for Fetched {
    fn from_json(value: &Value) -> Result<Fetched, Error> {
        Ok(Fetched {
            url: value.field("url")?,
            settling: value.field("settling")?,
            waiting: value.field("waiting")?,
        })
    }
}

// Untagged: a comment when it has one, failed checks otherwise.
#[derive(Clone)]
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

impl FromJson for Conversation {
    fn from_json(value: &Value) -> Result<Conversation, Error> {
        let comment = || -> Result<Conversation, Error> {
            Ok(Conversation::Comment {
                thread_id: value.field("thread_id")?,
                comment: value.field("comment")?,
            })
        };
        if let Ok(conversation) = comment() {
            return Ok(conversation);
        }
        match value.field("check_failure") {
            Ok(check_failure) => Ok(Conversation::Checks { check_failure }),
            Err(_) => Err(Error::new(
                "data did not match any variant of untagged enum Conversation",
            )),
        }
    }
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
pub fn check_failure(pr: &str, id: &str) -> Option<CheckFailure> {
    fetch(&[pr])
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
fn fetch(ids: &[&str]) -> Vec<Fetched> {
    let mut fetched = Vec::new();
    for chunk in ids.chunks(100) {
        match fetch_chunk(chunk) {
            Some(results) => fetched.extend(results),
            None if chunk.len() > 1 => {
                for &id in chunk {
                    fetched.extend(fetch_chunk(&[id]).into_iter().flatten());
                }
            }
            None => {}
        }
    }
    fetched
}

// The conversations of the PRs with node IDs `ids`, in one request, or None when it fails.
fn fetch_chunk(ids: &[&str]) -> Option<Vec<Fetched>> {
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
    let stdout = gh("conversations", &args)?;
    // Up to the first value that is not a PR's conversations.
    let values = gem_json::parse_stream(&String::from_utf8_lossy(&stdout)).unwrap_or_default();
    Some(
        (values.iter())
            .map_while(|value| Fetched::from_json(value).ok())
            .collect(),
    )
}

#[derive(Clone)]
pub struct Conversations(Arc<Inner>);

// At most `CONCURRENT_AGENTS` agents run at once; the others wait for a permit.
struct Semaphore {
    available: Mutex<usize>,
    released: Condvar,
}

// Gives its permit back when dropped.
struct Permit<'a>(&'a Semaphore);

impl Semaphore {
    fn new(permits: usize) -> Semaphore {
        Semaphore {
            available: Mutex::new(permits),
            released: Condvar::new(),
        }
    }

    fn acquire(&self) -> Permit<'_> {
        let available = self.available.lock().unwrap();
        let mut available = (self.released)
            .wait_while(available, |available| *available == 0)
            .unwrap();
        *available -= 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.available.lock().unwrap() += 1;
        self.0.released.notify_one();
    }
}

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
    // Persisted to no_proposal_reasons.
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

// Runs `sql` once per set of `params`. The memory it mirrors stays right when it fails.
fn save(sql: &str, params: impl IntoIterator<Item = Vec<SqlValue>>) {
    let saved = crate::db().with(|db| {
        let mut statement = db.prepare(sql)?;
        for params in params {
            statement.execute(params)?;
        }
        Ok(())
    });
    if let Err(error) = saved {
        warn!("cannot save conversations", error = error);
    }
}

impl Conversations {
    pub fn load() -> Result<Self, String> {
        let (handled, no_proposal) = crate::db().with(|db| {
            let handled: HashSet<String> = db
                .prepare("SELECT id FROM started_conversations")?
                .query_map(params![], |row| row.get::<String>(0))?
                .collect::<Result<_, _>>()?;
            let no_proposal: HashMap<String, String> = db
                .prepare("SELECT id, reason FROM no_proposal_reasons")?
                .query_map(params![], |row| {
                    Ok((row.get::<String>(0)?, row.get::<String>(1)?))
                })?
                .collect::<Result<_, _>>()?;
            Ok((handled, no_proposal))
        })?;
        Ok(Self(Arc::new(Inner {
            prs: RwLock::default(),
            handled: Mutex::new(handled.clone()),
            started: Mutex::new(handled),
            agents: Semaphore::new(CONCURRENT_AGENTS),
            answering: Mutex::default(),
            no_proposal: Mutex::new(no_proposal),
        })))
    }

    // The agents answering conversations, by the ID of the conversation each answers. One without
    // a thread is queued.
    pub fn running(&self) -> Vec<(String, Running)> {
        (self.0.answering.lock().unwrap().iter())
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
    pub fn thread_of(&self, id: &str) -> Option<String> {
        (self.0.answering.lock().unwrap().get(id)).and_then(|answering| answering.thread_id.clone())
    }

    // Stops showing the agent answering the conversation `id`. A queued one still starts, but does
    // not show.
    pub fn forget(&self, id: &str) {
        self.0.answering.lock().unwrap().remove(id);
    }

    // Every conversation waiting on the user by ID, with its comment as last fetched when it is one:
    // where in the PR's current diff a review comment now sits.
    pub fn waiting(&self) -> HashMap<String, Option<Comment>> {
        let prs = self.0.prs.read().unwrap();
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
    pub fn restart(&self, urls: &[String]) {
        proposals::remove_on(urls);
        let ids: HashSet<String> = {
            let prs = self.0.prs.read().unwrap();
            urls.iter()
                .filter_map(|url| prs.get(url))
                .flat_map(|known| known.waiting.iter().map(|c| c.id().to_string()))
                .collect()
        };
        {
            let mut started = self.0.started.lock().unwrap();
            started.retain(|id| !ids.contains(id));
            save(
                "DELETE FROM started_conversations WHERE id = ?1",
                ids.iter().map(|id| params![id]),
            );
        }
        self.0
            .handled
            .lock()
            .unwrap()
            .retain(|id| !ids.contains(id));
        {
            let mut no_proposal = self.0.no_proposal.lock().unwrap();
            no_proposal.retain(|id, _| !ids.contains(id));
            save(
                "DELETE FROM no_proposal_reasons WHERE id = ?1",
                ids.iter().map(|id| params![id]),
            );
        }
        self.answer_waiting();
    }

    // Each PR's reason its failed checks have no proposal, while they still wait on the user.
    pub fn fill(&self, clusters: &mut [PullRequestCluster]) {
        let prs = self.0.prs.read().unwrap();
        let no_proposal = self.0.no_proposal.lock().unwrap();
        for pr in clusters.iter_mut().flat_map(|c| c.pull_requests.iter_mut()) {
            pr.no_proposal_reason = (prs.get(&pr.url).into_iter())
                .flat_map(|known| &known.waiting)
                .find_map(|c| no_proposal.get(c.id()))
                .cloned()
                .unwrap_or_default();
        }
    }

    pub fn refresh(&self, prs: &[SearchedPullRequest]) {
        let changed: Vec<&SearchedPullRequest> = {
            let known = self.0.prs.read().unwrap();
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
        for mut result in fetch(&ids) {
            result.waiting.retain(|c| {
                !matches!(c, Conversation::Comment { comment, .. } if comment.author == login())
            });
            fetched.insert(result.url.clone(), result);
        }

        {
            let mut known = self.0.prs.write().unwrap();
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
            Tokens::here().retain(|subject| match subject {
                Subject::Investigation { .. } => true,
                Subject::Comment { pr, .. } | Subject::CheckFailure { pr, .. } => {
                    prs.iter().any(|open| open.url == *pr)
                        && known.get(pr).is_none_or(|known| {
                            known.waiting.iter().any(|c| c.id() == subject.id())
                        })
                }
            });
        }

        let heads = prs
            .iter()
            .map(|pr| (pr.url.as_str(), pr.head_oid.as_str()))
            .collect();
        let stale = proposals::remove_stale(&heads);
        if !stale.is_empty() {
            let mut started = self.0.started.lock().unwrap();
            started.retain(|id| !stale.contains(id));
            save(
                "DELETE FROM started_conversations WHERE id = ?1",
                stale.iter().map(|id| params![id]),
            );
        }

        self.0
            .handled
            .lock()
            .unwrap()
            .retain(|id| !stale.contains(id));
        self.answer_waiting();
    }

    // Queues an agent for each waiting conversation that no agent was queued for.
    fn answer_waiting(&self) {
        let known = self.0.prs.read().unwrap();
        let mut handled = self.0.handled.lock().unwrap();
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
                (self.0.answering.lock().unwrap()).insert(conversation.id().to_string(), answering);
                let this = self.clone();
                let pr = pr.target.clone();
                let conversation = conversation.clone();
                std::thread::spawn(move || {
                    let _permit = this.0.agents.acquire();
                    {
                        let mut started = this.0.started.lock().unwrap();
                        started.insert(conversation.id().to_string());
                        save(
                            "INSERT OR IGNORE INTO started_conversations (id) VALUES (?1)",
                            [params![conversation.id()]],
                        );
                    }
                    let id = conversation.id();
                    let (title, prompt) = conversation.prompt(&pr);
                    let subject = conversation.subject(&pr.url);
                    let started = match mcp::agent_config(&subject) {
                        Ok(config) => gem_t3::start(&title, &prompt, Some(&config)),
                        Err(error) => Err(error),
                    };
                    match started {
                        Ok(thread_id) => {
                            if let Some(answering) = this.0.answering.lock().unwrap().get_mut(id) {
                                answering.thread_id = Some(thread_id.clone());
                            }
                            let reply = gem_t3::wait(&thread_id);
                            if let Conversation::Checks { check_failure } = &conversation {
                                let reply = reply.trim();
                                investigations::archive(&title, &pr.url, check_failure, reply);
                                if !proposals::exist_for(id) {
                                    let mut no_proposal = this.0.no_proposal.lock().unwrap();
                                    save(
                                        "INSERT OR REPLACE INTO no_proposal_reasons (id, reason) \
                                         VALUES (?1, ?2)",
                                        [params![id, reply]],
                                    );
                                    no_proposal.insert(id.to_string(), reply.to_string());
                                }
                            }
                        }
                        Err(e) => eprintln!("{title}: {e}"),
                    }
                    this.0.answering.lock().unwrap().remove(id);
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
- open a new pull request, with `open_pull_request`: when the change belongs in a pull request of \
its own, such as a fix on the base branch. Make its `patch` the same way against the latest \
`origin/<base branch>`, and link {url} in its description.
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
- open a new pull request, with `open_pull_request`: when the fix belongs in a pull request of its \
own, such as a test that fails the same way on the base branch. Make its `patch` the same way \
against the latest `origin/<base branch>`, and link {url} in its description.
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
settle this thread. Then reply in one or two sentences: first what you proposed, or, when you \
proposed nothing, why, then the root cause, naming the failing test or error, with the pull \
request of each earlier investigation into the same failure. {login} reads its start next to the \
failed checks, and later agents find it with `search_investigations`.",
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
