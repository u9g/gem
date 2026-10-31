import type { JsonValue } from '@bufbuild/protobuf';
import { sha1 } from '@noble/hashes/legacy.js';
import { CoreMemory, Database, Hasher, ReadArrayList, type ReadCursor, ReadHashMap } from 'xitdb';

// A replica of the server's view database (server/src/view.rs), the clusters as gem shows them.
// The server's file only grows, so the replica is sent the whole of it once and then only the
// bytes each change appends. Nothing here knows what carries them.

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
  // Each map and list as JSON, by where it is in the file. Committed data is never changed in
  // place, and a change copies only the path to what changed, so after a delta only that path is
  // converted again. Entries for what later moments no longer hold stay until the file is
  // replaced, as a compaction replaces it once it has grown, so they stay about as big as the file.
  private converted = new Map<string, JsonValue>();

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
    this.converted.clear();
    return 'applied';
  }

  // The latest moment, a map of what the server last wrote, or undefined before the first sync.
  latest(): ReadHashMap | undefined {
    if (!this.db) return undefined;
    const cursor = new ReadArrayList(this.db.rootCursor()).getCursor(-1);
    return cursor ? new ReadHashMap(cursor) : undefined;
  }

  // The `ListOpenPullRequestsResponse` JSON the server's view was written from, as pbjson writes
  // it: the clusters in `order`, and its other fields.
  response(): JsonValue | undefined {
    const moment = this.latest();
    if (!moment) return undefined;
    const value = (key: string) => {
      const cursor = moment.getCursor(key);
      return cursor ? this.json(cursor) : undefined;
    };
    const map = moment.getCursor('clusters');
    const clusters = map ? new ReadHashMap(map) : undefined;
    const order = (value('order') ?? []) as string[];
    const shown = order.flatMap((key) => {
      const cursor = clusters?.getCursor(key);
      return cursor ? [this.json(cursor)] : [];
    });
    return { clusters: shown, login: value('login') ?? '', search: value('search') ?? '', agents: value('agents') ?? [] };
  }

  // A value the server's `Structured` codec wrote, as the JSON it was serialized from: a hash map is
  // an object, an array list an array, bytes a string, or a bool when tagged `bl`, and an empty
  // value null. An integer is a number, or a decimal string past 2^53, as protobuf's JSON writes
  // 64-bit integers (pbjson writes those as strings, so the server's never are).
  json(cursor: ReadCursor): JsonValue {
    const slot = cursor.slot();
    switch (slot.tag) {
      case NONE:
        return null;
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
    }
    const at = `${slot.tag}:${slot.value}`;
    const known = this.converted.get(at);
    if (known !== undefined) return known;
    let converted: JsonValue;
    if (slot.tag === HASH_MAP) {
      const object: { [key: string]: JsonValue } = {};
      for (const pair of new ReadHashMap(cursor)) {
        const { keyCursor, valueCursor } = pair.readKeyValuePair();
        object[decoder.decode(keyCursor.readBytes())] = this.json(valueCursor);
      }
      converted = object;
    } else if (slot.tag === ARRAY_LIST) {
      converted = [...new ReadArrayList(cursor)].map((item) => this.json(item));
    } else {
      throw new Error(`the view holds a value tagged ${slot.tag}, which it never writes`);
    }
    this.converted.set(at, converted);
    return converted;
  }
}

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

function integer(value: bigint): number | string {
  const safe = value <= BigInt(Number.MAX_SAFE_INTEGER) && value >= BigInt(Number.MIN_SAFE_INTEGER);
  return safe ? Number(value) : value.toString();
}

const decoder = new TextDecoder();

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
