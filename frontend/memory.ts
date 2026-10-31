import {
  Bytes,
  CoreMemory,
  Database,
  Float,
  Int,
  ReadArrayList,
  Uint,
  WriteArrayList,
  type WriteCursor,
  WriteHashMap,
} from 'xitdb';

import { BrowserSha1, type Db, type Int64, type Keyed, type Node, Nodes, type Time } from './db';
import type { Agent, Cluster, Moment } from './shapes';

// A view database in memory, written from plain values in the shapes of shapes.ts the way the
// server writes its own, for stories and tests to render and read as the app reads its replica.

// A value of type `T` to write: any field can be left out; a Timestamp is a Date or RFC 3339; a
// 64-bit integer a bigint or a decimal string; an enum its name.
export type Input<T> = T extends Time
  ? Date | string
  : T extends Int64
    ? bigint | string
    : T extends string | number | boolean
      ? T
      : T extends Keyed<infer V>
        ? Record<string, Input<V>>
        : T extends readonly (infer U)[]
          ? readonly Input<U>[]
          : { readonly [K in keyof T]?: Input<T[K]> };

// What the UI shows, written as a moment: the clusters keyed and ordered as the server does.
export type Shown = {
  clusters?: readonly Input<Cluster>[];
  agents?: readonly Input<Agent>[];
  login?: string;
  search?: string;
};

export class MemoryDb implements Db {
  private db = new Database(new CoreMemory(), new BrowserSha1());
  private nodes = new Nodes();
  private changes = 0;
  private listeners = new Set<() => void>();
  // What the latest moment holds, which each write rewrites whole.
  private current: Record<string, unknown> = {};

  constructor(shown: Shown = {}) {
    this.show(shown);
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  version = () => this.changes;

  moment = (): Node<Moment> | undefined => {
    const moment = new ReadArrayList(this.db.rootCursor()).getCursor(-1);
    return moment ? this.nodes.node<Moment>(moment) : undefined;
  };

  // Writes a new moment showing `shown`.
  show(shown: Shown) {
    const clusters = shown.clusters ?? [];
    const key = (cluster: Input<Cluster>) => cluster.pullRequests?.[0]?.url ?? '';
    this.append({
      clusters: Object.fromEntries(clusters.map((cluster) => [key(cluster), cluster])),
      order: clusters.map(key),
      login: shown.login ?? '',
      search: shown.search ?? '',
      agents: shown.agents ?? [],
    });
  }

  // Stores `value` in a moment of its own, which holds nothing else, and returns it, for a
  // component shown alone. The node stays readable after later moments, as committed data never
  // changes.
  put<S>(value: Input<S>): Node<S> {
    this.append({ value }, false);
    return this.moment()!.at<S>('value')!;
  }

  // Writes a moment of what the last held with `changes`, or, unless `keep`, of `changes` alone.
  private append(changes: Record<string, unknown>, keep = true) {
    this.current = keep ? { ...this.current, ...changes } : changes;
    this.db.runContext((root) => write(new WriteArrayList(root).appendCursor(), this.current));
    this.changes++;
    this.listeners.forEach((listener) => listener());
  }
}

// Writes `value` at `cursor` in the shape the server's `Structured` codec writes it.
export function write(cursor: WriteCursor, value: unknown) {
  if (typeof value === 'string') cursor.write(new Bytes(value));
  else if (typeof value === 'boolean') cursor.write(new Bytes(value ? 'true' : 'false', 'bl'));
  else if (typeof value === 'bigint') cursor.write(new Bytes(value.toString()));
  else if (typeof value === 'number') {
    if (!Number.isInteger(value)) cursor.write(new Float(value));
    else cursor.write(value < 0 ? new Int(value) : new Uint(value));
  } else if (value instanceof Date) cursor.write(new Bytes(value.toISOString()));
  else if (Array.isArray(value)) {
    const list = new WriteArrayList(cursor);
    for (const item of value) write(list.appendCursor(), item);
  } else if (typeof value === 'object' && value !== null) {
    const map = new WriteHashMap(cursor);
    for (const [key, field] of Object.entries(value)) {
      if (field !== undefined && field !== null) write(map.putCursor(key), field);
    }
  }
}
