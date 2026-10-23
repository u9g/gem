use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

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
        number title url headRefName baseRefName
        repository { nameWithOwner }
        headRepository { nameWithOwner }
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
  base: .baseRefName
}"#;

#[derive(Deserialize)]
struct SearchedPullRequest {
    repository: String,
    number: u32,
    title: String,
    url: String,
    head: Option<String>,
    base: String,
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

// A PR has one base branch, so dependencies form a forest; each root PR starts a cluster.
fn cluster(prs: &[SearchedPullRequest]) -> Vec<PullRequestCluster> {
    let heads: HashSet<(&str, &str)> = prs
        .iter()
        .filter_map(|pr| Some((pr.repository.as_str(), pr.head.as_deref()?)))
        .collect();
    let mut dependents: HashMap<(&str, &str), Vec<&SearchedPullRequest>> = HashMap::new();
    for pr in prs {
        dependents
            .entry((pr.repository.as_str(), pr.base.as_str()))
            .or_default()
            .push(pr);
    }

    prs.iter()
        .filter(|pr| !heads.contains(&(pr.repository.as_str(), pr.base.as_str())))
        .map(|root| {
            let mut pull_requests = Vec::new();
            let mut stack = vec![root];
            while let Some(pr) = stack.pop() {
                pull_requests.push(PullRequest {
                    repository: pr.repository.clone(),
                    number: pr.number,
                    title: pr.title.clone(),
                    url: pr.url.clone(),
                });
                if let Some(head) = &pr.head {
                    if let Some(children) = dependents.get(&(pr.repository.as_str(), head.as_str()))
                    {
                        stack.extend(children.iter().rev());
                    }
                }
            }
            PullRequestCluster { pull_requests }
        })
        .collect()
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
