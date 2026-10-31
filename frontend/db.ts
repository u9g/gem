import { sha1 } from '@noble/hashes/legacy.js';
import { createContext, useContext, useMemo, useRef, useSyncExternalStore } from 'react';
import {
  Bytes,
  Float,
  Hasher,
  Int,
  ReadArrayList,
  type ReadCursor,
  ReadHashMap,
  Uint,
  WriteArrayList,
  type WriteCursor,
  WriteHashMap,
} from 'xitdb';

import type { Moment } from './shapes';

// Components read the server's view straight from this frontend's replica of its database, through
// `useDb`, which re-renders them when what they read changes.
//
// The server stores each message as pbjson's serde gives it: a hash map from each field's
// camelCase name to its value, with fields at their default left out; a list for a repeated
// field; and bytes for strings, for enums (by name), for 64-bit integers (as decimal strings) and
// for Timestamps (as RFC 3339), numbers for 32-bit integers, and the text `true` or `false`,
// tagged `bl`, for bools. That form is ambiguous on its own (a decimal string, a time and a
// string all look alike, and a missing key could be any type's default), so each message is
// described once, at runtime, by a `shape` of field kinds (shapes.ts), from which come both its
// TypeScript type and how each field is read and written.
//
// A message is read as a view: a read-only object of its type whose properties are getters, each
// reading its field from the replica the first time it's used and keeping it. Nothing is read up
// front, so a component reads only the fields it renders.
//
// What changed: committed data never changes in place, so a stored value whose slot (its tag and
// its position in the file) is the same is the same value. Views and lists are kept by slot, so
// what a change didn't touch is the identical object after it, and `same` compares what a read
// returns: views, lists and everything else by identity, dates by time, and arrays and plain
// objects by their items.

// How a field is read from the replica, as a `T`, and written, from an `I`. `cursor` is undefined
// for a field left out, which reads as its default.
export type Kind<T, I = T> = {
  readonly read: (cursor: ReadCursor | undefined, views: Views) => T;
  readonly write: (cursor: WriteCursor, value: I) => void;
};

type Fields = { readonly [field: string]: Kind<unknown, never> };
type Value<K> = K extends Kind<infer T, never> ? T : never;
type Written<K> = K extends Kind<unknown, infer I> ? I : never;

// A message: its fields' kinds by their camelCase names.
export type Shape<F extends Fields = Fields> = Kind<Message<F>, Partial<Input<F>>> & { readonly fields: F };
type Message<F> = { readonly [K in keyof F]: Value<F[K]> };
type Input<F> = { readonly [K in keyof F]?: Written<F[K]> };

// The type of a message of shape `S`, as it's read.
export type Infer<S> = S extends Shape<infer F> ? Message<F> : never;
// A message of shape `S` to write, for fixtures: any field can be left out.
export type Write<S> = S extends Shape<infer F> ? Input<F> : never;

// A scalar: `zero` when left out.
function scalar<T, I>(zero: T, from: (stored: string | number | boolean) => T, to: (value: I) => Bytes | Uint | Int | Float): Kind<T, I> {
  return {
    read: (cursor) => (cursor ? from(stored(cursor)) : zero),
    write: (cursor, value) => cursor.write(to(value)),
  };
}

export const str = scalar('', String, (value: string) => new Bytes(value));
// A 32-bit integer or a double.
export const num = scalar(0, Number, (value: number) => {
  if (!Number.isInteger(value)) return new Float(value);
  return value < 0 ? new Int(value) : new Uint(value);
});
export const bool = scalar(false, isTrue, (value: boolean) => new Bytes(value ? 'true' : 'false', 'bl'));
// A 64-bit integer, stored as a decimal string.
export const int64 = scalar(0n, (value) => BigInt(value as string | number), (value: bigint) => new Bytes(value.toString()));
// A Timestamp, stored as RFC 3339, or undefined when it isn't set.
export const time = scalar<Date | undefined, Date | string>(
  undefined,
  (value) => new Date(String(value)),
  (value) => new Bytes(typeof value === 'string' ? value : value.toISOString()),
);
// An enum, by its values' names, and `zero`, its first value, when left out.
export const enumOf = <E extends string>(zero: E) => scalar<E, E>(zero, (value) => String(value) as E, (value) => new Bytes(value));

// A message of fields `fields`, read as a view. As a list's items it's always there; as a field,
// wrap it in `optional`, as a proto3 message field can be unset.
export function shape<F extends Fields>(fields: F): Shape<F> {
  const self: Shape<F> = {
    fields,
    read: (cursor, views) => {
      if (!cursor) throw new Error('a message that is not stored');
      return views.view(self, cursor) as Message<F>;
    },
    write: (cursor, value) => {
      const map = new WriteHashMap(cursor);
      for (const [name, field] of Object.entries(value)) {
        if (field !== undefined && field !== null) (fields[name] as Kind<unknown, unknown>).write(map.putCursor(name), field);
      }
    },
  };
  return self;
}

// A message field, undefined when it isn't set.
export function optional<S extends Shape>(message: S): Kind<Infer<S> | undefined, Write<S>> {
  return {
    read: (cursor, views) => (cursor ? (message.read(cursor, views) as Infer<S>) : undefined),
    write: (cursor, value) => message.write(cursor, value),
  };
}

const none: readonly never[] = Object.freeze([]);

// A repeated field, of messages or scalars, empty when left out.
export function list<T, I>(item: Kind<T, I>): Kind<readonly T[], readonly I[]> {
  return {
    read: (cursor, views) => (cursor ? views.list(cursor, item) : none),
    write: (cursor, values) => {
      const list = new WriteArrayList(cursor);
      for (const value of values) item.write(list.appendCursor(), value);
    },
  };
}

