use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use gem_db::{
    MapMut,
    xitdb::{Map, Set},
};
use pbjson_types::Timestamp;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{
    SearchedPullRequest,
    agents::Running,
    gh, investigations, login, mcp, proposals,
    proto::{CheckFailure, Comment, Proposal, PullRequestCluster},
    t3,
    tokens::{Subject, Tokens},
};

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
    // By the ID of the failed checks they answer, or the URL of the cluster whose comments they
    // answer, until they stop or the user closes them.
    answering: Mutex<HashMap<String, Answering>>,
    // The oldest pull request of each open PR's cluster, by PR URL, as last refreshed.
    clusters: RwLock<HashMap<String, Target>>,
    // The thread answering each cluster's comments, by the URL of its oldest pull request when it
    // started. Persisted to cluster_agents.
    threads: Mutex<HashMap<String, String>>,
    // Held by whatever is telling a cluster's agent something, by the same URL, so it hears one
    // message at a time and a cluster starts one agent.
    telling: Mutex<HashMap<String, Arc<Mutex<()>>>>,
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

// In the database's moment: the IDs of the conversations an agent was started for,
const STARTED: Set<str> = Set::new("started-conversations");
// why each agent answering failed checks proposed nothing, by the checks' ID,
const NO_PROPOSAL: Map<str, String> = Map::new("no-proposal-reasons");
// and the thread of the agent answering the comments on each cluster, by the URL of the cluster's
// oldest pull request when it started.
const THREADS: Map<str, String> = Map::new("cluster-agents");

// Makes the changes `write` makes. The memory it mirrors stays right when it fails.
fn save<T>(write: impl FnOnce(&mut MapMut) -> gem_db::Result<T>) {
    if let Err(error) = crate::db().write(write) {
        tracing::warn!(%error, "cannot save conversations");
    }
}

fn strs<'k>(keys: impl IntoIterator<Item = &'k String>) -> impl Iterator<Item = &'k str> {
    keys.into_iter().map(String::as_str)
}

impl Conversations {
    pub fn load() -> Result<Self, String> {
        let (handled, no_proposal, threads) = crate::db().read(|moment| {
            let handled: HashSet<String> = STARTED.iter(moment)?.collect::<gem_db::Result<_>>()?;
            let no_proposal: HashMap<String, String> =
                NO_PROPOSAL.iter(moment)?.collect::<gem_db::Result<_>>()?;
            let threads: HashMap<String, String> =
                THREADS.iter(moment)?.collect::<gem_db::Result<_>>()?;
            Ok((handled, no_proposal, threads))
        })?;
        Ok(Self(Arc::new(Inner {
            prs: RwLock::default(),
            handled: Mutex::new(handled.clone()),
            started: Mutex::new(handled),
            agents: Semaphore::new(CONCURRENT_AGENTS),
            answering: Mutex::default(),
            clusters: RwLock::default(),
            threads: Mutex::new(threads),
            telling: Mutex::default(),
            no_proposal: Mutex::new(no_proposal),
        })))
    }

    // The agents answering conversations, by the ID of the failed checks or the URL of the cluster
    // each answers. One without a thread is queued.
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

    // The thread of the agent answering `id`, once it started.
    pub async fn thread_of(&self, id: &str) -> Option<String> {
        (self.0.answering.lock().await.get(id)).and_then(|answering| answering.thread_id.clone())
    }

