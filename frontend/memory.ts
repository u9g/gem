import { Bytes, CoreMemory, Database, Float, Int, ReadArrayList, Uint, WriteArrayList, type WriteCursor, WriteHashMap } from 'xitdb';

import { BrowserSha1, type Db, Views } from './db';
import type { Agent, Cluster, DiffHighlights, Moment } from './view-types';

// A view database in memory, written from plain values the way the server's `Structured` codec
// writes its structs, for stories and tests to render and read as the app reads its replica: an
// object is a map, an array a list, a string bytes, a bool `bl` text, an integer an Int or Uint,
// another number a Float, and undefined is left out.

// What the UI shows, written as a moment: the clusters keyed and ordered as the server does.
export type Shown = {
  clusters?: readonly Cluster[];
  agents?: readonly Agent[];
  login?: string;
  search?: string;
  // The keys of the actions in progress.
  pending?: readonly string[];
  // What omnidiff makes of diffs, by `diffKey`.
  diffs?: Readonly<Record<string, DiffHighlights>>;
};

export class MemoryDb implements Db {
  private db = new Database(new CoreMemory(), new BrowserSha1());
  private views = new Views();
  private changes = 0;
  private listeners = new Set<() => void>();

  constructor(shown: Shown = {}) {
    this.show(shown);
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  version = () => this.changes;

  moment = (): Moment | undefined =>
    this.views.read(new ReadArrayList(this.db.rootCursor()).getCursor(-1)) as Moment | undefined;

  // Writes a new moment showing `shown`. Each cluster's key is the URL of its oldest pull request.
  show(shown: Shown) {
    const clusters = shown.clusters ?? [];
    this.append({
      clusters: Object.fromEntries(clusters.map((cluster) => [cluster.pullRequests[0]?.url ?? '', cluster])),
      order: clusters.map((cluster) => cluster.pullRequests[0]?.url ?? ''),
      login: shown.login ?? '',
      search: shown.search ?? '',
      agents: shown.agents ?? [],
      pending: Object.fromEntries((shown.pending ?? []).map((key) => [key, ''])),
      diffs: shown.diffs ?? {},
    });
  }

  // Stores `value` in a moment of its own, which holds nothing else, and returns it as it reads,
  // for a component shown alone. It stays readable after later moments, as committed data never
  // changes.
  put<T>(value: T): T {
    this.append(value);
    return this.views.read(new ReadArrayList(this.db.rootCursor()).getCursor(-1)) as T;
  }

  private append(value: unknown) {
    const write = (cursor: WriteCursor, value: unknown) => {
      if (typeof value === 'string') cursor.write(new Bytes(value));
      else if (typeof value === 'boolean') cursor.write(new Bytes(value ? 'true' : 'false', 'bl'));
      else if (typeof value === 'number') {
        if (!Number.isInteger(value)) cursor.write(new Float(value));
        else cursor.write(value < 0 ? new Int(value) : new Uint(value));
      } else if (Array.isArray(value)) {
        const list = new WriteArrayList(cursor);
        for (const item of value) write(list.appendCursor(), item);
      } else if (typeof value === 'object' && value !== null) {
        const map = new WriteHashMap(cursor);
        for (const [key, field] of Object.entries(value)) if (field !== undefined) write(map.putCursor(key), field);
      }
    };
    this.db.runContext((root) => write(new WriteArrayList(root).appendCursor(), value));
    this.changes++;
    this.listeners.forEach((listener) => listener());
  }
}
