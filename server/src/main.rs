use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    process::Command,
    signal::unix::{SignalKind, signal},
    sync::RwLock,
};
use tower_http::cors::CorsLayer;

use proto::{ListOpenPullRequestsRequest, ListOpenPullRequestsResponse};

mod proto {
    include!(concat!(env!("OUT_DIR"), "/gem.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/gem.v1.serde.rs"));
}

type Cache<T> = Arc<RwLock<Option<T>>>;

const REPOS: &[&str] = &[
    "repo", "list", "u9g", "--limit", "1000", "--json", "name,url",
];

const PRS: &[&str] = &[
    "search",
    "prs",
    "--owner",
    "livekit",
    "--owner",
    "livekit-examples",
    "--author",
    "u9g",
    "--state",
    "open",
    "--limit",
    "1000",
    "--json",
    "number,title,url,repository",
    "--jq",
    "{pullRequests: map({repository: .repository.nameWithOwner, number, title, url})}",
];

// GitHub search allows 30 requests/min; each refresh costs one request per 100 results.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() {
    let repos: Cache<Value> = cached(REPOS);
    let prs: Cache<ListOpenPullRequestsResponse> = cached(PRS);

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
fn cached<T: DeserializeOwned + Send + Sync + 'static>(args: &'static [&'static str]) -> Cache<T> {
    let cache = Cache::default();
    let writer = cache.clone();
    tokio::spawn(async move {
        loop {
            if let Some(value) = gh(args).await {
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

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
async fn gh<T: DeserializeOwned>(args: &[&str]) -> Option<T> {
    let output = Command::new("gh").args(args).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

// Kubernetes sends SIGTERM before killing the pod; in-flight requests must drain.
async fn shutdown() {
    signal(SignalKind::terminate()).unwrap().recv().await;
}