    // Stops showing the agent answering `id`. A queued one still starts, but does not show.
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
    // conversations and their clusters' agents, and queues new agents for them.
    pub async fn restart(&self, urls: &[String]) {
        proposals::remove_on(urls).await;
        let keys: HashSet<String> = {
            let clusters = self.0.clusters.read().await;
            (urls.iter().filter_map(|url| clusters.get(url)))
                .map(|key| key.url.clone())
                .collect()
        };
        self.0.threads.lock().await.retain(|pr, _| !keys.contains(pr));
        save(|moment| THREADS.remove_all(moment, strs(&keys)));
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
            save(|moment| STARTED.remove_all(moment, strs(&ids)));
        }
        self.0.handled.lock().await.retain(|id| !ids.contains(id));
        {
            let mut no_proposal = self.0.no_proposal.lock().await;
            no_proposal.retain(|id, _| !ids.contains(id));
            save(|moment| NO_PROPOSAL.remove_all(moment, strs(&ids)));
        }
        self.answer_waiting().await;
    }

    // Tells the agent answering the comments on `proposal`'s cluster that the user approved it, so
    // it updates the proposals the approval affects. `edited` when the user changed it first.
    pub async fn approved(&self, proposal: &Proposal, edited: bool) {
        let Some(comment) = &proposal.comment else {
            return;
        };
        let Some(key) = (self.0.clusters.read().await)
            .get(&proposal.pull_request_url)
            .cloned()
        else {
            return;
        };
        let edited = match edited {
            true => " after editing it: read what was posted on GitHub",
            false => "",
        };
        // A cluster's agent that is gone is replaced only when it has proposals to update.
        let start = {
            let clusters = self.0.clusters.read().await;
            (proposals::load(crate::db()).iter()).any(|other| {
                other.comment.is_some()
                    && clusters
                        .get(&other.pull_request_url)
                        .is_some_and(|k| k.url == key.url)
            })
        };
        let message = format!(
            "{login} approved the proposal {id} on {url} (\"{summary}\"), answering {comment}, and \
gem ran it{edited}. Update the open proposals it affects, each by proposing its new version with \
`replaces`.",
            login = login(),
            id = proposal.id,
            url = proposal.pull_request_url,
            summary = proposal.summary,
            comment = comment.url,
        );
        self.tell(key, message, Vec::new(), start).await;
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

    // `clusters` hold `prs`, oldest pull request first.
    pub async fn refresh(&self, prs: &[SearchedPullRequest], clusters: &[PullRequestCluster]) {
        {
            let mut threads = self.0.threads.lock().await;
            let closed: Vec<String> = (threads.keys())
                .filter(|pr| !prs.iter().any(|open| open.url == **pr))
                .cloned()
                .collect();
            threads.retain(|pr, _| !closed.contains(pr));
            save(|moment| THREADS.remove_all(moment, strs(&closed)));
        }
        {
            let by_url: HashMap<&str, &SearchedPullRequest> =
                prs.iter().map(|pr| (pr.url.as_str(), pr)).collect();
            let mut keys = self.0.clusters.write().await;
            keys.clear();
            for cluster in clusters {
                let Some(oldest) = (cluster.pull_requests.first())
                    .and_then(|pr| by_url.get(pr.url.as_str()))
                else {
                    continue;
                };
                for pr in &cluster.pull_requests {
                    keys.insert(pr.url.clone(), Target::new(oldest));
                }
            }
        }
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
            // proposals no longer show, and a cluster's once its oldest pull request closes. A PR
            // whose fetch failed keeps its agents' tokens.
            Tokens::here()
                .retain(|subject| match subject {
                    Subject::Investigation { .. } => true,
                    Subject::Cluster { pr } => prs.iter().any(|open| open.url == *pr),
                    Subject::CheckFailure { pr, .. } => {
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
            save(|moment| STARTED.remove_all(moment, strs(&stale)));
        }

        self.0.handled.lock().await.retain(|id| !stale.contains(id));
        self.answer_waiting().await;
    }

    // Queues an agent for each waiting failed checks no agent was queued for, and sends each
    // cluster's agent the comments waiting on the user it was not sent, each with its proposals
    // whose patches went stale. A comment with no proposal on a cluster with no agent is sent
    // again: the agent it went to is gone, such as when its oldest pull request closed.
    async fn answer_waiting(&self) {
        let proposals = proposals::load(crate::db());
        let answered: HashSet<&str> = (proposals.iter())
            .filter_map(|p| p.comment.as_ref().map(|c| c.id.as_str()))
            .collect();
        let known = self.0.prs.read().await;
        let clusters = self.0.clusters.read().await;
        let threads = self.0.threads.lock().await.clone();
        let answering: HashSet<String> = self.0.answering.lock().await.keys().cloned().collect();
        let mut handled = self.0.handled.lock().await;
        // By the URL of the cluster's oldest pull request: it, the comments asked about, and their
        // IDs.
        let mut asks: HashMap<String, (Target, Vec<String>, Vec<String>)> = HashMap::new();
        for pr in known.values() {
            for conversation in &pr.waiting {
                let id = conversation.id().to_string();
                match conversation {
                    Conversation::Comment { thread_id, comment } => {
                        let Some(key) = clusters.get(&pr.target.url) else {
                            continue;
                        };
                        let orphaned = !answered.contains(id.as_str())
                            && !threads.contains_key(&key.url)
                            && !answering.contains(&key.url);
                        if !handled.insert(id.clone()) && !orphaned {
                            continue;
                        }
                        let removed: Vec<&Proposal> = (proposals.iter())
                            .filter(|p| p.error == proposals::STALE)
                            .filter(|p| p.comment.as_ref().is_some_and(|c| c.id == id))
                            .collect();
                        let (_, asked, ids) = (asks.entry(key.url.clone()))
                            .or_insert_with(|| (key.clone(), Vec::new(), Vec::new()));
                        asked.push(ask(&pr.target, thread_id, comment, &removed));
                        ids.push(id);
                    }
                    Conversation::Checks { check_failure } => {
                        if handled.insert(id) {
                            self.answer_checks(pr.target.clone(), check_failure.clone())
                                .await;
                        }
                    }
                }
            }
        }
        for (key, asked, ids) in asks.into_values() {
            self.tell(key, asked.join("\n"), ids, true).await;
        }
    }

    // Sends `message` to the agent answering the comments on the cluster whose oldest pull request
    // is `key`, starting one when the cluster has none and `start`. `asked` are the IDs of the
    // comments it asks about.
    async fn tell(&self, key: Target, message: String, asked: Vec<String>, start: bool) {
        let title = format!("Answer comments on {}#{}", key.full_name, key.number);
        if start {
            (self.0.answering.lock().await)
                .entry(key.url.clone())
                .or_insert_with(|| Answering {
                    pr: key.url.clone(),
                    title: title.clone(),
                    thread_id: None,
                    queued_at: now(),
                });
        }
        let turn = Arc::clone(
            (self.0.telling.lock().await)
                .entry(key.url.clone())
                .or_default(),
        );
        let this = self.clone();
        tokio::spawn(async move {
            let delivering = turn.lock().await;
            let _permit = this.0.agents.acquire().await.unwrap();
            let thread = this.0.threads.lock().await.get(&key.url).cloned();
            let mut sent = Err("no agent answers the cluster's comments".to_string());
            if let Some(thread_id) = thread {
                sent = t3::send(&thread_id, &message).await.map(|()| thread_id);
                if sent.is_err() {
                    this.forget_thread(&key.url).await;
                }
            }
            if sent.is_err() && start {
                let subject = Subject::Cluster {
                    pr: key.url.clone(),
                };
                let prompt = format!("{}\n\n{message}", answer(&key));
                sent = match mcp::agent_config(&subject).await {
                    Ok(config) => t3::start(&title, &prompt, Some(&config)).await,
                    Err(error) => Err(error),
                };
                if let Ok(thread_id) = &sent {
                    (this.0.threads.lock().await).insert(key.url.clone(), thread_id.clone());
                    save(|moment| THREADS.put(moment, &key.url, thread_id));
                }
            }
            match sent {
                Ok(thread_id) => {
                    {
                        let mut started = this.0.started.lock().await;
                        started.extend(asked.iter().cloned());
                        save(|moment| {
                            for id in &asked {
                                STARTED.insert(moment, id)?;
                            }
                            Ok(())
                        });
                    }
                    (this.0.answering.lock().await)
                        .entry(key.url.clone())
                        .or_insert_with(|| Answering {
                            pr: key.url.clone(),
                            title,
                            thread_id: None,
                            queued_at: now(),
                        })
                        .thread_id = Some(thread_id.clone());
                    // The message reached the agent: later ones can be sent while it works, and
                    // are its next turns.
                    drop(delivering);
                    t3::wait(&thread_id).await;
                }
                // The next refresh asks about them again.
                Err(e) => {
                    eprintln!("{title}: {e}");
                    (this.0.handled.lock().await).retain(|id| !asked.contains(id));
                }
            }
            this.0.answering.lock().await.remove(&key.url);
        });
    }

    // Forgets the thread answering the comments on the cluster whose oldest pull request is `pr`.
    async fn forget_thread(&self, pr: &str) {
        self.0.threads.lock().await.remove(pr);
        save(|moment| THREADS.remove(moment, pr));
    }

    // Queues an agent answering `failure` on `pr`.
    async fn answer_checks(&self, pr: Target, failure: CheckFailure) {
        let (title, prompt) = fix(&pr, &failure);
        let answering = Answering {
            pr: pr.url.clone(),
            title: title.clone(),
            thread_id: None,
            queued_at: now(),
        };
        (self.0.answering.lock().await).insert(failure.id.clone(), answering);
        let this = self.clone();
        tokio::spawn(async move {
            let _permit = this.0.agents.acquire().await.unwrap();
            let id = failure.id.as_str();
            {
                let mut started = this.0.started.lock().await;
                started.insert(id.to_string());
                save(|moment| STARTED.insert(moment, id));
            }
            let subject = Subject::CheckFailure {
                pr: pr.url.clone(),
                id: id.to_string(),
            };
            let started = match mcp::agent_config(&subject).await {
                Ok(config) => t3::start(&title, &prompt, Some(&config)).await,
                Err(error) => Err(error),
            };
            match started {
                Ok(thread_id) => {
                    if let Some(answering) = this.0.answering.lock().await.get_mut(id) {
                        answering.thread_id = Some(thread_id.clone());
                    }
                    let reply = t3::wait(&thread_id).await;
                    let reply = reply.trim();
                    investigations::archive(&title, &pr.url, &failure, reply).await;
                    if !proposals::exist_for(id).await {
                        let mut no_proposal = this.0.no_proposal.lock().await;
                        save(|moment| NO_PROPOSAL.put(moment, id, reply));
                        no_proposal.insert(id.to_string(), reply.to_string());
                    }
                }
                Err(e) => eprintln!("{title}: {e}"),
            }
            this.0.answering.lock().await.remove(id);
        });
    }
}

// Seconds since the epoch.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
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

// The prompt of the agent answering the comments on the cluster whose oldest pull request is `key`,
// before the comments it starts on. It keeps the wording of the prompt that answered one comment:
// replaying 20 past comments, that answered as often as longer prompts that also listed the steps,
// said to read the pull request, and introduced `execute`, or more often. Without "once per
// reasonable alternative", agents made one proposal and answered far less often.
fn answer(key: &Target) -> String {
    format!(
        "You answer the comments waiting on {login} on the cluster of their pull requests holding \
{url} (\"{title}\"): `gem.cluster({url:?})` shows it now, with what is already proposed. gem sends \
you each comment in this thread as it comes, and tells you when {login} approves a proposal. A \
message can be out of date by the time you read it: check what is proposed now, and skip a change \
an earlier message already made.

Record how {login} should answer each comment with `gem.propose`, with `pr` the pull request it is \
on, and `comment` and `thread` as given. Propose once per reasonable alternative. To change a \
proposal, propose its new version with `replaces` set to its ID.

Never push, comment, review or resolve anything on GitHub yourself: {login} approves proposals later.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread, and again after answering each message gem sends. Then reply with one line \
saying what you proposed.",
        login = login(),
        url = key.url,
        title = key.title,
    )
}

// Asks a cluster's agent to answer `comment` on `pr`, saying which proposals answering it went
// stale and need replacing.
fn ask(
    pr: &Target,
    thread_id: &Option<String>,
    comment: &Comment,
    removed: &[&Proposal],
) -> String {
    let (where_, thread) = match thread_id {
        Some(thread) => (
            format!("in the review thread {thread}"),
            format!(", `thread` {thread:?}"),
        ),
        None => (
            "in the pull request's top-level comments".to_string(),
            String::new(),
        ),
    };
    let quoted: String = comment
        .body
        .lines()
        .map(|line| format!("> {line}\n"))
        .collect();
    let removed = match removed {
        [] => String::new(),
        removed => {
            let listed: String = (removed.iter())
                .map(|p| format!("- {} ({})\n", p.summary, p.id))
                .collect();
            format!(
                "
These proposals answering it have patches made against older heads of the pull requests. Replace \
each with one against the new heads, with `replaces`:

{listed}"
            )
        }
    };
    format!(
        "{author} is waiting on {login} {where_} on the pull request {url} (\"{title}\"). Their \
latest comment ({comment_url}), with `comment` {id:?}{thread}:

{quoted}{removed}",
        author = comment.author,
        login = login(),
        url = pr.url,
        title = pr.title,
        comment_url = comment.url,
        id = comment.id,
    )
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
- Read what earlier investigations found: `gem.investigations()` lists them, to filter by the \
failing test's name or words of the error. Ones that fail the same way may already know the root \
cause, or a fix that did not hold.
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
this one consumes. Make each patch the same way, against its own pull request. \
`gem.cluster({url:?})` lists the cluster's pull requests.
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

{EXECUTE}

Record each proposal with `gem.propose`, with `pr` {url}, a `summary` of one line saying what \
approving does, linking the pull request of each earlier investigation into the same failure, its \
`excerpts`, and its steps. Propose once per answer, and once per alternative when more than one \
answer is reasonable. It checks the proposal and throws an Error with the reason when something \
is wrong: fix it and propose again. If the failure needs nothing from {login}, propose nothing.

If answering well needs something these steps cannot do, propose with its `excerpts` and \
`request_tool` instead of steps: markdown describing the missing tool (what it would do and why \
this failure needs it), with a `summary` of \"<tool name>: <one line>\".

Never push, comment, rerun or change anything on GitHub yourself: {login} approves proposals later.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread. Then reply in one or two sentences: first what you proposed, or, when you \
proposed nothing, why, then the root cause, naming the failing test or error, with the pull \
request of each earlier investigation into the same failure. {login} reads its start next to the \
failed checks, and later agents find it in `gem.investigations()`.",
        head = failure.head_oid,
        login = login(),
        url = pr.url,
        title = pr.title,
        full_name = pr.full_name,
        number = pr.number,
        EXECUTE = mcp::EXECUTE,
    );
    let title = format!("Fix checks on {}#{}", pr.full_name, pr.number);
    (title, prompt)
}
