import type { JsonValue } from '@bufbuild/protobuf';
import { ConnectionState, type Participant, Room, RoomEvent } from 'livekit-client';
import { SignJWT } from 'jose';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { concat, Replica, type View, type ViewBytes, viewBytes } from './view';

const PROFILES_KEY = 'gem.profiles';
const SELECTED_KEY = 'gem.profile';
// Where the one project was saved before there were profiles, when every server joined the room gem.
const PROJECT_KEY = 'gem.project';
// The server joins its profile's room, its GEM_ROOM, as SERVER.
const SERVER = 'server';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;
// The frontend asks for the bytes its replica of the server's view lacks with an RPC call to this
// method: when it connects, when the server joins, and when a change doesn't follow on from its
// replica. The server sends them, like every change to the view, as a byte stream on VIEW_TOPIC.
const SYNC_METHOD = 'view-sync';
const VIEW_TOPIC = 'view';
// The server sends the bytes before it answers, and the first copy can take a while.
const SYNC_TIMEOUT_MS = 60_000;
// How many changes that arrived ahead of the replica are kept until it catches up to them.
const AHEAD = 16;

// The user's own LiveKit project, the one their gem server is configured with. It never leaves
// this browser: tokens are minted here.
export type Project = { url: string; apiKey: string; apiSecret: string };

// One gem server: the project and room it serves in, and a name to pick it by. Each server lists
// one GitHub user's PRs narrowed by its GEM_SEARCH; login and search are what it last reported, to
// label it while another profile is connected.
export type Profile = Project & { id: string; name: string; room: string; login?: string; search?: string };

// Unreachable when the project refuses to connect, e.g. for a wrong URL, API key or secret;
// offline when the room is reached but the server is not in it.
export type ServerStatus = 'no-profile' | 'connecting' | 'unreachable' | 'offline' | 'online';

// A LiveKit access token that can only exchange data in the profile's room.
function mintToken(profile: Profile): Promise<string> {
  return new SignJWT({
    video: { room: profile.room, roomJoin: true, canPublish: false, canSubscribe: false, canPublishData: true },
  })
    .setProtectedHeader({ alg: 'HS256' })
    .setIssuer(profile.apiKey)
    .setSubject(crypto.randomUUID())
    .setNotBefore('0s')
    .setExpirationTime('6h')
    .sign(new TextEncoder().encode(profile.apiSecret));
}

function savedProfiles(): Profile[] {
  const saved = localStorage.getItem(PROFILES_KEY);
  if (saved) return JSON.parse(saved);
  const project = localStorage.getItem(PROJECT_KEY);
  return project ? [{ ...JSON.parse(project), id: crypto.randomUUID(), name: 'gem', room: 'gem' }] : [];
}

