// Every T3 Code agent gem runs, as the one list the frontend shows: investigations and the agents
// answering the user's messages, which keep their own files, and the agents answering
// conversations, rebasing and merging, which are kept by what they work on. Each is told apart by
// its ID's prefix, so a message or a close reaches the right one.

use std::{collections::HashMap, sync::LazyLock, time::Duration};

use pbjson_types::Timestamp;
use tokio::sync::RwLock;

use crate::conversations::Conversations;
use crate::proto::{Agent, AgentState, ListOpenPullRequestsResponse};
use crate::{investigations, merges, rebases, t3};

// Reading a thread is a local CLI call; only a changed reply reaches GitHub, to render it.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

const ANSWER: &str = "answer:";
const REBASE: &str = "rebase:";
const MERGE: &str = "merge:";

// An agent answering a conversation, rebasing or merging, as what it works on keeps it.
pub struct Running {
    // None until its thread starts.
    pub thread_id: Option<String>,
    pub title: String,
    // It waits on the user to answer a question T3 Code asked for it.
    pub question: bool,
    // Seconds since the epoch.
    pub started_at: i64,
    pub pull_request_urls: Vec<String>,
}

// What each of those agents' threads last showed, by thread ID.
#[derive(Clone, Default)]
struct Shown {
    working: bool,
    reply: String,
    reply_html: String,
}

static SHOWN: LazyLock<RwLock<HashMap<String, Shown>>> = LazyLock::new(RwLock::default);

pub async fn fill(response: &mut ListOpenPullRequestsResponse, conversations: &Conversations) {
    let mut agents = investigations::agents().await;
    let shown = SHOWN.read().await;
    let agent = |id: String, running: Running, waiting: bool| {
        let shown = (running.thread_id.as_ref())
            .and_then(|thread_id| shown.get(thread_id))
            .cloned()
            .unwrap_or_default();
        let state = match running.thread_id {
            None => AgentState::Queued,
            Some(_) if waiting => AgentState::Waiting,
            Some(_) => AgentState::Working,
        };
        Agent {
            id,
            title: running.title,
            state: state as i32,
            pull_request_urls: running.pull_request_urls,
            reply: shown.reply,
            reply_html: shown.reply_html,
            proposals: Vec::new(),
            // 0 for a rebase or merge started before its start was kept.
            started_at: (running.started_at > 0).then_some(Timestamp {
                seconds: running.started_at,
                nanos: 0,
            }),
        }
    };
    // An agent answering a conversation is listed until it stops: its proposals show on the pull
    // request. A rebase or merge stopped without settling its thread waits on the user.
    let stopped = |running: &Running| {
        running.question
            || (running.thread_id.as_ref())
                .and_then(|thread_id| shown.get(thread_id))
                .is_some_and(|shown| !shown.working)
    };
    for (id, running) in conversations.running().await {
        agents.push(agent(format!("{ANSWER}{id}"), running, false));
    }
    for (prefix, running) in (rebases::running().into_iter().map(|r| (REBASE, r)))
        .chain(merges::running().into_iter().map(|r| (MERGE, r)))
    {
        let id = format!(
            "{prefix}{}",
            running.thread_id.as_deref().unwrap_or_default()
        );
        let waiting = stopped(&running);
        agents.push(agent(id, running, waiting));
    }
    agents.sort_by_key(|a| std::cmp::Reverse(a.started_at.as_ref().map(|t| (t.seconds, t.nanos))));
    response.agents = agents;
}

pub async fn message(conversations: &Conversations, id: &str, message: &str) -> Result<(), String> {
    let thread_id = if let Some(conversation) = id.strip_prefix(ANSWER) {
        (conversations.thread_of(conversation).await).ok_or("the agent has not started yet")?
    } else if let Some(thread_id) = id.strip_prefix(REBASE).or(id.strip_prefix(MERGE)) {
        if thread_id.is_empty() {
            return Err("the agent has not started yet".into());
        }
        thread_id.to_string()
    } else {
        return investigations::message(id, message).await;
    };
    t3::send(&thread_id, message).await?;
    SHOWN.write().await.entry(thread_id).or_default().working = true;
    Ok(())
}

pub async fn close(conversations: &Conversations, id: &str) -> Result<(), String> {
    if let Some(conversation) = id.strip_prefix(ANSWER) {
        conversations.forget(conversation).await;
    } else if let Some(thread_id) = id.strip_prefix(REBASE) {
        rebases::forget(thread_id)?;
    } else if let Some(thread_id) = id.strip_prefix(MERGE) {
        merges::forget(thread_id)?;
    } else if investigations::has(id).await {
        investigations::close(id).await;
    } else {
        return Err(format!("no agent {id}"));
    }
    Ok(())
}

// Keeps whether each agent answering a conversation, rebasing or merging works, and its latest
// reply, up to date with its thread. Investigations keep their own.
pub fn watch(conversations: Conversations) {
    tokio::spawn(async move {
        loop {
            let mut threads: HashMap<String, String> = HashMap::new();
            let running = (conversations.running().await.into_iter().map(|(_, r)| r))
                .chain(rebases::running())
                .chain(merges::running());
            for running in running {
                if let (Some(thread_id), Some(pr)) = (
                    running.thread_id,
                    running.pull_request_urls.into_iter().next(),
                ) {
                    threads.insert(thread_id, pr);
                }
            }
            SHOWN
                .write()
                .await
                .retain(|thread_id, _| threads.contains_key(thread_id));
            for (thread_id, pr) in threads {
                let Some((working, reply)) = t3::show(&thread_id).await else {
                    continue;
                };
                let known = SHOWN.read().await.get(&thread_id).cloned();
                let reply_html = match known {
                    Some(known) if known.reply == reply => known.reply_html,
                    _ if reply.is_empty() => String::new(),
                    _ => gem_github::render(&pr, &reply).await.unwrap_or_default(),
                };
                SHOWN.write().await.insert(
                    thread_id,
                    Shown {
                        working,
                        reply,
                        reply_html,
                    },
                );
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}
