import type { JsonValue } from '@bufbuild/protobuf';
import { ConnectionState, type Participant, Room, RoomEvent } from 'livekit-client';
import { SignJWT } from 'jose';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ReadHashMap } from 'xitdb';

import { concat, Replica, type ViewBytes, viewBytes } from './view';

const PROFILES_KEY = 'gem.profiles';
const SELECTED_KEY = 'gem.profile';
// Where the one project was saved before there were profiles, when every server joined the room gem.
const PROJECT_KEY = 'gem.project';
// The server joins its profile's room, its GEM_ROOM, as SERVER.
const SERVER = 'server';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;
// The frontend asks for the bytes its replica of the server's view lacks with an RPC call to this
// method. The server sends them, and every later change to the view, as a byte stream on
// VIEW_TOPIC, and answers which bytes it sent.
const SYNC_METHOD = 'view-sync';
const VIEW_TOPIC = 'view';
// How long a sync waits for the server's answer, and then for the bytes it sent.
const SYNC_TIMEOUT_MS = 60_000;

// The user's own LiveKit project, the one their gem server is configured with. It never leaves
// this browser: tokens are minted here.
export type Project = { url: string; apiKey: string; apiSecret: string };

// One gem server: the project and room it serves in, and a name to pick it by. Each server lists
// one GitHub user's PRs narrowed by its GEM_SEARCH; login and search are what it last reported, to
// label it while another profile is connected.
export type Profile = Project & { id: string; name: string; room: string; login?: string; search?: string };

// The server's view as this frontend's replica last had it: its latest moment, and the
// ListOpenPullRequestsResponse JSON rebuilt from it.
export type View = { moment: ReadHashMap; response: JsonValue };

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
  // The view, since connecting.
  const [view, setView] = useState<View>();
  // Asks the server for the bytes the replica lacks, while connected.
  const syncView = useRef<() => Promise<void>>(() => Promise.resolve());

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
    // Syncs waiting for the replica to have the bytes the server answered it sent.
    let waiting: { sent: ViewBytes; done: () => void }[] = [];
    const has = (sent: ViewBytes) => replica.fileId === sent.fileId && replica.committedSize >= sent.end;
    const settle = () => {
      waiting.filter((wait) => has(wait.sent)).forEach((wait) => wait.done());
      waiting = waiting.filter((wait) => !has(wait.sent));
    };
    let syncing: Promise<void> | undefined;
    // Whether to sync again once the sync in flight is done, for bytes that arrived during it and
    // didn't follow on from the replica.
    let again = false;
    const sync = (): Promise<void> => {
      if (closed) return Promise.resolve();
      // A sync asked for after an action has to see it, so it waits for one that starts later.
      if (syncing) {
        again = true;
        return syncing.then(() => syncing);
      }
      const asked = room.localParticipant
        .performRpc({
          destinationIdentity: SERVER,
          method: SYNC_METHOD,
          payload: JSON.stringify(replica.state()),
          responseTimeout: SYNC_TIMEOUT_MS,
        })
        .then((reply) => {
          const sent = viewBytes(JSON.parse(reply));
          if (!sent || has(sent)) return;
          return new Promise<void>((done) => {
            waiting.push({ sent, done });
            // Bytes lost on the way are asked for again when the next change doesn't follow on.
            setTimeout(done, SYNC_TIMEOUT_MS);
          });
        })
        .catch(() => {})
        .finally(() => {
          syncing = undefined;
          if (again) {
            again = false;
            void sync();
          }
        });
      syncing = asked;
      return asked;
    };
    syncView.current = sync;
    // Streams are applied in the order they start, one at a time.
    let applying = Promise.resolve();
    room.registerByteStreamHandler(VIEW_TOPIC, (reader) => {
      applying = applying
        .then(async () => {
          const bytes = concat(await reader.readAll());
          const sent = viewBytes(reader.info.attributes);
          if (closed || !sent) return;
          const applied = replica.apply(sent, bytes);
          if (applied === 'applied') {
            const moment = replica.latest();
            const response = replica.response();
            if (moment && response) setView({ moment, response });
          } else if (applied === 'gap') {
            if (syncing) again = true;
            else void sync();
          }
          settle();
        })
        .catch(() => {});
    });
    // The server sends a participant data only once it is active.
    const syncWhenActive = (server: Participant) =>
      server.waitUntilActive().then(() => void sync());
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
      waiting.forEach((wait) => wait.done());
      syncView.current = () => Promise.resolve();
      room.removeAllListeners();
      room.unregisterByteStreamHandler(VIEW_TOPIC);
      room.disconnect();
      setStatus('connecting');
      setView(undefined);
    };
  }, [room, connection, visible]);

  // Resolves once the replica has the view as the server has it now, or the server could not be
  // asked.
  const sync = useCallback(() => syncView.current(), []);

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

  return { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post, sync, view };
}