// The selected profile's gem server, reached through its room. It is offline while the room has
// no participant with its identity. Every connected participant costs the project's connection
// minutes, and the server stays in the room only while a frontend does, so a hidden page leaves
// and only the selected profile is connected.
export function useServer() {
  const [profiles, setProfiles] = useState(savedProfiles);
  const [selectedId, setSelectedId] = useState(() => localStorage.getItem(SELECTED_KEY));
  const profile = profiles.find((p) => p.id === selectedId) ?? profiles[0];
  const [room] = useState(() => new Room());
  const [status, setStatus] = useState<ServerStatus>('connecting');
  const [visible, setVisible] = useState(() => document.visibilityState === 'visible');
  // The server's view as this frontend's replica has it, since connecting.
  const [view, setView] = useState<View>();

  useEffect(() => {
    const update = () => setVisible(document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', update);
    return () => document.removeEventListener('visibilitychange', update);
  }, []);

  useEffect(() => {
    localStorage.setItem(PROFILES_KEY, JSON.stringify(profiles));
    localStorage.removeItem(PROJECT_KEY);
  }, [profiles]);

  // The last reported login and search change the profile, but not where it connects.
  const connection = useMemo<Profile | undefined>(
    () => profile,
    [profile?.id, profile?.url, profile?.apiKey, profile?.apiSecret, profile?.room],
  );

  useEffect(() => {
    if (!connection) {
      setStatus('no-profile');
      return;
    }
    if (!visible) return;
    const update = () =>
      setStatus(
        room.state !== ConnectionState.Connected
          ? 'connecting'
          : room.remoteParticipants.has(SERVER)
            ? 'online'
            : 'offline',
      );
    // Another connection, to this profile or another, starts from an empty replica.
    const replica = new Replica();
    let closed = false;
    // While a sync is in flight, a change that doesn't follow on doesn't ask again: LiveKit
    // delivers the server's data in order, so the bytes it asked for arrive before its answer.
    let syncing = false;
    const sync = () => {
      if (closed || syncing) return;
      syncing = true;
      room.localParticipant
        .performRpc({
          destinationIdentity: SERVER,
          method: SYNC_METHOD,
          payload: JSON.stringify(replica.state()),
          responseTimeout: SYNC_TIMEOUT_MS,
        })
        .catch(() => {})
        .finally(() => {
          syncing = false;
        });
    };
    // Changes that arrived ahead of the replica, like those committed while a sync was read and
    // sent, applied once it has caught up to them.
    let ahead: { sent: ViewBytes; bytes: Uint8Array }[] = [];
    // Streams are applied in the order they start, one at a time.
    let applying = Promise.resolve();
    room.registerByteStreamHandler(VIEW_TOPIC, (reader) => {
      applying = applying
        .then(async () => {
          const bytes = concat(await reader.readAll());
          const sent = viewBytes(reader.info.attributes);
          if (closed || !sent) return;
          const applied = replica.apply(sent, bytes);
          if (applied === 'gap') {
            ahead = [...ahead, { sent, bytes }].slice(-AHEAD);
            sync();
            return;
          }
          let changed = applied === 'applied';
          for (let caughtUp = changed; caughtUp; ) {
            caughtUp = false;
            ahead = ahead.filter((change) => {
              const result = replica.apply(change.sent, change.bytes);
              if (result === 'applied') changed = caughtUp = true;
              return result === 'gap';
            });
          }
          if (changed) setView(replica.view());
        })
        .catch(() => {});
    });
    // The server sends a participant data only once it is active.
    const syncWhenActive = (server: Participant) =>
      server.waitUntilActive().then(sync);
    const joined = (participant: Participant) => {
      update();
      if (participant.identity === SERVER) void syncWhenActive(participant);
    };
    room
      .on(RoomEvent.ConnectionStateChanged, update)
      .on(RoomEvent.ParticipantConnected, joined)
      .on(RoomEvent.ParticipantDisconnected, update);
    mintToken(connection)
      .then((jwt) => room.connect(connection.url, jwt))
      .then(() => {
        const server = room.remoteParticipants.get(SERVER);
        if (server) void syncWhenActive(server);
      })
      // A project that refuses the token can still be edited or switched away from.
      .catch(() => setStatus('unreachable'));
    return () => {
      closed = true;
      room.removeAllListeners();
      room.unregisterByteStreamHandler(VIEW_TOPIC);
      room.disconnect();
      setStatus('connecting');
      setView(undefined);
    };
  }, [room, connection, visible]);


  const selectProfile = useCallback((id: string) => {
    localStorage.setItem(SELECTED_KEY, id);
    setSelectedId(id);
  }, []);

  // Adds the profile, or replaces the one with its id, and selects it.
  const saveProfile = useCallback(
    (saved: Profile) => {
      setProfiles((all) =>
        all.some((p) => p.id === saved.id) ? all.map((p) => (p.id === saved.id ? saved : p)) : [...all, saved],
      );
      selectProfile(saved.id);
    },
    [selectProfile],
  );

  const removeProfile = useCallback((id: string) => {
    setProfiles((all) => all.filter((p) => p.id !== id));
  }, []);

  // Keeps what the selected profile's server reported, to label the profile while it is not
  // connected.
  const reported = useCallback(
    (login: string, search: string) =>
      setProfiles((all) =>
        all.map((p) => (p.id === profile?.id && (p.login !== login || p.search !== search) ? { ...p, login, search } : p)),
      ),
    [profile?.id],
  );

  const post = useCallback(
    (path: string, body: JsonValue): Promise<JsonValue> =>
      room.localParticipant
        .performRpc({
          destinationIdentity: SERVER,
          method: 'http',
          payload: JSON.stringify({ method: 'POST', path, body }),
          responseTimeout: RESPONSE_TIMEOUT_MS,
        })
        .then(JSON.parse),
    [room],
  );

  return { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post, view };
}
