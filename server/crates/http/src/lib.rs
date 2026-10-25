// HTTP without a socket: a request to the router arrives as JSON naming its method, path and body,
// and is answered with the router's body, or with the HTTP status and body when it fails. Nothing
// here knows what carries the requests.

use std::error::Error;

use axum::{Router, body::Body, http::Request, response::Response};
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

// The router's answer to `request`. A request that is not one fails with 400.
pub async fn respond(app: Router, request: String) -> Result<String, (u32, String)> {
    let request = parse(&request).inspect_err(|(code, message)| {
        tracing::warn!(code, message, "unparseable request");
    })?;
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    // `/prs` is asked for after every action and on every reconnect.
    if path == "/prs" {
        tracing::debug!(%method, path, status, "request");
    } else {
        tracing::info!(%method, path, status, "request");
    }
    body(response).await
}

fn parse(request: &str) -> Result<Request<Body>, (u32, String)> {
    #[derive(Deserialize)]
    struct HttpRequest {
        method: String,
        path: String,
        body: Value,
    }

    let bad_request = |error: &dyn Error| (400, error.to_string());
    let request: HttpRequest = serde_json::from_str(request).map_err(|e| bad_request(&e))?;
    Request::builder()
        .method(request.method.as_str())
        .uri(request.path)
        .header("content-type", "application/json")
        .body(Body::from(request.body.to_string()))
        .map_err(|e| bad_request(&e))
}

async fn body(response: Response) -> Result<String, (u32, String)> {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| (500, e.to_string()))?;
    let body = String::from_utf8_lossy(&body).into_owned();
    if status.is_success() {
        Ok(body)
    } else {
        Err((status.as_u16().into(), body))
    }
}
