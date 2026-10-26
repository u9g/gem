use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::{
    gh,
    proto::{FileDiff, PullRequest, PullRequestCluster},
};

// How long a PR whose diff could not be fetched waits before it is asked for again.
const RETRY: Duration = Duration::from_secs(60);

enum Diff {
    Fetching,
    Fetched(Vec<FileDiff>),
    Failed(Instant),
}

// Each PR's diff by file at its latest head commit, by URL; only a push changes it.
static DIFFS: LazyLock<Mutex<HashMap<String, (String, Diff)>>> = LazyLock::new(Default::default);

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
                let changed = (patch.lines())
                    .filter_map(|line| line.strip_prefix("+++ b/"))
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

// The PR's diff when fetched for its head; otherwise starts fetching it, unless that already
// started or failed within RETRY.
async fn diff(pr: &PullRequest) -> Vec<FileDiff> {
    let mut diffs = DIFFS.lock().await;
    match diffs.get(&pr.url) {
        Some((head, Diff::Fetched(files))) if *head == pr.head_oid => return files.clone(),
        Some((head, Diff::Fetching)) if *head == pr.head_oid => return Vec::new(),
        Some((head, Diff::Failed(at))) if *head == pr.head_oid && at.elapsed() < RETRY => {
            return Vec::new();
        }
        _ => {}
    }
    diffs.insert(pr.url.clone(), (pr.head_oid.clone(), Diff::Fetching));
    let pr = pr.clone();
    tokio::spawn(async move {
        let diff = fetch(&pr).await.map_or_else(|| Diff::Failed(Instant::now()), Diff::Fetched);
        DIFFS.lock().await.insert(pr.url, (pr.head_oid, diff));
    });
    Vec::new()
}

async fn fetch(pr: &PullRequest) -> Option<Vec<FileDiff>> {
    let repo = pr.repository.as_ref()?;
    let stdout = gh(&[
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
