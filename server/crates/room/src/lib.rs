use std::{env, error::Error, future::Future, sync::Arc, time::Duration};

use livekit::{
    StreamTextOptions,
    prelude::{Room, RoomEvent, RoomOptions, RpcError},
};
use livekit_api::{
    access_token::{AccessToken, VideoGrants},
    services::room::RoomClient,
};
use tokio::sync::watch;
use tracing::Instrument;

// The frontend sends every request to this participant, as an RPC call to this method.
const IDENTITY: &str = "server";
const METHOD: &str = "http";
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

// Answers the frontends' requests with `handler`, which is given each request's payload and
// returns the reply, or an error code and message for the frontend to see. The requests are
// carried over a room in the user's own LiveKit project instead of a port: nothing listens on the
// network. LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET name the
// project and GEM_ROOM the room, so several servers can share a project. Each new snapshot is sent
// to the frontends in the room. Returns on SIGTERM.
//
// Every connected participant costs the project's connection minutes, so the server is in the
// room only while a frontend is.
pub async fn serve<H, F>(handler: H, mut snapshots: watch::Receiver<String>)
where
    H: Fn(String) -> F + Send + Sync + 'static,
    F: Future<Output = Result<String, (u32, String)>> + Send + 'static,
{
    let handler = Arc::new(handler);
    let url = env::var("LIVEKIT_URL").expect("LIVEKIT_URL is not set");
    let room = env::var("GEM_ROOM").expect("GEM_ROOM is not set");
    let rooms = RoomClient::new(&url).expect("LIVEKIT_API_KEY or LIVEKIT_API_SECRET is not set");
    let mut terminate = terminate();
    // Counts the times the server has been in the room, to tell one stay's events from another's.
    let mut session: u64 = 0;
    loop {
        // A room that does not exist yet has no frontend in it.
        let occupied = rooms
            .list_participants(&room)
            .await
            .is_ok_and(|p| !p.is_empty());
        if occupied {
            session += 1;
            // Everything logged during this stay in the room, requests included, carries it.
            let span = tracing::info_span!("room_session", room = %room, session);
            match join(&url, &room, &handler, &mut snapshots, &mut terminate)
                .instrument(span.clone())
                .await
            {
                Ok(Left::Terminated) => return,
                Ok(Left::Empty) => {}
                Err(error) => span.in_scope(|| tracing::error!(%error, "room lost")),
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
// the SDK reconnects through shorter outages itself. Every frontend is sent the current snapshot
// when either the server or that frontend joins, and every later one.
async fn join<H, F>(
    url: &str,
    name: &str,
    handler: &Arc<H>,
    snapshots: &mut watch::Receiver<String>,
    terminate: &mut Terminate,
) -> Result<Left, Box<dyn Error>>
where
    H: Fn(String) -> F + Send + Sync + 'static,
    F: Future<Output = Result<String, (u32, String)>> + Send + 'static,
{
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
    tracing::info!(
        frontends = room.remote_participants().len(),
        "joined the room"
    );
    let handler = handler.clone();
    // LiveKit answers requests on its own tasks, outside this stay's span unless it is named.
    let session = tracing::Span::current();
    room.local_participant()
        .register_rpc_method(METHOD.to_string(), move |data| {
            // Everything logged while answering carries the request's ID and caller.
            let span = tracing::info_span!(
                parent: &session,
                "rpc",
                request_id = %data.request_id,
                caller = %data.caller_identity,
            );
            let reply = span.in_scope(|| handler(data.payload));
            let timeout_ms = data.response_timeout.as_millis() as u64;
            Box::pin(
                async move {
                    let started = std::time::Instant::now();
                    let reply = reply.await;
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    match &reply {
                        Ok(_) => tracing::info!(elapsed_ms, timeout_ms, "rpc answered"),
                        Err((code, message)) => {
                            tracing::warn!(elapsed_ms, timeout_ms, code, message, "rpc failed")
                        }
                    }
                    reply.map_err(|(code, message)| RpcError::new(code, message, None))
                }
                .instrument(span),
            )
        });
    // The last frontend can leave between the poll and the join.
    let mut left = room.remote_participants().is_empty().then_some(Left::Empty);
    snapshots.mark_changed();
    while left.is_none() {
        tokio::select! {
            Ok(()) = snapshots.changed() => {
                let snapshot = snapshots.borrow_and_update().clone();
                // Empty until the server has filled its first snapshot.
                if snapshot.is_empty() {
                    continue;
                }
                let options = StreamTextOptions::new_with_topic(SNAPSHOT_TOPIC);
                if let Err(error) = room.local_participant().send_text(&snapshot, options).await {
                    tracing::warn!(%error, "cannot send a snapshot");
                }
            }
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    return Err(format!("disconnected: {reason:?}").into());
                }
                Some(RoomEvent::ParticipantConnected(p)) => {
                    tracing::info!(frontend = %p.identity(), "frontend connected");
                }
                // A frontend receives data only once it is active, not when it connects.
                Some(RoomEvent::ParticipantActive(p)) => {
                    tracing::info!(frontend = %p.identity(), "frontend active");
                    snapshots.mark_changed();
                }
                Some(RoomEvent::ParticipantDisconnected(p)) => {
                    let frontends = room.remote_participants().len();
                    tracing::info!(frontend = %p.identity(), frontends, "frontend disconnected");
                    if frontends == 0 {
                        left = Some(Left::Empty);
                    }
                }
                Some(RoomEvent::Reconnecting) => tracing::warn!("reconnecting to the room"),
                Some(RoomEvent::Reconnected) => tracing::info!("reconnected to the room"),
                Some(_) => {}
                None => return Err("room closed".into()),
            },
            _ = terminate.recv() => left = Some(Left::Terminated),
        }
    }
    room.close().await?;
    let left = left.unwrap();
    tracing::info!(
        reason = match left {
            Left::Empty => "no frontend left",
            Left::Terminated => "terminated",
        },
        "left the room"
    );
    Ok(left)
}
