use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use tokio::sync::Mutex;

use crate::{
    gh,
    proto::{FileDiff, PullRequest, PullRequestCluster},
};

// Each PR's diff by file, keyed by its URL and head commit: only a push changes it.
static DIFFS: LazyLock<Mutex<HashMap<(String, String), Vec<FileDiff>>>> =
    LazyLock::new(Default::default);

// Gives each PR with proposals its diff of the files they are about.
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
        pr.files = (diff(pr).await.into_iter())
            .filter(|file| paths.contains(&file.path))
            .collect();
    }
}

async fn diff(pr: &PullRequest) -> Vec<FileDiff> {
    let key = (pr.url.clone(), pr.head_oid.clone());
    if let Some(files) = DIFFS.lock().await.get(&key) {
        return files.clone();
    }
    let Some(repo) = &pr.repository else {
        return Vec::new();
    };
    let Some(stdout) = gh(&[
        "api",
        "--paginate",
        &format!(
            "repos/{}/{}/pulls/{}/files?per_page=100",
            repo.owner, repo.name, pr.number
        ),
        "--jq",
        ".[] | {path: .filename, patch: (.patch // \"\")}",
    ])
    .await
    else {
        return Vec::new();
    };
    let Ok(files) = serde_json::Deserializer::from_slice(&stdout)
        .into_iter()
        .collect::<Result<Vec<FileDiff>, _>>()
    else {
        return Vec::new();
    };
    DIFFS.lock().await.insert(key, files.clone());
    files
}
