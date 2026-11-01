use std::{
    collections::HashMap,
    env,
    error::Error,
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

pub use gem_db::Delta;
use gem_db::{Db, Replica, ReplicaState};
use livekit::{
    StreamByteOptions, StreamResult,
    prelude::{LocalParticipant, ParticipantIdentity, Room, RoomEvent, RoomOptions, RpcError},
};
use livekit_api::{
    access_token::{AccessToken, VideoGrants},
    services::room::RoomClient,
};
use tokio::sync::{
    broadcast::{self, error::RecvError},
    mpsc,
};
use tracing::Instrument;

mod frontends;
use frontends::Frontends;

// The frontend sends every request to this participant, as an RPC call to this method.
const IDENTITY: &str = "server";
const METHOD: &str = "http";
// How long a frontend waits for the server to join after the frontend does.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
// The server sends each frontend the view, and every change to it, as byte streams on VIEW_TOPIC:
// the first copy is far bigger than the 15 KiB an RPC payload can be. Each frontend reports its
// replica's state in a data message on STATE_TOPIC when it connects and after each stream. Neither
// is an RPC call, so neither waits on a method the server registers only once it's in the room,
// where frontends can already reach it.
const VIEW_TOPIC: &str = "view";
const STATE_TOPIC: &str = "view-state";
// How often the server looks for frontends that haven't caught up in time.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

// What a frontend reports: `null` for an empty replica, and otherwise its file ID, a u64 as a
// decimal string, with its state as xitdb's serde writes it, the checksum a decimal string too:
// `{"fileId":"789","committedSize":123,"checksum":"456"}`.
#[derive(serde::Deserialize)]
struct Report {
    #[serde(rename = "fileId")]
    file_id: String,
    #[serde(flatten)]
    state: ReplicaState,
}

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
// project and GEM_ROOM the room, so several servers can share a project. Each frontend keeps a
// replica of `view`: each change sent on `changes` goes to every frontend in the room, and each
// is sent the bytes it lacks besides, as it reports what it has. Returns on SIGTERM.
//
// Every connected participant costs the project's connection minutes, so the server is in the
// room only while a frontend is.
pub async fn serve<H, F>(handler: H, view: Db, changes: broadcast::Sender<Arc<Delta>>)
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
            let changes = changes.subscribe();
            match join(&url, &room, &handler, &view, changes, &mut terminate)
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
// the SDK reconnects through shorter outages itself.
async fn join<H, F>(
    url: &str,
    name: &str,
    handler: &Arc<H>,
    view: &Db,
    mut changes: broadcast::Receiver<Arc<Delta>>,
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
    // Frontends see the server as soon as it's in the room, before this is registered: the SDK
    // registers a method only on a connected room's session.
    let handler = handler.clone();
    register(&room, METHOD, move |payload, _| handler(payload));
    let mut frontends = Frontends::default();
    // Bytes for one frontend alone, such as a whole copy, are sent on tasks of their own, so
    // changes and events go on meanwhile, and each task says when it's done.
    let (done, mut sent) = mpsc::unbounded_channel::<String>();
    let send_to = |id: String, delta: Delta| {
        let (participant, done) = (room.local_participant(), done.clone());
        let sending = async move {
            let to = vec![ParticipantIdentity(id.clone())];
            match send(&participant, &delta, to).await {
                Ok(()) => tracing::info!(frontend = id, size = delta.bytes.len(), "view sent"),
                Err(error) => tracing::warn!(frontend = id, %error, "cannot send the view"),
            }
            let _ = done.send(id);
        };
        tokio::spawn(sending.in_current_span());
    };
    // Frontends in the room before the server are active already.
    for id in room.remote_participants().keys() {
        match frontends.active(id.as_str(), view) {
            Ok(Some(delta)) => send_to(id.to_string(), delta),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "cannot read the view"),
        }
    }
    let mut check = tokio::time::interval(CHECK_INTERVAL);
    // The last frontend can leave between the poll and the join.
    let mut left = room.remote_participants().is_empty().then_some(Left::Empty);
    while left.is_none() {
        tokio::select! {
            change = changes.recv() => match change {
                Ok(delta) => {
                    if let Err(error) = send(&room.local_participant(), &delta, vec![]).await {
                        tracing::warn!(%error, "cannot send a change to the view");
                    }
                    frontends.changed(&delta, Instant::now());
                }
                // A frontend that misses a change is sent the bytes it lacks once it reports a
                // replica that's behind, or doesn't report for long enough.
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "changes to the view were not sent");
                }
                Err(RecvError::Closed) => return Err("the view is gone".into()),
            },
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    return Err(format!("disconnected: {reason:?}").into());
                }
                Some(RoomEvent::ParticipantConnected(p)) => {
                    tracing::info!(frontend = %p.identity(), "frontend connected");
                }
                // A frontend receives data only once it is active, not when it connects, so it
                // is sent the view then.
                Some(RoomEvent::ParticipantActive(p)) => {
                    tracing::info!(frontend = %p.identity(), "frontend active");
                    match frontends.active(p.identity().as_str(), view) {
                        Ok(Some(delta)) => send_to(p.identity().to_string(), delta),
                        Ok(None) => {}
                        Err(error) => tracing::warn!(%error, "cannot read the view"),
                    }
                }
                Some(RoomEvent::DataReceived { payload, topic, participant: Some(p), .. })
                    if topic.as_deref() == Some(STATE_TOPIC) =>
                {
                    let id = p.identity().to_string();
                    let replica = match serde_json::from_slice::<Option<Report>>(&payload) {
                        Ok(None) => Ok(None),
                        Ok(Some(report)) => (report.file_id.parse())
                            .map(|file_id| Some(Replica { file_id, state: report.state }))
                            .map_err(|error| error.to_string()),
                        Err(error) => Err(error.to_string()),
                    };
                    let delta = replica.and_then(|replica| {
                        frontends.report(&id, replica, view, Instant::now())
                    });
                    match delta {
                        Ok(Some(delta)) => send_to(id, delta),
                        Ok(None) => {}
                        Err(error) => tracing::warn!(frontend = id, %error, "bad report"),
                    }
                }
                Some(RoomEvent::ParticipantDisconnected(p)) => {
                    frontends.left(p.identity().as_str());
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
            Some(id) = sent.recv() => frontends.sent(&id, Instant::now()),
            _ = check.tick() => match frontends.due(view, Instant::now()) {
                Ok(due) => due.into_iter().for_each(|(id, delta)| send_to(id, delta)),
                Err(error) => tracing::warn!(%error, "cannot read the view"),
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

// Answers RPC calls to `method` with `answer`, which is given each call's payload and caller and
// returns the reply, or an error code and message for the frontend to see.
fn register<A, F>(room: &Room, method: &'static str, answer: A)
where
    A: Fn(String, ParticipantIdentity) -> F + Send + Sync + 'static,
    F: Future<Output = Result<String, (u32, String)>> + Send + 'static,
{
    // LiveKit answers requests on its own tasks, outside this stay's span unless it is named.
    let session = tracing::Span::current();
    room.local_participant().register_rpc_method(method.to_string(), move |data| {
        // Everything logged while answering carries the request's method, ID and caller.
        let span = tracing::info_span!(
            parent: &session,
            "rpc",
            method,
            request_id = %data.request_id,
            caller = %data.caller_identity,
        );
        let reply = span.in_scope(|| answer(data.payload, data.caller_identity));
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
}

// Sends `delta` on VIEW_TOPIC to the frontends `to`, or to all of them when it is empty, with
// attributes saying which bytes they are: the file ID as a decimal string, since it is a u64 and
// JavaScript's numbers are not.
async fn send(
    participant: &LocalParticipant,
    delta: &Delta,
    to: Vec<ParticipantIdentity>,
) -> StreamResult<()> {
    let attributes = HashMap::from([
        ("fileId".to_string(), delta.file_id.to_string()),
        ("start".to_string(), delta.start.to_string()),
        ("end".to_string(), delta.end.to_string()),
    ]);
    let options = StreamByteOptions {
        destination_identities: to,
        ..StreamByteOptions::new_with_topic(VIEW_TOPIC).with_attributes(attributes)
    };
    participant.send_bytes(&delta.bytes, options).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_reads_as_the_frontend_writes_it() {
        let json = br#"{"fileId":"18446744073709551615","committedSize":123,"checksum":"456"}"#;
        let report: Option<Report> = serde_json::from_slice(json).unwrap();
        let report = report.unwrap();
        assert_eq!(report.file_id.parse::<u64>().unwrap(), u64::MAX);
        assert_eq!((report.state.committed_size, report.state.checksum), (123, 456));
        assert!(serde_json::from_slice::<Option<Report>>(b"null").unwrap().is_none());
    }
}
