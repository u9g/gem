use std::{env, error::Error, time::Duration};

use axum::{Router, body::Body, http::Request};
use livekit::prelude::{Room, RoomEvent, RoomOptions, RpcError};
use livekit_api::{
    access_token::{AccessToken, VideoGrants},
    services::room::RoomClient,
};
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

// The room the frontend joins in the same project.
const ROOM: &str = "gem";
// The frontend sends every request to this participant.
const IDENTITY: &str = "server";
// How long a frontend waits for the server to join after the frontend does.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

// SIGTERM, or Ctrl+C on Windows, which has no SIGTERM.
#[cfg(unix)]
type Terminate = tokio::signal::unix::Signal;
#[cfg(windows)]
type Terminate = tokio::signal::windows::CtrlC;

fn terminate() -> Terminate {
    #[cfg(unix)]
    return tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    #[cfg(windows)]
    return tokio::signal::windows::ctrl_c().unwrap();
}

enum Left {
    Empty,
    Terminated,
}

// The router's requests, carried over a room in the user's own LiveKit project instead of a port:
// nothing listens on the network. LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET name the
// project. Returns on SIGTERM.
//
// Every connected participant costs the project's connection minutes, so the server is in the
// room only while a frontend is.
pub async fn serve(app: Router) {
    let url = env::var("LIVEKIT_URL").expect("LIVEKIT_URL is not set");
    let rooms = RoomClient::new(&url).expect("LIVEKIT_API_KEY or LIVEKIT_API_SECRET is not set");
    let mut terminate = terminate();
    loop {
        // A room that does not exist yet has no frontend in it.
        let occupied = rooms.list_participants(ROOM).await.is_ok_and(|p| !p.is_empty());
        if occupied {
            match join(&url, &app, &mut terminate).await {
                Ok(Left::Terminated) => return,
                Ok(Left::Empty) => {}
                Err(error) => eprintln!("room: {error}"),
            }
        }
        tokio::select! {
            () = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = terminate.recv() => return,
        }
    }
}

// Leaves the room once no frontend is in it, and on SIGTERM, so the frontend sees the server go
// offline at once instead of after LiveKit's participant timeout. Errors when the room is lost;
// the SDK reconnects through shorter outages itself.
async fn join(url: &str, app: &Router, terminate: &mut Terminate) -> Result<Left, Box<dyn Error>> {
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
    // The last frontend can leave between the poll and the join.
    let mut left = room.remote_participants().is_empty().then_some(Left::Empty);
    while left.is_none() {
        tokio::select! {
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    return Err(format!("disconnected: {reason:?}").into());
                }
                Some(RoomEvent::ParticipantDisconnected(_))
                    if room.remote_participants().is_empty() =>
                {
                    left = Some(Left::Empty);
                }
                Some(_) => {}
                None => return Err("room closed".into()),
            },
            _ = terminate.recv() => left = Some(Left::Terminated),
        }
    }
    room.close().await?;
    Ok(left.unwrap())
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
