import { sha1 } from '@noble/hashes/legacy.js';
import { CoreMemory, Database, Hasher, ReadArrayList, ReadHashMap } from 'xitdb';

import { type Agent, AgentSchema, type PullRequestCluster, PullRequestClusterSchema } from './gen/gem_pb';
import { json, Views } from './lazy';

// A replica of the server's view database (server/src/view.rs), the clusters as gem shows them.
// The server's file only grows, so the replica is sent the whole of it once and then only the
// bytes each change appends. Nothing here knows what carries them.

// The view as of the replica's latest moment: the moment itself, and lazy views of what the UI
// shows from it, read as they're used.
export type View = {
  moment: ReadHashMap;
  // In the order they show, most recently updated first.
  clusters: PullRequestCluster[];
  login: string;
  search: string;
  // The agents acting on no open pull request.
  agents: Agent[];
};

// Which committed bytes of which file the server sent: `start` to `end` of the file with
// `fileId`, a u64 as a decimal string. They replace the replica when `start` is 0.
export type ViewBytes = { fileId: string; start: number; end: number };

// What the replica has, as the server reads it: null for nothing yet, and otherwise its committed
// size and its last commit's checksum, a u64 as a decimal string.
export type ReplicaState = { committedSize: number; checksum: string } | null;

// Applied: the replica has the bytes now. Had: it had them already. Gap: they don't follow on
// from it, so it must ask the server for the bytes it lacks.
export type Applied = 'applied' | 'had' | 'gap';

// The server's view database hashes keys with SHA-1. xitdb's own `Hasher` hashes with node's
// crypto, which Metro resolves to an empty module on the web, so its `digest` is replaced with a
// synchronous one in plain JavaScript. Everything in xitdb that hashes calls `db.hasher.digest`.
export class BrowserSha1 extends Hasher {
  constructor() {
    super('SHA-1');
  }

  override digest(data: Uint8Array): Uint8Array {
    return sha1(data);
  }
}

export class Replica {
  private db?: Database;
  // The messages read from the replica's file.
  private views = new Views();

  get fileId(): string | undefined {
    return this.db?.header.fileId.toString();
  }

  get committedSize(): number {
    return this.db?.committedSize() ?? 0;
  }

  state(): ReplicaState {
    if (!this.db) return null;
    const { committedSize, checksum } = this.db.replicaState();
    return { committedSize, checksum: checksum.toString() };
  }

  // Applies `bytes` when they follow on from the replica, or replace it. Of bytes that overlap
  // what it has, only the rest is written: committed bytes never change.
  apply(sent: ViewBytes, bytes: Uint8Array): Applied {
    if (bytes.length !== sent.end - sent.start) return 'gap';
    const sameFile = this.db !== undefined && sent.fileId === this.fileId;
    const size = this.committedSize;
    if (sameFile && sent.end <= size) return 'had';
    if (sent.start === 0 && !sameFile) return this.replace(bytes);
    if (!sameFile || sent.start > size) return 'gap';
    try {
      const writer = this.db!.openReplicated(size, sent.end - size);
      writer.write(bytes.subarray(size - sent.start));
      writer.close();
      return 'applied';
    } catch {
      // The replica stays at its last commit; the server sends it the bytes again.
      return 'gap';
    }
  }

  private replace(bytes: Uint8Array): Applied {
    const db = new Database(new CoreMemory(), new BrowserSha1());
    try {
      const writer = db.openReplicated(0, bytes.length);
      writer.write(bytes);
      writer.close();
    } catch {
      return 'gap';
    }
    this.db = db;
    this.views.clear();
    return 'applied';
  }

  // The latest moment, a map of what the server last wrote, or undefined before the first sync.
  latest(): ReadHashMap | undefined {
    if (!this.db) return undefined;
    const cursor = new ReadArrayList(this.db.rootCursor()).getCursor(-1);
    return cursor ? new ReadHashMap(cursor) : undefined;
  }

  // The view as of the latest moment. Only the order of the clusters is read now; each cluster,
  // and each agent, is read as it's used, and one a change didn't touch is the object it was.
  view(): View | undefined {
    const moment = this.latest();
    if (!moment) return undefined;
    const string = (key: string) => {
      const cursor = moment.getCursor(key);
      return cursor ? String(json(cursor)) : '';
    };
    const order = moment.getCursor('order');
    const keys = order ? [...new ReadArrayList(order)].map((key) => String(json(key))) : [];
    const stored = moment.getCursor('clusters');
    const byKey = stored ? new ReadHashMap(stored) : undefined;
    const clusters = keys.flatMap((key) => {
      const cluster = byKey?.getCursor(key);
      return cluster ? [this.views.message(PullRequestClusterSchema, cluster)] : [];
    });
    const agents = moment.getCursor('agents');
    return {
      moment,
      clusters,
      login: string('login'),
      search: string('search'),
      agents: agents ? this.views.messages(AgentSchema, agents) : [],
    };
  }
}

// The attributes the server sends bytes with, or undefined when they are not that.
export function viewBytes(attributes: Record<string, string> | undefined): ViewBytes | undefined {
  const { fileId, start, end } = attributes ?? {};
  if (!fileId || !start || !end) return undefined;
  return { fileId, start: Number(start), end: Number(end) };
}

// Joins a byte stream's chunks.
export function concat(chunks: Uint8Array[]): Uint8Array {
  const all = new Uint8Array(chunks.reduce((length, chunk) => length + chunk.length, 0));
  let at = 0;
  for (const chunk of chunks) {
    all.set(chunk, at);
    at += chunk.length;
  }
  return all;
}
