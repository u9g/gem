import { sha1 } from '@noble/hashes/legacy.js';
import { createContext, useContext, useRef, useSyncExternalStore } from 'react';
import { Hasher, ReadArrayList, type ReadCursor, ReadHashMap } from 'xitdb';

import type { Moment } from './shapes';

// Components read the server's view straight from this frontend's replica of its database, through
// `useDb`, which re-renders them when what they read changes.
//
// The server stores each message as pbjson's serde gives it: a hash map from each field's
// camelCase name to its value, with fields at their default left out; a list for a repeated
// field; and bytes for strings, for enums (by name), for 64-bit integers (as decimal strings) and
// for Timestamps (as RFC 3339), numbers for 32-bit integers, and the text `true` or `false`,
// tagged `bl`, for bools. A `Node` is one stored message, read a field at a time with the getter
// for its shape, which gives the field's default when it's missing. Shapes are plain types in
// shapes.ts, not protobuf messages.
//
// What changed: committed data never changes in place, so a stored value whose slot (its tag and
// its position in the file) is the same is the same value. Nodes are kept by slot, so an unchanged
// message is the identical `Node` across changes, and `same` compares what a read returns: nodes
// and everything else by identity, dates by time, and arrays and plain objects by their items.

// A Timestamp, as RFC 3339, and a 64-bit integer, as a decimal string, as stored.
declare const rfc3339: unique symbol;
export type Time = string & { readonly [rfc3339]: true };
declare const decimal: unique symbol;
export type Int64 = string & { readonly [decimal]: true };
// A map keyed by data rather than by field names, like the clusters by key.
declare const keyed: unique symbol;
export type Keyed<V> = { readonly [keyed]: V };

type Of<S, K extends keyof S> = NonNullable<S[K]>;
// The fields of `S` holding a `V`.
type Fields<S, V> = { [K in keyof S]-?: Of<S, K> extends V ? K : never }[keyof S] & string;
// The fields of `S` holding a message, or a keyed map.
type Messages<S> = {
  [K in keyof S]-?: Of<S, K> extends string | number | boolean | readonly unknown[] ? never : K;
}[keyof S] &
  string;
// The fields of `S` holding a list of messages.
type Lists<S> = {
  [K in keyof S]-?: Of<S, K> extends readonly (infer U)[] ? (U extends string ? never : K) : never;
}[keyof S] &
  string;
type Item<T> = T extends readonly (infer U)[] ? U : never;

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

// One stored message, of shape `S`.
export class Node<S = unknown> {
  private map?: ReadHashMap;

  constructor(
    private readonly nodes: Nodes,
    readonly cursor: ReadCursor,
    // Its slot, in the file it was read from.
    readonly id: string,
  ) {}

  private get fields(): ReadHashMap {
    return (this.map ??= new ReadHashMap(this.cursor));
  }

  // The value of `field`, unless it's missing or empty.
  private value(field: string): ReadCursor | undefined {
    const cursor = this.fields.getCursor(field);
    return cursor && cursor.slot().tag !== NONE ? cursor : undefined;
  }

  has(field: keyof S & string): boolean {
    return this.value(field) !== undefined;
  }

  str(field: Fields<S, string>): string {
    const value = this.value(field);
    return value ? String(scalar(value)) : '';
  }

  // An enum, by its name, or `zero` when it holds its first value, which is left out.
  enum<K extends Fields<S, string>>(field: K, zero: Of<S, K>): Of<S, K> {
    const value = this.value(field);
    return value ? (String(scalar(value)) as Of<S, K>) : zero;
  }

  num(field: Fields<S, number>): number {
    const value = this.value(field);
    return value ? Number(scalar(value)) : 0;
  }

  bool(field: Fields<S, boolean>): boolean {
    const value = this.value(field);
    return value ? isTrue(scalar(value)) : false;
  }

  int64(field: Fields<S, Int64>): bigint {
    const value = this.value(field);
    return value ? BigInt(scalar(value) as string | number) : 0n;
  }

  time(field: Fields<S, Time>): Date | undefined {
    const value = this.value(field);
    return value ? new Date(String(scalar(value))) : undefined;
  }

  node<K extends Messages<S>>(field: K): Node<Of<S, K>> | undefined {
    const value = this.value(field);
    return value && this.nodes.node(value);
  }

  list<K extends Lists<S>>(field: K): Node<Item<Of<S, K>>>[] {
    const value = this.value(field);
    return value ? this.nodes.list(value, (item) => this.nodes.node(item)) : [];
  }

  strings(field: Fields<S, readonly string[]>): string[] {
    const value = this.value(field);
    return value ? this.nodes.list(value, (item) => String(scalar(item))) : [];
  }

  // The message at `key` of a keyed map.
  at<V = S extends Keyed<infer T> ? T : never>(key: string): Node<V> | undefined {
    const value = this.value(key);
    return value && this.nodes.node<V>(value);
  }
}

// The nodes and lists read from one file, by slot. A replica replaced by another file starts
// another `Nodes`, so a node of the old file is never mistaken for one of the new.
export class Nodes {
  private static files = 0;
  private readonly file = ++Nodes.files;
  private known = new Map<string, unknown>();

  node<S>(cursor: ReadCursor): Node<S> {
    const id = this.id(cursor);
    let node = this.known.get(id) as Node<S> | undefined;
    if (!node) this.known.set(id, (node = new Node<S>(this, cursor, id)));
    return node;
  }

  list<T>(cursor: ReadCursor, item: (cursor: ReadCursor) => T): T[] {
    const id = `[${this.id(cursor)}]`;
    let list = this.known.get(id) as T[] | undefined;
    if (!list) this.known.set(id, (list = [...new ReadArrayList(cursor)].map(item)));
    return list;
  }

  private id(cursor: ReadCursor): string {
    const slot = cursor.slot();
    return `${this.file}:${slot.tag}:${slot.value}`;
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
  moment: () => Node<Moment> | undefined;
}

// Before a connection: nothing to read.
export const emptyDb: Db = { subscribe: () => () => {}, version: () => 0, moment: () => undefined };

export const DbContext = createContext<Db>(emptyDb);

// What `read` returned from `db` as of `version`.
export type Kept<T> = { db: Db; version: number; read: unknown; value: T };

// What `read` returns from `db` now, or what it returned before when that's the same, so that
// React, which re-renders when this changes, re-renders only when what was read changed.
export function select<T>(db: Db, read: (moment: Node<Moment> | undefined) => T, kept?: Kept<T>): Kept<T> {
  const version = db.version();
  if (kept && kept.db === db && kept.version === version && kept.read === read) return kept;
  const value = read(db.moment());
  const before = kept && kept.db === db && same(kept.value, value);
  return { db, version, read, value: before ? kept.value : value };
}

// What `read` returns from the latest moment of the database in context, read again after each
// change, re-rendering the component only when it returns something else. `read` gets undefined
// before the first sync.
export function useDb<T>(read: (moment: Node<Moment> | undefined) => T): T {
  const db = useContext(DbContext);
  const kept = useRef<Kept<T>>(undefined);
  return useSyncExternalStore(db.subscribe, () => (kept.current = select(db, read, kept.current)).value);
}

const decoder = new TextDecoder();

// A stored scalar: bytes as a string, or a bool when tagged `bl`, and integers as numbers, or
// decimal strings past 2^53.
function scalar(cursor: ReadCursor): string | number | boolean {
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
