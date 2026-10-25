// HTTP over the room: each request to the router arrives as an RPC call named http, whose payload
// is the request as JSON, and goes back as the router's body, or an RPC error whose code is the
// HTTP status. The room is to the router what a TCP listener is to `axum::serve`.

use std::error::Error;

use axum::{Router, body::Body, http::Request, response::Response};
use livekit::prelude::{LocalParticipant, RpcError};
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

const METHOD: &str = "http";

// Answers the participant's http calls with `app` until it leaves the room.
pub fn serve(participant: &LocalParticipant, app: Router) {
    participant.register_rpc_method(METHOD.to_string(), move |data| {
        let app = app.clone();
        Box::pin(async move {
            let request = request(&data.payload)?;
            let response = app.oneshot(request).await.unwrap();
            payload(response).await
        })
    });
}

fn request(payload: &str) -> Result<Request<Body>, RpcError> {
    #[derive(Deserialize)]
    struct HttpRequest {
        method: String,
        path: String,
        body: Value,
    }

    let bad_request = |error: &dyn Error| RpcError::new(400, error.to_string(), None);
    let request: HttpRequest = serde_json::from_str(payload).map_err(|e| bad_request(&e))?;
    Request::builder()
        .method(request.method.as_str())
        .uri(request.path)
        .header("content-type", "application/json")
        .body(Body::from(request.body.to_string()))
        .map_err(|e| bad_request(&e))
}

async fn payload(response: Response) -> Result<String, RpcError> {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| RpcError::new(500, e.to_string(), None))?;
    let body = String::from_utf8_lossy(&body).into_owned();
    if status.is_success() {
        Ok(body)
    } else {
        Err(RpcError::new(status.as_u16().into(), body, None))
    }
}
