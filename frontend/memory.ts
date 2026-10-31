import { CoreMemory, Database, ReadArrayList, WriteArrayList } from 'xitdb';

import { BrowserSha1, type Db, type Infer, type Shape, Views, type Write } from './db';
import { type Agent, type Cluster, Moment } from './shapes';

// A view database in memory, written from values of the shapes in shapes.ts the way the server
// writes its own, for stories and tests to render and read as the app reads its replica.

// What the UI shows, written as a moment: the clusters keyed and ordered as the server does.
export type Shown = {
  clusters?: readonly Write<typeof Cluster>[];
  agents?: readonly Write<typeof Agent>[];
  login?: string;
  search?: string;
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

  moment = (): Moment | undefined => {
    const moment = this.latest();
    return moment && Moment.read(moment, this.views);
  };

  // Writes a new moment showing `shown`.
  show(shown: Shown) {
    const clusters = shown.clusters ?? [];
    const key = (cluster: Write<typeof Cluster>) => cluster.pullRequests?.[0]?.url ?? '';
    this.append(Moment, {
      clusters: Object.fromEntries(clusters.map((cluster) => [key(cluster), cluster])),
      order: clusters.map(key),
      login: shown.login ?? '',
      search: shown.search ?? '',
      agents: shown.agents ?? [],
    });
  }

  // Stores `value`, of shape `shape`, in a moment of its own, which holds nothing else, and
  // returns it, for a component shown alone. It stays readable after later moments, as committed
  // data never changes.
  put<S extends Shape>(shape: S, value: Write<S>): Infer<S> {
    this.append(shape, value);
    return shape.read(this.latest(), this.views) as Infer<S>;
  }

  private latest() {
    return new ReadArrayList(this.db.rootCursor()).getCursor(-1) ?? undefined;
  }

  private append<S extends Shape>(shape: S, value: Write<S>) {
    this.db.runContext((root) => shape.write(new WriteArrayList(root).appendCursor(), value as never));
    this.changes++;
    this.listeners.forEach((listener) => listener());
  }
}
