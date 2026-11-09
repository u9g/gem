import type { JsonValue } from '@bufbuild/protobuf';
import { ConnectionState, Room, RoomEvent, RpcError } from 'livekit-client';
import { SignJWT } from 'jose';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { Replica } from './view';

const PROFILES_KEY = 'gem.profiles';
const SELECTED_KEY = 'gem.profile';
// Where the one project was saved before there were profiles, when every server joined the room gem.
const PROJECT_KEY = 'gem.project';
// The server joins its profile's room, its GEM_ROOM, as SERVER.
const SERVER = 'server';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;
// The server sends the bytes of its view a replica lacks, and every change to it, as byte streams
// on VIEW_TOPIC, and the frontend reports what its replica has in data messages on STATE_TOPIC.
const VIEW_TOPIC = 'view';
const STATE_TOPIC = 'view-state';

// The user's own LiveKit project, the one their gem server is configured with. It never leaves
// this browser: tokens are minted here.
export type Project = { url: string; apiKey: string; apiSecret: string };

// One gem server: the project and room it serves in, and a name to pick it by. Each server lists
// one GitHub user's PRs narrowed by its GEM_SEARCH; login and search are what it last reported, to
// label it while another profile is connected.
export type Profile = Project & { id: string; name: string; room: string; login?: string; search?: string };

// Unreachable when the project refuses to connect, e.g. for a wrong URL, API key or secret;
// offline when the room is reached but the server is not in it; syncing until the replica has a
// whole copy of the server's view.
export type ServerStatus = 'no-profile' | 'connecting' | 'unreachable' | 'offline' | 'syncing' | 'online';

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

// Connects with `connect` and keeps `replica` a copy of the server's view, reporting the server's
// status, until the returned function leaves the room. It's apart from `useServer` for its test.
//
// The server drives syncing: it sends the whole view once this frontend is active, every change
// as it commits, and whatever this frontend's reports show it lacks. So the frontend only applies
// what it's sent, and reports what it has after each stream and when it connects.
export function followView(
  room: Room,
  replica: Replica,
  setStatus: (status: ServerStatus) => void,
  connect: () => Promise<void>,
): () => void {
  let closed = false;
  // Whether the replica has had a whole copy yet.
  let synced = replica.moment() !== undefined;
  const update = () =>
    setStatus(
      room.state !== ConnectionState.Connected
        ? 'connecting'
        : !room.remoteParticipants.has(SERVER)
          ? 'offline'
          : synced
            ? 'online'
            : 'syncing',
    );
  // Its file ID, a u64, as a decimal string, with xitdb's serde form of its state, or null while
  // it's empty.
  const report = () => {
    const state = replica.state();
    const payload = new TextEncoder().encode(JSON.stringify(state && { fileId: replica.fileId, ...state }));
    room.localParticipant
      .publishData(payload, { reliable: true, destinationIdentities: [SERVER], topic: STATE_TOPIC })
      // The server sends again what a report it doesn't get would have asked for.
      .catch(() => {});
  };
  // Streams are applied in the order they start, one at a time.
  let applying = Promise.resolve();
  room.registerByteStreamHandler(VIEW_TOPIC, (reader) => {
    applying = applying
      .then(async () => {
        // The stream's chunks, joined.
        const bytes = new Uint8Array(await new Blob((await reader.readAll()) as BlobPart[]).arrayBuffer());
        // Which bytes they are, as the server says in the stream's attributes.
        const { fileId, start, end } = reader.info.attributes ?? {};
        if (closed || !fileId || !start || !end) return;
        // Bytes that don't follow on from the replica are left out; the report says what it has.
        replica.apply({ fileId, start: Number(start), end: Number(end) }, bytes);
        if (!synced && replica.moment()) {
          synced = true;
          update();
        }
        report();
      })
      .catch(() => {});
  });
  room
    .on(RoomEvent.ConnectionStateChanged, update)
    .on(RoomEvent.ParticipantConnected, (participant) => {
      if (participant.identity !== SERVER) return;
      update();
      report();
    })
    .on(RoomEvent.ParticipantDisconnected, update);
  connect()
    .then(() => {
      update();
      if (room.remoteParticipants.has(SERVER)) report();
    })
    // A project that refuses the token can still be edited or switched away from.
    .catch(() => setStatus('unreachable'));
  return () => {
    closed = true;
    room.removeAllListeners();
    room.unregisterByteStreamHandler(VIEW_TOPIC);
    void room.disconnect();
  };
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
  // The replica of the connection's server's view, which components read through `useDb`. It
  // outlives leaving the room while the page is hidden, so the view stays shown; another
  // connection starts from an empty one.
  const replica = useMemo(() => new Replica(), [connection]);

  useEffect(() => {
    if (!connection) {
      setStatus('no-profile');
      return;
    }
    if (!visible) return;
    const stop = followView(room, replica, setStatus, () =>
      mintToken(connection).then((jwt) => room.connect(connection.url, jwt)),
    );
    return () => {
      stop();
      setStatus('connecting');
    };
  }, [room, connection, visible, replica]);


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

  return { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post, db: connection ? replica : undefined };
}

// Why a request `post` sent failed: what the server answered, or LiveKit's reason it got no
// answer.
export function failureMessage(error: unknown): string {
  if (!(error instanceof RpcError)) return error instanceof Error ? error.message : String(error);
  // Approving can outlast the wait, and the server goes on with it.
  if (error.code === RpcError.ErrorCode.RESPONSE_TIMEOUT) return "the server didn't answer in time; it may still finish";
  if (error.code === RpcError.ErrorCode.RECIPIENT_NOT_FOUND) return "the server isn't connected";
  if (error.code === RpcError.ErrorCode.RECIPIENT_DISCONNECTED) return 'the server disconnected before answering';
  // The server answers with the HTTP response's status as the code and its body, which can be
  // empty, as the message.
  return error.message || `the server answered ${error.code}`;
}
