import { CoreMemory, Database, ReadArrayList } from 'xitdb';

import { BrowserSha1, type Db, Views } from './db';
import { Moment } from './shapes';

// A replica of the server's view database (server/src/view.rs), the clusters as gem shows them,
// which components read through `useDb`. The server's file only grows, so the replica is sent the
// whole of it once and then only the bytes each change appends. Nothing here knows what carries
// them.

// Which committed bytes of which file the server sent: `start` to `end` of the file with
// `fileId`, a u64 as a decimal string. They replace the replica when `start` is 0.
export type ViewBytes = { fileId: string; start: number; end: number };

// What the replica has, as the server reads it: null for nothing yet, and otherwise its committed
// size and its last commit's checksum, a u64 as a decimal string.
export type ReplicaState = { committedSize: number; checksum: string } | null;

// Applied: the replica has the bytes now. Had: it had them already. Gap: they don't follow on
// from it, so it must ask the server for the bytes it lacks.
export type Applied = 'applied' | 'had' | 'gap';

export class Replica implements Db {
  private db?: Database;
  // The messages read from the replica's file.
  private views = new Views();
  private changes = 0;
  private listeners = new Set<() => void>();

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  version = () => this.changes;

  // The latest moment, what the server last wrote, or undefined before the first sync.
  moment = (): Moment | undefined => {
    if (!this.db) return undefined;
    const cursor = new ReadArrayList(this.db.rootCursor()).getCursor(-1);
    return cursor ? Moment.read(cursor, this.views) : undefined;
  };

  private changed() {
    this.changes++;
    this.listeners.forEach((listener) => listener());
  }

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
      this.changed();
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
    this.views = new Views();
    this.changed();
    return 'applied';
  }
}