// Values keyed by data rather than by field names, like the clusters by key.
export class Keyed<T> {
  private kept = new Map<string, T | undefined>();

  constructor(
    private readonly map: ReadHashMap | undefined,
    private readonly item: Kind<T, never>,
    private readonly views: Views,
  ) {}

  has(key: string): boolean {
    return this.get(key) !== undefined;
  }

  get(key: string): T | undefined {
    if (!this.map) return undefined;
    if (!this.kept.has(key)) {
      const cursor = present(this.map.getCursor(key));
      this.kept.set(key, cursor && this.item.read(cursor, this.views));
    }
    return this.kept.get(key);
  }

  keys(): string[] {
    if (!this.map) return [];
    return [...this.map].map((pair) => decoder.decode(pair.readKeyValuePair().keyCursor.readBytes()));
  }

  entries(): [string, T][] {
    return this.keys().map((key) => [key, this.get(key)!]);
  }
}

export function keyed<T, I>(item: Kind<T, I>): Kind<Keyed<T>, Readonly<Record<string, I>>> {
  return {
    read: (cursor, views) => (cursor ? views.keyed(cursor, item) : new Keyed(undefined, item, views)),
    write: (cursor, values) => {
      const map = new WriteHashMap(cursor);
      for (const [key, value] of Object.entries(values)) item.write(map.putCursor(key), value);
    },
  };
}

// The class of every view, so `same` compares them by identity rather than field by field.
export class View {}

// The views and lists read from one file, by slot. A replica replaced by another file starts
// another `Views`, so a view of the old file is never mistaken for one of the new.
export class Views {
  private known = new Map<string, unknown>();
  private kinds = new Map<unknown, number>();

  // The view of the message of `shape` at `cursor`.
  view(shape: Shape, cursor: ReadCursor): View {
    return this.kept(shape, cursor, () => {
      const map = new ReadHashMap(cursor);
      const view = new View();
      for (const [name, kind] of Object.entries(shape.fields)) {
        Object.defineProperty(view, name, {
          enumerable: true,
          configurable: true,
          get: () => {
            const value = kind.read(present(map.getCursor(name)), this);
            Object.defineProperty(view, name, { value, enumerable: true, writable: false, configurable: false });
            return value;
          },
        });
      }
      return Object.preventExtensions(view);
    });
  }

  list<T>(cursor: ReadCursor, item: Kind<T, never>): readonly T[] {
    return this.kept(item, cursor, () => Object.freeze([...new ReadArrayList(cursor)].map((value) => item.read(value, this))));
  }

  keyed<T>(cursor: ReadCursor, item: Kind<T, never>): Keyed<T> {
    return this.kept(this, cursor, () => new Keyed(new ReadHashMap(cursor), item, this));
  }

  // What `make` made of the value at `cursor` as `kind`, kept by the value's slot.
  private kept<T>(kind: unknown, cursor: ReadCursor, make: () => T): T {
    if (!this.kinds.has(kind)) this.kinds.set(kind, this.kinds.size);
    const slot = cursor.slot();
    const at = `${this.kinds.get(kind)}:${slot.tag}:${slot.value}`;
    if (!this.known.has(at)) this.known.set(at, make());
    return this.known.get(at) as T;
  }
}

// Whether `a` and `b`, two results of a read, are the same: see the top of this file.
export function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (a instanceof Date && b instanceof Date) return a.getTime() === b.getTime();
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((item, i) => same(item, b[i]));
  if (plain(a) && plain(b)) {
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length && keys.every((key) => key in b && same(a[key], b[key]));
  }
  return false;
}

function plain(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && Object.getPrototypeOf(value) === Object.prototype;
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
  const keys = useDb((moment) => moment?.pending.keys().sort() ?? []);
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

// The value at `cursor`, unless it's missing or empty, as a field left out is.
function present(cursor: ReadCursor | null): ReadCursor | undefined {
  return cursor && cursor.slot().tag !== NONE ? cursor : undefined;
}

// A stored scalar: bytes as a string, or a bool when tagged `bl`, and integers as numbers, or
// decimal strings past 2^53.
function stored(cursor: ReadCursor): string | number | boolean {
  const slot = cursor.slot();
  switch (slot.tag) {
    case BYTES:
    case SHORT_BYTES: {
      const bytes = cursor.readBytesObject();
      const text = decoder.decode(bytes.value);
      const tag = bytes.formatTag;
      return tag?.length === 2 && tag[0] === BOOL[0] && tag[1] === BOOL[1] ? text === 'true' : text;
    }
    case UINT:
      return integer(BigInt.asUintN(64, slot.value));
    case INT:
      return integer(BigInt.asIntN(64, slot.value));
    case FLOAT:
      return cursor.readFloat();
    default:
      throw new Error(`a value tagged ${slot.tag} where a scalar was expected`);
  }
}

// A bool as stored, or as xitdb's own bool codec writes it, a 0 or 1.
function isTrue(value: string | number | boolean): boolean {
  return value === true || value === 1 || value === 'true';
}

function integer(value: bigint): number | string {
  const safe = value <= BigInt(Number.MAX_SAFE_INTEGER) && value >= BigInt(Number.MIN_SAFE_INTEGER);
  return safe ? Number(value) : value.toString();
}

// xitdb's tags, as numbers: its `Tag` is a const enum, which a bundler that compiles a file at a
// time can't inline.
const NONE = 0;
const BYTES = 6;
const SHORT_BYTES = 7;
const UINT = 8;
const INT = 9;
const FLOAT = 10;
// The format tag of a bool, `bl`.
const BOOL = [0x62, 0x6c];
