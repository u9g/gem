import type { JsonValue } from '@bufbuild/protobuf';
import { ConnectionState, Room, RoomEvent } from 'livekit-client';
import { SignJWT } from 'jose';
import { useCallback, useEffect, useState } from 'react';

const PROJECT_KEY = 'gem.project';
// The server joins this room in the same project, as SERVER.
const ROOM = 'gem';
const SERVER = 'server';
// Approving pushes a commit and posts to GitHub before it answers.
const RESPONSE_TIMEOUT_MS = 5 * 60_000;

// The user's own LiveKit project, the one their gem server is configured with. It never leaves
// this browser: tokens are minted here.
export type Project = { url: string; apiKey: string; apiSecret: string };

export type ServerStatus = 'no-project' | 'connecting' | 'offline' | 'online';

// A LiveKit access token that can only exchange data in ROOM.
function mintToken(project: Project): Promise<string> {
  return new SignJWT({
    video: { room: ROOM, roomJoin: true, canPublish: false, canSubscribe: false, canPublishData: true },
  })
    .setProtectedHeader({ alg: 'HS256' })
    .setIssuer(project.apiKey)
    .setSubject(crypto.randomUUID())
    .setNotBefore('0s')
    .setExpirationTime('6h')
    .sign(new TextEncoder().encode(project.apiSecret));
}

function savedProject(): Project | undefined {
  const saved = localStorage.getItem(PROJECT_KEY);
  return saved ? JSON.parse(saved) : undefined;
}

// The gem server, reached through the project's room. It is offline while the room has no
// participant with its identity.
export function useServer() {
  const [project, setProject] = useState(savedProject);
  const [room] = useState(() => new Room());
  const [status, setStatus] = useState<ServerStatus>('connecting');

  useEffect(() => {
    if (!project) {
      setStatus('no-project');
      return;
    }
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
    mintToken(project)
      .then((jwt) => room.connect(project.url, jwt))
      .catch(() => setStatus('no-project'));
    return () => {
      room.removeAllListeners();
      room.disconnect();
    };
  }, [room, project]);

  const addProject = useCallback((added: Project | undefined) => {
    if (added) localStorage.setItem(PROJECT_KEY, JSON.stringify(added));
    else localStorage.removeItem(PROJECT_KEY);
    setProject(added);
  }, []);

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

  return { status, project, addProject, post };
}
