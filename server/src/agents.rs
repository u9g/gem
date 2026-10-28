// Every T3 Code agent gem runs, as the one list the frontend shows: investigations and the agents
// answering the user's messages, which keep their own files, and the agents answering
// conversations, rebasing and merging, which are kept by what they work on. Each is told apart by
// its ID's prefix, so a message or a close reaches the right one.

use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
    time::Duration,
};

use gem_proto::Timestamp;

use crate::conversations::Conversations;
use crate::proto::{Agent, AgentState, ListOpenPullRequestsResponse, PullRequestCluster};
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

// Each agent is in the one cluster holding its open pull requests, or in the response's own list when
// none is open. Clustering and placement read the same agents, so they always agree.
pub fn fill(response: &mut ListOpenPullRequestsResponse, conversations: &Conversations) {
    let agents = list(conversations);
    join(&mut response.clusters, &agents);
    for agent in agents {
        let cluster = (response.clusters.iter_mut()).find(|cluster| {
            (cluster.pull_requests.iter()).any(|pr| agent.pull_request_urls.contains(&pr.url))
        });
        match cluster {
            Some(cluster) => cluster.agents.push(agent),
            None => response.agents.push(agent),
        }
    }
}

// Joins the clusters each agent acts across into one: its pull requests sorted oldest first, updated
// when its latest pull request was, and mergeable as its pull requests together are. Clusters stay
// most recently updated first.
fn join(clusters: &mut Vec<PullRequestCluster>, agents: &[Agent]) {
    let cluster_of: HashMap<&str, usize> = (clusters.iter().enumerate())
        .flat_map(|(i, cluster)| {
            cluster
                .pull_requests
                .iter()
                .map(move |pr| (pr.url.as_str(), i))
        })
        .collect();
    let links = agents.iter().flat_map(|agent| {
        let acted_on: Vec<usize> = (agent.pull_request_urls.iter())
            .filter_map(|url| cluster_of.get(url.as_str()).copied())
            .collect();
        acted_on
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect::<Vec<_>>()
    });
    let groups = gem_cluster::components(clusters.len(), links);
    if groups.len() == clusters.len() {
        return;
    }
    let mut parts: Vec<Option<PullRequestCluster>> = clusters.drain(..).map(Some).collect();
    let time = |t: &Option<Timestamp>| t.as_ref().map(|t| (t.seconds, t.nanos));
    for group in groups {
        let mut joined: Vec<PullRequestCluster> =
            group.into_iter().filter_map(|i| parts[i].take()).collect();
        if joined.len() == 1 {
            clusters.push(joined.pop().unwrap());
            continue;
        }
        let mut cluster = PullRequestCluster {
            updated_at: joined
                .iter()
                .map(|c| c.updated_at.clone())
                .max_by_key(time)
                .flatten(),
            pull_requests: joined.into_iter().flat_map(|c| c.pull_requests).collect(),
            ..Default::default()
        };
        cluster.pull_requests.sort_by_key(|pr| time(&pr.created_at));
        cluster.merge_by = crate::cluster_merge_by(&cluster) as i32;
        clusters.push(cluster);
    }
    clusters.sort_by_key(|c| std::cmp::Reverse(time(&c.updated_at)));
}

