use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use tokio::signal::unix::{SignalKind, signal};

#[tokio::main]
async fn main() {
    let app = Router::new().route("/healthz", get(healthz));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .unwrap();
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

// Kubernetes sends SIGTERM before killing the pod; in-flight requests must drain.
async fn shutdown() {
    signal(SignalKind::terminate()).unwrap().recv().await;
}
