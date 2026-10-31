import type { JsonValue } from '@bufbuild/protobuf';
import { SignJWT } from 'jose';
import { ConnectionState, type RemoteParticipant, Room, RoomEvent, RpcError } from 'livekit-client';
import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';

const LOGIN_KEY = 'underpinning.login';
// The gem server joins its room as this participant and answers every request sent to it as an
// RPC call to this method.
const SERVER = 'server';
const METHOD = 'http';
// The server sends what `/prs` answers on this topic whenever it changes.
const SNAPSHOT_TOPIC = 'prs';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;
const LOG_LIMIT = 200;

// The LiveKit project and room a gem server is configured with: its LIVEKIT_URL, LIVEKIT_API_KEY,
// LIVEKIT_API_SECRET and GEM_ROOM. It never leaves this browser: tokens are minted here.
export type Login = { url: string; apiKey: string; apiSecret: string; room: string };

// Unreachable when the project refuses to connect; offline when the frontend is in the room but
// the backend is not.
export type Status = 'logged-out' | 'connecting' | 'unreachable' | 'offline' | 'online';

// One message in the log. A response or error names the request it answers.
export type Entry = {
  id: number;
  at: Date;
  kind: 'sent' | 'response' | 'error' | 'pushed' | 'event';
  title: string;
  body?: JsonValue;
  replyTo?: number;
  ms?: number;
};

// A LiveKit access token that can only exchange data in the login's room.
function mintToken(login: Login, identity: string): Promise<string> {
  return new SignJWT({
    video: { room: login.room, roomJoin: true, canPublish: false, canSubscribe: false, canPublishData: true },
  })
    .setProtectedHeader({ alg: 'HS256' })
    .setIssuer(login.apiKey)
    .setSubject(identity)
    .setNotBefore('0s')
    .setExpirationTime('6h')
    .sign(new TextEncoder().encode(login.apiSecret));
}

function parse(text: string): JsonValue {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function savedLogin(): Login | undefined {
  const saved = localStorage.getItem(LOGIN_KEY);
  return saved ? JSON.parse(saved) : undefined;
}

// The gem frontend's saved profiles, when it runs on the same origin, to log in with.
export function gemProfiles(): (Login & { name: string })[] {
  try {
    return JSON.parse(localStorage.getItem('gem.profiles') ?? '[]');
  } catch {
    return [];
  }
}

// This page as a frontend of the gem server: it joins the login's room as a client of the backend,
// logs what the backend pushes, and sends requests and logs their responses. Like the gem frontend,
// it leaves the room while the page is hidden, since the backend stays only while a frontend does.
export function useBackend() {
  const [login, setLogin] = useState(savedLogin);
  const [room] = useState(() => new Room());
  const [identity] = useState(() => `underpinning-${crypto.randomUUID().slice(0, 8)}`);
  const [status, setStatus] = useState<Status>('logged-out');
  const [visible, setVisible] = useState(() => document.visibilityState === 'visible');
  const [entries, setEntries] = useState<Entry[]>([]);
  const nextId = useRef(0);

  const log = useCallback((entry: Omit<Entry, 'id' | 'at'>) => {
    const id = nextId.current++;
    setEntries((all) => [{ ...entry, id, at: new Date() }, ...all].slice(0, LOG_LIMIT));
    return id;
  }, []);

  useEffect(() => {
    const update = () => setVisible(document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', update);
    return () => document.removeEventListener('visibilitychange', update);
  }, []);

  useEffect(() => {
    if (!login) {
      setStatus('logged-out');
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
    const joined = (participant: RemoteParticipant) => {
      if (participant.identity === SERVER) log({ kind: 'event', title: 'Backend joined the room' });
      update();
    };
    const left = (participant: RemoteParticipant) => {
      if (participant.identity === SERVER) log({ kind: 'event', title: 'Backend left the room' });
      update();
    };
    room
      .on(RoomEvent.ConnectionStateChanged, (state) => {
        log({ kind: 'event', title: `Frontend ${state} as ${identity} in room ${login.room}` });
        update();
      })
      .on(RoomEvent.ParticipantConnected, joined)
      .on(RoomEvent.ParticipantDisconnected, left);
    room.registerTextStreamHandler(SNAPSHOT_TOPIC, (reader, participant) =>
      reader
        .readAll()
        .then((text) => log({ kind: 'pushed', title: `${SNAPSHOT_TOPIC} from ${participant.identity}`, body: parse(text) })),
    );
    mintToken(login, identity)
      .then((jwt) => room.connect(login.url, jwt))
      .catch((error: Error) => {
        setStatus('unreachable');
        log({ kind: 'error', title: 'Could not connect to LiveKit', body: error.message });
      });
    return () => {
      room.removeAllListeners();
      room.unregisterTextStreamHandler(SNAPSHOT_TOPIC);
      room.disconnect();
      setStatus('connecting');
    };
  }, [room, login, visible, identity, log]);

  const saveLogin = useCallback((saved: Login | undefined) => {
    if (saved) localStorage.setItem(LOGIN_KEY, JSON.stringify(saved));
    else localStorage.removeItem(LOGIN_KEY);
    setLogin(saved);
  }, []);

  // Sends the request the way the gem frontend does, as HTTP carried over an RPC call, and logs
  // it and its response. The server fails a request with its HTTP status as the RPC error's code.
  const send = useCallback(
    async (method: string, path: string, body: JsonValue) => {
      const title = `${method} ${path}`;
      const replyTo = log({ kind: 'sent', title, body });
      const started = performance.now();
      const ms = () => Math.round(performance.now() - started);
      try {
        const reply = await room.localParticipant.performRpc({
          destinationIdentity: SERVER,
          method: METHOD,
          payload: JSON.stringify({ method, path, body }),
          responseTimeout: RESPONSE_TIMEOUT_MS,
        });
        log({ kind: 'response', title, replyTo, ms: ms(), body: parse(reply) });
      } catch (error) {
        const failed = error instanceof RpcError ? `${title} → ${error.code}` : title;
        log({ kind: 'error', title: failed, replyTo, ms: ms(), body: parse((error as Error).message) });
      }
    },
    [room, log],
  );

  const clear = useCallback(() => setEntries([]), []);

  return { login, saveLogin, identity, status, entries, send, clear };
}

export const BackendContext = createContext<ReturnType<typeof useBackend> | null>(null);

export function useBackendContext() {
  return useContext(BackendContext)!;
}
