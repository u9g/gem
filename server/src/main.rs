use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    process::Command,
    signal::unix::{SignalKind, signal},
    sync::RwLock,
};
use tower_http::cors::CorsLayer;

use proto::{
    ListOpenPullRequestsRequest, ListOpenPullRequestsResponse, PullRequest, PullRequestCluster,
};

mod proto {
    include!(concat!(env!("OUT_DIR"), "/gem.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/gem.v1.serde.rs"));
}

type Cache<T> = Arc<RwLock<Option<T>>>;

const REPOS: &[&str] = &[
    "repo", "list", "u9g", "--limit", "1000", "--json", "name,url",
];

// Paginated by `gh api graphql --paginate`, which fills in $endCursor.
const PRS_QUERY: &str = r#"
query($endCursor: String) {
  search(query: "is:pr is:open author:u9g org:livekit org:livekit-examples", type: ISSUE, first: 100, after: $endCursor) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        number title url headRefName baseRefName createdAt
        repository { nameWithOwner }
        headRepository { nameWithOwner }
        timelineItems(itemTypes: [CROSS_REFERENCED_EVENT], first: 100) {
          nodes { ... on CrossReferencedEvent { source { ... on PullRequest { url } } } }
        }
      }
    }
  }
}
"#;

// A fork's head branch can share a name with a base-repo branch (e.g. `main`), so only
// same-repository heads can have dependents.
const PRS_JQ: &str = r#".data.search.nodes[] | {
  repository: .repository.nameWithOwner, number, title, url,
  head: (if .headRepository.nameWithOwner == .repository.nameWithOwner then .headRefName else null end),
  base: .baseRefName,
  created_at: .createdAt,
  mentioned_by: [.timelineItems.nodes[].source.url | select(. != null)]
}"#;

#[derive(Deserialize)]
struct SearchedPullRequest {
    repository: String,
    number: u32,
    title: String,
    url: String,
    head: Option<String>,
    base: String,
    // RFC 3339 UTC, so it sorts chronologically as a string.
    created_at: String,
    // URLs of PRs whose description or comments link to this one.
    mentioned_by: Vec<String>,
}

// A refresh costs one GitHub API request per 100 results, far under GitHub's hourly limits.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() {
    let repos: Cache<Value> = cached(|| gh_json(REPOS));
    let prs: Cache<ListOpenPullRequestsResponse> = cached(open_pull_requests);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/repos", get(move || serve(repos)))
        .route(
            "/prs",
            post(move |Json(_): Json<ListOpenPullRequestsRequest>| serve(prs)),
        )
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .unwrap();
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

// Requests never call GitHub; a failed refresh keeps serving the last good result.
fn cached<T, F, Fut>(refresh: F) -> Cache<T>
where
    T: Send + Sync + 'static,
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Option<T>> + Send,
{
    let cache = Cache::default();
    let writer = cache.clone();
    tokio::spawn(async move {
        loop {
            if let Some(value) = refresh().await {
                *writer.write().await = Some(value);
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
    cache
}

async fn serve<T: Serialize + Clone>(cache: Cache<T>) -> Result<Json<T>, StatusCode> {
    cache
        .read()
        .await
        .clone()
        .map(Json)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

async fn open_pull_requests() -> Option<ListOpenPullRequestsResponse> {
    let stdout = gh(&[
        "api",
        "graphql",
        "--paginate",
        "-f",
        &format!("query={PRS_QUERY}"),
        "--jq",
        PRS_JQ,
    ])
    .await?;
    let prs = serde_json::Deserializer::from_slice(&stdout)
        .into_iter()
        .collect::<Result<Vec<SearchedPullRequest>, _>>()
        .ok()?;
    Some(ListOpenPullRequestsResponse {
        clusters: cluster(&prs),
    })
}

// Two PRs are linked when one is stacked on the other (its base branch is the other's head
// branch in the same repository) or one mentions the other. A cluster is a connected component
// of links, so PRs join through any chain of links, not only direct ones.
fn cluster(prs: &[SearchedPullRequest]) -> Vec<PullRequestCluster> {
    let by_url: HashMap<&str, usize> = prs
        .iter()
        .enumerate()
        .map(|(i, pr)| (pr.url.as_str(), i))
        .collect();
    let by_head: HashMap<(&str, &str), usize> = prs
        .iter()
        .enumerate()
        .filter_map(|(i, pr)| Some(((pr.repository.as_str(), pr.head.as_deref()?), i)))
        .collect();

    let mut parent: Vec<usize> = (0..prs.len()).collect();
    for (i, pr) in prs.iter().enumerate() {
        let stacked_on = by_head.get(&(pr.repository.as_str(), pr.base.as_str()));
        let mentioned_by = pr
            .mentioned_by
            .iter()
            .filter_map(|url| by_url.get(url.as_str()));
        for &j in stacked_on.into_iter().chain(mentioned_by) {
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            parent[a] = b;
        }
    }

    let mut clusters: Vec<Vec<&SearchedPullRequest>> = Vec::new();
    let mut cluster_of_root: HashMap<usize, usize> = HashMap::new();
    for (i, pr) in prs.iter().enumerate() {
        let r = root(&mut parent, i);
        let c = *cluster_of_root.entry(r).or_insert_with(|| {
            clusters.push(Vec::new());
            clusters.len() - 1
        });
        clusters[c].push(pr);
    }

    clusters
        .into_iter()
        .map(|mut members| {
            members.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            PullRequestCluster {
                pull_requests: members
                    .into_iter()
                    .map(|pr| PullRequest {
                        repository: pr.repository.clone(),
                        number: pr.number,
                        title: pr.title.clone(),
                        url: pr.url.clone(),
                    })
                    .collect(),
            }
        })
        .collect()
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
async fn gh(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("gh").args(args).output().await.ok()?;
    output.status.success().then_some(output.stdout)
}

async fn gh_json<T: DeserializeOwned>(args: &[&str]) -> Option<T> {
    serde_json::from_slice(&gh(args).await?).ok()
}

// Kubernetes sends SIGTERM before killing the pod; in-flight requests must drain.
async fn shutdown() {
    signal(SignalKind::terminate()).unwrap().recv().await;
}
