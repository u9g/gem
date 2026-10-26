use std::{
    collections::{HashMap, HashSet},
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::{
    gh,
    proto::{FileDiff, PullRequest, PullRequestCluster},
};

// How long a PR whose diff could not be fetched waits before it is asked for again, and how long a
// fetch may take before it counts as failed.
const RETRY: Duration = Duration::from_secs(60);

enum Diff {
    // The previous diff, shown meanwhile so threads stay where they were.
    Fetching(Vec<FileDiff>),
    Fetched(Vec<FileDiff>),
    Failed(Instant),
}

// What a PR's diff is of: its head commit and the branch it merges into. Only a push or a retarget
// changes it.
fn of(pr: &PullRequest) -> (String, String) {
    (pr.head_oid.clone(), pr.base.clone())
}

// Each PR's diff by file, by URL, with what it is of and the fetch that wrote it.
static DIFFS: LazyLock<Mutex<HashMap<String, ((String, String), u64, Diff)>>> =
    LazyLock::new(Default::default);
static FETCHES: AtomicU64 = AtomicU64::new(0);

// Gives each PR with proposals its diff of the files they are about, once fetched: a snapshot
// never waits on GitHub.
pub async fn fill(clusters: &mut [PullRequestCluster]) {
    for pr in clusters.iter_mut().flat_map(|c| c.pull_requests.iter_mut()) {
        if pr.proposals.is_empty() {
            continue;
        }
        let paths: HashSet<String> = (pr.proposals.iter())
            .flat_map(|p| {
                let patch = p.patch.as_ref().map_or("", |patch| &patch.diff);
                // A deleted file's new side is /dev/null.
                let changed = (patch.lines())
                    .filter_map(|line| line.strip_prefix("+++ b/").or(line.strip_prefix("--- a/")))
                    .map(str::to_string);
                p.comment.iter().map(|c| c.path.clone()).chain(changed)
            })
            .filter(|path| !path.is_empty())
            .collect();
        if paths.is_empty() {
            continue;
        }
        pr.files = (diff(pr).await.into_iter())
            .filter(|file| paths.contains(&file.path))
            .collect();
    }
}

// The PR's diff when fetched for what it is now of; otherwise starts fetching it, unless that
// already started or failed within RETRY.
async fn diff(pr: &PullRequest) -> Vec<FileDiff> {
    let mut diffs = DIFFS.lock().await;
    match diffs.get(&pr.url) {
        Some((of_, _, Diff::Fetched(files) | Diff::Fetching(files))) if *of_ == of(pr) => {
            return files.clone();
        }
        Some((of_, _, Diff::Failed(at))) if *of_ == of(pr) && at.elapsed() < RETRY => {
            return Vec::new();
        }
        _ => {}
    }
    let previous = match diffs.remove(&pr.url) {
        Some((_, _, Diff::Fetched(files) | Diff::Fetching(files))) => files,
        _ => Vec::new(),
    };
    let fetch_id = FETCHES.fetch_add(1, Ordering::Relaxed);
    diffs.insert(pr.url.clone(), (of(pr), fetch_id, Diff::Fetching(previous.clone())));
    let pr = pr.clone();
    tokio::spawn(async move {
        let files = tokio::time::timeout(RETRY, fetch(&pr)).await.ok().flatten();
        let diff = files.map_or_else(|| Diff::Failed(Instant::now()), Diff::Fetched);
        // A later fetch of the PR, after a push or retarget, owns its entry now.
        if let Some(entry) = DIFFS.lock().await.get_mut(&pr.url)
            && entry.1 == fetch_id
        {
            entry.2 = diff;
        }
    });
    previous
}

async fn fetch(pr: &PullRequest) -> Option<Vec<FileDiff>> {
    let repo = pr.repository.as_ref()?;
    let stdout = gh("files", &[
        "api",
        "--paginate",
        &format!(
            "repos/{}/{}/pulls/{}/files?per_page=100",
            repo.owner, repo.name, pr.number
        ),
        "--jq",
        ".[] | {path: .filename, patch: (.patch // \"\")}",
    ])
    .await?;
    serde_json::Deserializer::from_slice(&stdout)
        .into_iter()
        .collect::<Result<Vec<FileDiff>, _>>()
        .ok()
}
