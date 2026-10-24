use std::{env, error::Error, time::Duration};

use axum::{Router, body::Body, http::Request};
use livekit::prelude::{Room, RoomEvent, RoomOptions, RpcError};
use livekit_api::access_token::{AccessToken, VideoGrants};
use serde::Deserialize;
use serde_json::Value;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tower::ServiceExt;

// The room the frontend joins in the same project.
const ROOM: &str = "gem";
// The frontend sends every request to this participant.
const IDENTITY: &str = "server";
const REJOIN_DELAY: Duration = Duration::from_secs(5);

// The router's requests, carried over a room in the user's own LiveKit project instead of a port:
// nothing listens on the network. LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET name the
// project. Returns on SIGTERM.
pub async fn serve(app: Router) {
    let url = env::var("LIVEKIT_URL").expect("LIVEKIT_URL is not set");
    let mut terminate = signal(SignalKind::terminate()).unwrap();
    loop {
        match join(&url, &app, &mut terminate).await {
            Ok(()) => return,
            Err(error) => eprintln!("room: {error}"),
        }
        tokio::select! {
            () = tokio::time::sleep(REJOIN_DELAY) => {}
            _ = terminate.recv() => return,
        }
    }
}

// Leaves the room on SIGTERM, so the frontend sees the server go offline at once instead of after
// LiveKit's participant timeout. Errors when the room is lost; the SDK reconnects through shorter
// outages itself.
async fn join(url: &str, app: &Router, terminate: &mut Signal) -> Result<(), Box<dyn Error>> {
    let token = AccessToken::new()?
        .with_identity(IDENTITY)
        .with_grants(VideoGrants {
            room_join: true,
            room: ROOM.to_string(),
            can_publish: Some(false),
            can_subscribe: Some(false),
            can_publish_data: Some(true),
            ..Default::default()
        })
        .to_jwt()?;
    let (room, mut events) = Room::connect(url, &token, RoomOptions::default()).await?;
    let app = app.clone();
    room.local_participant()
        .register_rpc_method("http".to_string(), move |data| {
            Box::pin(handle(app.clone(), data.payload))
        });
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    return Err(format!("disconnected: {reason:?}").into());
                }
                Some(_) => {}
                None => return Err("room closed".into()),
            },
            _ = terminate.recv() => {
                room.close().await?;
                return Ok(());
            }
        }
    }
}

#[derive(Deserialize)]
struct HttpRequest {
    method: String,
    path: String,
    body: Value,
}

// The payload is a JSON request to the router; the response is the router's body, or an RPC error
// whose code is the HTTP status.
async fn handle(app: Router, payload: String) -> Result<String, RpcError> {
    let bad_request = |error: &dyn Error| RpcError::new(400, error.to_string(), None);
    let request: HttpRequest = serde_json::from_str(&payload).map_err(|e| bad_request(&e))?;
    let request = Request::builder()
        .method(request.method.as_str())
        .uri(request.path)
        .header("content-type", "application/json")
        .body(Body::from(request.body.to_string()))
        .map_err(|e| bad_request(&e))?;
    let response = app.oneshot(request).await.unwrap();
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
