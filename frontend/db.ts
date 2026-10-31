import { sha1 } from '@noble/hashes/legacy.js';
import { createContext, useContext, useMemo, useRef, useSyncExternalStore } from 'react';
import { Hasher, ReadArrayList, type ReadCursor, ReadHashMap } from 'xitdb';

import type { Moment } from './view-types';

// Components read the server's view straight from this frontend's replica of its database, through
// `useDb`, which re-renders them when what they read changes.
//
// The server writes the view's plain structs with xitdb's `Structured` codec, every field written,
// so the stored data describes itself and is read without a schema: a map is a read-only object
// whose properties are read from the replica as they're used, a list a frozen array, bytes a
// string, or a bool when tagged `bl`, a number a number, and an empty slot undefined. Nothing is
// read up front, so a component reads only what it renders. view-types.ts, generated
// from the server's structs, says what the objects hold.
//
// What changed: committed data never changes in place, so a stored value whose slot (its tag and
// its position in the file) is the same is the same value. Maps and lists are kept by slot, so
// what a change didn't touch is the identical object after it, and `same` compares what a read
// returns: maps, lists and everything else by identity, and arrays and plain objects by items.

// The class of every map read, so `same` compares them by identity rather than field by field.
class Stored {}

// The values read from one file, by slot. A replica replaced by another file starts another
// `Views`, so a value of the old file is never mistaken for one of the new.
export class Views {
  private known = new Map<string, unknown>();

  // The value at `cursor`, or undefined for none.
  read(cursor: ReadCursor | null | undefined): unknown {
    if (!cursor) return undefined;
    const slot = cursor.slot();
    const at = `${slot.tag}:${slot.value}`;
    switch (slot.tag) {
      case NONE:
        return undefined;
      case BYTES:
      case SHORT_BYTES: {
        const { value, formatTag } = cursor.readBytesObject();
        const text = decoder.decode(value);
        return formatTag?.[0] === BOOL[0] && formatTag[1] === BOOL[1] ? text === 'true' : text;
      }
      case UINT:
        return cursor.readUint();
      case INT:
        return cursor.readInt();
      case FLOAT:
        return cursor.readFloat();
      case ARRAY_LIST:
        if (!this.known.has(at)) this.known.set(at, Object.freeze([...new ReadArrayList(cursor)].map((item) => this.read(item))));
        return this.known.get(at);
      case HASH_MAP: {
        if (this.known.has(at)) return this.known.get(at);
        const map = new ReadHashMap(cursor);
        // Each key's value once it's read, and the keys once they're listed.
        const values = new Map<string, unknown>();
        let keys: string[] | undefined;
        const value = (key: string) => {
          if (!values.has(key)) values.set(key, this.read(map.getCursor(key)));
          return values.get(key);
        };
        const read = new Proxy(new Stored(), {
          // A key the map hasn't is the object's own, like `toString`.
          get: (target, key) => (typeof key === 'string' ? (value(key) ?? Reflect.get(target, key)) : Reflect.get(target, key)),
          has: (_, key) => typeof key === 'string' && value(key) !== undefined,
          ownKeys: () => (keys ??= [...map].map((pair) => decoder.decode(pair.readKeyValuePair().keyCursor.readBytes()))),
          getOwnPropertyDescriptor: (_, key) =>
            typeof key === 'string' && value(key) !== undefined
              ? { value: value(key), enumerable: true, configurable: true, writable: false }
              : undefined,
          set: () => false,
          defineProperty: () => false,
          deleteProperty: () => false,
        });
        this.known.set(at, read);
        return read;
      }
      default:
        throw new Error(`a value tagged ${slot.tag}, which the view never holds`);
    }
  }
}

// Whether `a` and `b`, two results of a read, are the same: see the top of this file.
export function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((item, i) => same(item, b[i]));
  // Plain objects, as a read builds, not maps read from the view.
  if (
    typeof a === 'object' &&
    typeof b === 'object' &&
    a !== null &&
    b !== null &&
    Object.getPrototypeOf(a) === Object.prototype &&
    Object.getPrototypeOf(b) === Object.prototype
  ) {
    const [x, y] = [a as Record<string, unknown>, b as Record<string, unknown>];
    const keys = Object.keys(x);
    return keys.length === Object.keys(y).length && keys.every((key) => key in y && same(x[key], y[key]));
  }
  return false;
}

// The database a component reads: the replica, or a fixture in stories and tests. `version`
// changes with each change, after which every subscriber is called.
export interface Db {
  subscribe: (listener: () => void) => () => void;
  version: () => number;
  moment: () => Moment | undefined;
}

// Before a connection: nothing to read.
export const emptyDb: Db = { subscribe: () => () => {}, version: () => 0, moment: () => undefined };

export const DbContext = createContext<Db>(emptyDb);

// What `read` returned from `db` as of `version`.
export type Kept<T> = { db: Db; version: number; read: unknown; value: T };

// What `read` returns from `db` now, or what it returned before when that's the same, so that
// React, which re-renders when this changes, re-renders only when what was read changed.
export function select<T>(db: Db, read: (moment: Moment | undefined) => T, kept?: Kept<T>): Kept<T> {
  const version = db.version();
  if (kept && kept.db === db && kept.version === version && kept.read === read) return kept;
  const value = read(db.moment());
  const before = kept && kept.db === db && same(kept.value, value);
  return { db, version, read, value: before ? kept.value : value };
}

// What `read` returns from the latest moment of the database in context, read again after each
// change, re-rendering the component only when it returns something else. `read` gets undefined
// before the first sync.
export function useDb<T>(read: (moment: Moment | undefined) => T): T {
  const db = useContext(DbContext);
  const kept = useRef<Kept<T>>(undefined);
  return useSyncExternalStore(db.subscribe, () => (kept.current = select(db, read, kept.current)).value);
}

// The keys of the actions in progress, which every frontend greys out: the same set until they
// change.
export function usePending(): ReadonlySet<string> {
  const keys = useDb((moment) => Object.keys(moment?.pending ?? {}).sort());
  return useMemo(() => new Set(keys), [keys]);
}

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

const decoder = new TextDecoder();

// xitdb's tags, as numbers: its `Tag` is a const enum, which a bundler that compiles a file at a
// time can't inline.
const NONE = 0;
const ARRAY_LIST = 2;
const HASH_MAP = 4;
const BYTES = 6;
const SHORT_BYTES = 7;
const UINT = 8;
const INT = 9;
const FLOAT = 10;
// The format tag of a bool, `bl`.
const BOOL = [0x62, 0x6c];
