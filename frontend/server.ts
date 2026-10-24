import type { JsonValue } from '@bufbuild/protobuf';
import { ConnectionState, Room, RoomEvent } from 'livekit-client';
import { SignJWT } from 'jose';
import { useCallback, useEffect, useMemo, useState } from 'react';

const PROFILES_KEY = 'gem.profiles';
const SELECTED_KEY = 'gem.profile';
// Where the one project was saved before there were profiles.
const PROJECT_KEY = 'gem.project';
// The server joins its profile's room, this one unless its GEM_ROOM names another, as SERVER.
export const DEFAULT_ROOM = 'gem';
const SERVER = 'server';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;

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
  return project ? [{ ...JSON.parse(project), id: crypto.randomUUID(), name: 'gem', room: DEFAULT_ROOM }] : [];
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
    room
      .on(RoomEvent.ConnectionStateChanged, update)
      .on(RoomEvent.ParticipantConnected, update)
      .on(RoomEvent.ParticipantDisconnected, update);
    mintToken(connection)
      .then((jwt) => room.connect(connection.url, jwt))
      // A project that refuses the token can still be edited or switched away from.
      .catch(() => setStatus('unreachable'));
    return () => {
      room.removeAllListeners();
      room.disconnect();
      setStatus('connecting');
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

  return { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post };
}