// Every agent, most recently started first.
pub fn list(conversations: &Conversations) -> Vec<Agent> {
    let mut agents = investigations::agents();
    let shown = SHOWN.read().unwrap();
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
    for (id, running) in conversations.running() {
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
    agents
}

pub fn message(conversations: &Conversations, id: &str, message: &str) -> Result<(), String> {
    let thread_id = if let Some(conversation) = id.strip_prefix(ANSWER) {
        conversations
            .thread_of(conversation)
            .ok_or("the agent has not started yet")?
    } else if let Some(thread_id) = id.strip_prefix(REBASE).or(id.strip_prefix(MERGE)) {
        if thread_id.is_empty() {
            return Err("the agent has not started yet".into());
        }
        thread_id.to_string()
    } else {
        return investigations::message(id, message);
    };
    t3::send(&thread_id, message)?;
    SHOWN.write().unwrap().entry(thread_id).or_default().working = true;
    Ok(())
}

pub fn close(conversations: &Conversations, id: &str) -> Result<(), String> {
    if let Some(conversation) = id.strip_prefix(ANSWER) {
        conversations.forget(conversation);
    } else if let Some(thread_id) = id.strip_prefix(REBASE) {
        rebases::forget(thread_id)?;
    } else if let Some(thread_id) = id.strip_prefix(MERGE) {
        merges::forget(thread_id)?;
    } else if investigations::has(id) {
        investigations::close(id);
    } else {
        return Err(format!("no agent {id}"));
    }
    Ok(())
}

// Keeps whether each agent answering a conversation, rebasing or merging works, and its latest
// reply, up to date with its thread. Investigations keep their own.
pub fn watch(conversations: Conversations) {
    std::thread::spawn(move || {
        loop {
            let mut threads: HashMap<String, String> = HashMap::new();
            let running = (conversations.running().into_iter().map(|(_, r)| r))
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
                .unwrap()
                .retain(|thread_id, _| threads.contains_key(thread_id));
            for (thread_id, pr) in threads {
                let Some((working, reply)) = t3::show(&thread_id) else {
                    continue;
                };
                let known = SHOWN.read().unwrap().get(&thread_id).cloned();
                let reply_html = match known {
                    Some(known) if known.reply == reply => known.reply_html,
                    _ if reply.is_empty() => String::new(),
                    _ => gem_github::render(&pr, &reply).unwrap_or_default(),
                };
                SHOWN.write().unwrap().insert(
                    thread_id,
                    Shown {
                        working,
                        reply,
                        reply_html,
                    },
                );
            }
            std::thread::sleep(REFRESH_INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::PullRequest;

    fn cluster(prs: &[(u32, i64)]) -> PullRequestCluster {
        PullRequestCluster {
            pull_requests: (prs.iter())
                .map(|&(number, created)| PullRequest {
                    number,
                    url: format!("https://github.com/o/r/pull/{number}"),
                    created_at: Some(Timestamp {
                        seconds: created,
                        nanos: 0,
                    }),
                    ..Default::default()
                })
                .collect(),
            updated_at: (prs.iter().map(|&(_, created)| created).max())
                .map(|seconds| Timestamp { seconds, nanos: 0 }),
            ..Default::default()
        }
    }

    fn on(numbers: &[u32]) -> Agent {
        Agent {
            pull_request_urls: (numbers.iter())
                .map(|n| format!("https://github.com/o/r/pull/{n}"))
                .collect(),
            ..Default::default()
        }
    }

    fn numbers(clusters: &[PullRequestCluster]) -> Vec<Vec<u32>> {
        (clusters.iter())
            .map(|cluster| cluster.pull_requests.iter().map(|pr| pr.number).collect())
            .collect()
    }

    #[test]
    fn an_agent_joins_the_clusters_it_acts_across_oldest_first() {
        let mut clusters = vec![
            cluster(&[(3, 30)]),
            cluster(&[(2, 20)]),
            cluster(&[(1, 10), (4, 40)]),
        ];
        join(&mut clusters, &[on(&[3, 1])]);
        assert_eq!(numbers(&clusters), [vec![1, 3, 4], vec![2]]);
        assert_eq!(clusters[0].updated_at.as_ref().map(|t| t.seconds), Some(40));
    }

    #[test]
    fn agents_within_one_cluster_or_on_closed_pull_requests_join_nothing() {
        let mut clusters = vec![cluster(&[(1, 10), (2, 20)]), cluster(&[(3, 30)])];
        join(&mut clusters, &[on(&[1, 2]), on(&[3, 9])]);
        assert_eq!(numbers(&clusters), [vec![1, 2], vec![3]]);
    }
}
