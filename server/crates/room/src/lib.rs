mod http;

use std::{env, error::Error, time::Duration};

use axum::Router;
use livekit::{
    StreamTextOptions,
    prelude::{Room, RoomEvent, RoomOptions},
};
use livekit_api::{
    access_token::{AccessToken, VideoGrants},
    services::room::RoomClient,
};
use tokio::sync::watch;

// The frontend sends every request to this participant.
const IDENTITY: &str = "server";
// How long a frontend waits for the server to join after the frontend does.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
// Every frontend in the room receives the snapshots on this topic.
const SNAPSHOT_TOPIC: &str = "prs";

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
// project and GEM_ROOM the room, so several servers can share a project. Each new snapshot is sent
// to the frontends in the room. Returns on SIGTERM.
//
// Every connected participant costs the project's connection minutes, so the server is in the
// room only while a frontend is.
pub async fn serve(app: Router, mut snapshots: watch::Receiver<String>) {
    let url = env::var("LIVEKIT_URL").expect("LIVEKIT_URL is not set");
    let room = env::var("GEM_ROOM").expect("GEM_ROOM is not set");
    let rooms = RoomClient::new(&url).expect("LIVEKIT_API_KEY or LIVEKIT_API_SECRET is not set");
    let mut terminate = terminate();
    loop {
        // A room that does not exist yet has no frontend in it.
        let occupied = rooms
            .list_participants(&room)
            .await
            .is_ok_and(|p| !p.is_empty());
        if occupied {
            match join(&url, &room, &app, &mut snapshots, &mut terminate).await {
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
// the SDK reconnects through shorter outages itself. A frontend loads the current snapshot when
// the server joins, so only later ones are sent.
async fn join(
    url: &str,
    name: &str,
    app: &Router,
    snapshots: &mut watch::Receiver<String>,
    terminate: &mut Terminate,
) -> Result<Left, Box<dyn Error>> {
    let token = AccessToken::new()?
        .with_identity(IDENTITY)
        .with_grants(VideoGrants {
            room_join: true,
            room: name.to_string(),
            can_publish: Some(false),
            can_subscribe: Some(false),
            can_publish_data: Some(true),
            ..Default::default()
        })
        .to_jwt()?;
    let (room, mut events) = Room::connect(url, &token, RoomOptions::default()).await?;
    http::serve(&room.local_participant(), app.clone());
    // The last frontend can leave between the poll and the join.
    let mut left = room.remote_participants().is_empty().then_some(Left::Empty);
    snapshots.mark_unchanged();
    while left.is_none() {
        tokio::select! {
            Ok(()) = snapshots.changed() => {
                let snapshot = snapshots.borrow_and_update().clone();
                let options = StreamTextOptions::new_with_topic(SNAPSHOT_TOPIC);
                if let Err(error) = room.local_participant().send_text(&snapshot, options).await {
                    eprintln!("room: cannot send a snapshot: {error}");
                }
            }
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
