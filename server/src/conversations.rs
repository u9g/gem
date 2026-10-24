use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
};

use pbjson_types::Timestamp;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::{SearchedPullRequest, gh, login, proposals, proto::Comment, t3};

// Relative to the server's working directory, like the cluster names.
const HANDLED_FILE: &str = "handled-comments.json";
const CONCURRENT_AGENTS: usize = 4;

// Fetched only for PRs whose updatedAt moved, which any new comment or review thread does: the
// whole query costs about one rate-limit point per PR, against two for the search of all PRs.
const CONVERSATIONS_QUERY: &str = r#"
query($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on PullRequest {
      url
      reviewThreads(first: 100) {
        nodes { id isResolved comments(last: 1) { nodes { id body url authorAssociation author { login __typename } } } }
      }
      comments(last: 1) { nodes { id body url authorAssociation author { login __typename } } }
    }
  }
}
"#;

// An unresolved review thread's latest reply, or the PR's latest top-level comment, from a bot or
// a member of the PR's organization. It waits on the user unless it is the user's own.
const CONVERSATIONS_JQ: &str = r#".data.nodes[] | {
  url,
  waiting: [
    (.reviewThreads.nodes[] | select(.isResolved | not) | {thread_id: .id, comment: .comments.nodes[0]}),
    (.comments.nodes[] | {thread_id: null, comment: .})
    | select(.comment.author.__typename == "Bot" or .comment.authorAssociation == "MEMBER" or .comment.authorAssociation == "OWNER")
    | .comment |= {id, body, url, author: (.author.login // "ghost")}
  ]
}"#;

#[derive(Deserialize)]
struct Fetched {
    url: String,
    waiting: Vec<Conversation>,
}

#[derive(Deserialize, Clone)]
struct Conversation {
    thread_id: Option<String>,
    comment: Comment,
}

#[derive(Clone)]
pub struct Conversations(Arc<Inner>);

struct Inner {
    // By PR URL: the PR's updatedAt when its conversations were fetched, and the waiting ones.
    prs: RwLock<HashMap<String, (Timestamp, Vec<Conversation>)>>,
    // Comments an agent was queued for.
    handled: Mutex<HashSet<String>>,
    // Comments an agent was started for: the persisted part of `handled`. Never cleared, so a
    // restart answers comments that were only queued, and not the others.
    started: Mutex<HashSet<String>>,
    agents: Semaphore,
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
        }))
    }

    // IDs of the latest comment of every conversation waiting on the user.
    pub async fn waiting(&self) -> HashSet<String> {
        let prs = self.0.prs.read().await;
        prs.values()
            .flat_map(|(_, waiting)| waiting.iter().map(|c| c.comment.id.clone()))
            .collect()
    }

    pub async fn refresh(&self, prs: &[SearchedPullRequest]) {
        let changed: Vec<&SearchedPullRequest> = {
            let known = self.0.prs.read().await;
            prs.iter()
                .filter(|pr| known.get(&pr.url).map(|(at, _)| at) != Some(&pr.updated_at))
                .collect()
        };
        let mut fetched = HashMap::new();
        for chunk in changed.chunks(100) {
            let mut args = vec![
                "api".to_string(),
                "graphql".to_string(),
                "-f".to_string(),
                format!("query={CONVERSATIONS_QUERY}"),
                "--jq".to_string(),
                CONVERSATIONS_JQ.to_string(),
            ];
            for pr in chunk {
                args.extend(["-f".to_string(), format!("ids[]={}", pr.id)]);
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let Some(stdout) = gh(&args).await else {
                continue;
            };
            for result in serde_json::Deserializer::from_slice(&stdout).into_iter::<Fetched>() {
                let Ok(mut result) = result else { break };
                result.waiting.retain(|c| c.comment.author != login());
                fetched.insert(result.url, result.waiting);
            }
        }

        {
            let mut known = self.0.prs.write().await;
            known.retain(|url, _| prs.iter().any(|pr| &pr.url == url));
            for pr in &changed {
                if let Some(waiting) = fetched.remove(&pr.url) {
                    known.insert(pr.url.clone(), (pr.updated_at.clone(), waiting));
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
            let Some((_, waiting)) = known.get(&pr.url) else {
                continue;
            };
            for conversation in waiting {
                if !handled.insert(conversation.comment.id.clone()) {
                    continue;
                }
                let this = self.clone();
                let pr = Target::new(pr);
                let conversation = conversation.clone();
                tokio::spawn(async move {
                    let _permit = this.0.agents.acquire().await.unwrap();
                    {
                        let mut started = this.0.started.lock().await;
                        started.insert(conversation.comment.id.clone());
                        let json = serde_json::to_vec_pretty(&*started).unwrap();
                        let _ = tokio::fs::write(HANDLED_FILE, json).await;
                    }
                    answer(&pr, &conversation).await;
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

async fn answer(pr: &Target, conversation: &Conversation) {
    let comment = &conversation.comment;
    let exe = env::current_exe().unwrap();
    let dir = env::current_dir().unwrap().join(proposals::DIR);
    let mut propose = format!(
        "{} propose --dir {} --pr {} --comment {}",
        exe.display(),
        dir.display(),
        pr.url,
        comment.id
    );
    let (where_, reply_kind, resolve) = match &conversation.thread_id {
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
{reply_kind}.{resolve}
- hand the work to another agent, with --follow-up: when the right fix is too big to review as \
one patch on this pull request, such as matching changes in other repositories, write a prompt \
for a T3 Code agent to a markdown file. It must stand alone: the repositories (checked out under \
~/code), the branches or pull requests to build on, what to change, and how to verify it. \
Approving starts that agent, so pair it with a reply saying the work follows. Prefer --patch \
when the change fits this pull request.
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
