import type { JsonValue } from '@bufbuild/protobuf';
import { ConnectionState, Room, RoomEvent } from 'livekit-client';
import { SignJWT } from 'jose';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

const PROFILES_KEY = 'gem.profiles';
const SELECTED_KEY = 'gem.profile';
// Where the one project was saved before there were profiles, when every server joined the room gem.
const PROJECT_KEY = 'gem.project';
// The server joins its profile's room, its GEM_ROOM, as SERVER.
const SERVER = 'server';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;
// The server sends what `/prs` answers on this topic whenever it changes.
const SNAPSHOT_TOPIC = 'prs';
// Events kept for the server's log while it cannot be reached, across reloads.
const EVENTS_KEY = 'gem.events';
// The oldest are dropped past this, so a phone left offline for days does not grow without bound.
const MAX_EVENTS = 200;
// Where the frontend's events go in the server's log.
const LOG_PATH = '/log';

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

// Identifies this page load in the server's log.
const SESSION = crypto.randomUUID();

// Something the frontend saw that the server could not, e.g. a request that failed before reaching
// it. Kept until the server can be told.
type FrontendEvent = { at: string; event: string; fields: Record<string, JsonValue> };

function savedEvents(): FrontendEvent[] {
  try {
    return JSON.parse(localStorage.getItem(EVENTS_KEY) ?? '[]');
  } catch {
    return [];
  }
}

// Told of each recorded event, so one recorded while the server is reachable is sent at once.
const recordedListeners = new Set<() => void>();

// Records `event` for the server's log, and in the console.
function record(event: string, fields: Record<string, JsonValue> = {}) {
  console.warn(`gem: ${event}`, fields);
  const events = [...savedEvents(), { at: new Date().toISOString(), event, fields }].slice(-MAX_EVENTS);
  localStorage.setItem(EVENTS_KEY, JSON.stringify(events));
  recordedListeners.forEach((listener) => listener());
}

// LiveKit's RpcError carries a code; anything else is reported by its message alone.
function describe(error: unknown): Record<string, JsonValue> {
  if (error instanceof Error) {
    const code = (error as { code?: unknown }).code;
    return { error: error.message, ...(typeof code === 'number' ? { code } : {}) };
  }
  return { error: String(error) };
}

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
  // The latest snapshot the server sent since connecting, as JSON.
  const [snapshot, setSnapshot] = useState<JsonValue>();
  // Counts recorded events, to send each to the server.
  const [recorded, setRecorded] = useState(0);
  const sending = useRef(false);

  useEffect(() => {
    const listener = () => setRecorded((n) => n + 1);
    recordedListeners.add(listener);
    return () => {
      recordedListeners.delete(listener);
    };
  }, []);

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
    let last: ServerStatus | undefined;
    const update = () => {
      const next =
        room.state !== ConnectionState.Connected
          ? 'connecting'
          : room.remoteParticipants.has(SERVER)
            ? 'online'
            : 'offline';
      // Connecting at first is expected; losing the server once reached is what gets logged.
      if (last && next !== last) record('status changed', { from: last, to: next, room: connection.room });
      last = next;
      setStatus(next);
    };
    room
      .on(RoomEvent.ConnectionStateChanged, update)
      .on(RoomEvent.ParticipantConnected, update)
      .on(RoomEvent.ParticipantDisconnected, update)
      .on(RoomEvent.Disconnected, (reason) => record('room disconnected', { reason: reason ?? null }));
    room.registerTextStreamHandler(SNAPSHOT_TOPIC, (reader) =>
      reader.readAll().then((text) => setSnapshot(JSON.parse(text))),
    );
    mintToken(connection)
      .then((jwt) => room.connect(connection.url, jwt))
      // A project that refuses the token can still be edited or switched away from.
      .catch((error) => {
        record('cannot connect', { url: connection.url, room: connection.room, ...describe(error) });
        setStatus('unreachable');
      });
    return () => {
      room.removeAllListeners();
      room.unregisterTextStreamHandler(SNAPSHOT_TOPIC);
      room.disconnect();
      setStatus('connecting');
      setSnapshot(undefined);
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

  const rpc = useCallback(
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

  // Every failed request is recorded, with what the frontend knew of the connection when it
  // failed, before the caller sees the failure.
  const post = useCallback(
    (path: string, body: JsonValue): Promise<JsonValue> => {
      const started = performance.now();
      return rpc(path, body).catch((error) => {
        record('request failed', {
          path,
          elapsedMs: Math.round(performance.now() - started),
          roomState: room.state,
          serverInRoom: room.remoteParticipants.has(SERVER),
          ...describe(error),
        });
        throw error;
      });
    },
    [rpc, room],
  );

  // Sends the recorded events once the server can be reached, and keeps them if it cannot.
  useEffect(() => {
    if (status !== 'online' || sending.current) return;
    const events = savedEvents();
    if (events.length === 0) return;
    sending.current = true;
    rpc(LOG_PATH, { session: SESSION, events })
      // Events recorded while sending stay for the next send.
      .then(() => localStorage.setItem(EVENTS_KEY, JSON.stringify(savedEvents().slice(events.length))))
      .catch((error) => console.warn('gem: cannot send the recorded events', error))
      .finally(() => {
        sending.current = false;
      });
  }, [status, rpc, recorded]);

  return { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post, snapshot };
}
