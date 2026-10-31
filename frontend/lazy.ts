import {
  create,
  type DescEnum,
  type DescField,
  type DescMessage,
  fromJson,
  type JsonValue,
  type MessageShape,
  ScalarType,
} from '@bufbuild/protobuf';
import { base64Decode } from '@bufbuild/protobuf/wire';
import { ReadArrayList, type ReadCursor, ReadHashMap } from 'xitdb';

// Messages read from the server's view database as they're used. The server stores each message
// with xitdb's `Structured` codec, in the shape pbjson's serde gives it: a hash map from each set
// field's JSON name to its value, a list for a repeated field, a map for a map field, and bytes,
// numbers or `bl`-tagged bools for scalars, which pbjson writes as protobuf's JSON does (an enum by
// its name, a 64-bit integer as a decimal string, a Timestamp as RFC 3339).
//
// A view is shaped like the protobuf-es message, `$typeName` included, so anything typed against
// the message reads it, but each field is only read from the database, and converted to
// protobuf-es's form, the first time it's used. Committed data never changes in place, so a view
// is kept by where its value is in the file: what a change didn't touch is the identical object it
// was, and React's memoization holds across changes. Views belong to one file, so they're dropped
// with it.
export class Views {
  private known = new Map<string, unknown>();

  clear() {
    this.known.clear();
  }

  // The message of type `schema` stored at `cursor`, a hash map.
  message<Desc extends DescMessage>(schema: Desc, cursor: ReadCursor): MessageShape<Desc> {
    return this.kept(`${schema.typeName}@`, cursor, () => this.read(schema, cursor)) as MessageShape<Desc>;
  }

  // The messages of type `schema` stored at `cursor`, a list.
  messages<Desc extends DescMessage>(schema: Desc, cursor: ReadCursor): MessageShape<Desc>[] {
    return this.kept(`[${schema.typeName}]@`, cursor, () =>
      [...new ReadArrayList(cursor)].map((item) => this.message(schema, item)),
    ) as MessageShape<Desc>[];
  }

  private kept<T>(kind: string, cursor: ReadCursor, make: () => T): T {
    const slot = cursor.slot();
    const at = `${kind}${slot.tag}:${slot.value}`;
    if (this.known.has(at)) return this.known.get(at) as T;
    const made = make();
    this.known.set(at, made);
    return made;
  }

  private read(schema: DescMessage, cursor: ReadCursor): unknown {
    const map = new ReadHashMap(cursor);
    const value = (field: DescField) => set(map.getCursor(field.jsonName));
    const defaults = defaultsOf(schema);
    const message: Record<string, unknown> = { $typeName: schema.typeName };
    for (const member of schema.members) {
      if (member.kind === 'oneof') {
        lazy(message, member.localName, () => {
          for (const field of member.fields) {
            const stored = value(field);
            if (stored) return { case: field.localName, value: this.singular(field, stored) };
          }
          return { case: undefined };
        });
      } else if (member.localName in defaults) {
        // A field protobuf-es always has holds its default when it isn't stored.
        lazy(message, member.localName, () => {
          const stored = value(member);
          return stored ? this.field(member, stored) : fresh(defaults[member.localName]);
        });
      } else {
        // A field protobuf-es leaves out when unset, like a message field, is looked for now.
        const stored = value(member);
        if (stored) lazy(message, member.localName, () => this.field(member, stored));
      }
    }
    return message;
  }

  private field(field: DescField, cursor: ReadCursor): unknown {
    switch (field.fieldKind) {
      case 'list': {
        const item = (item: ReadCursor) =>
          field.listKind === 'message'
            ? this.message(field.message, item)
            : field.listKind === 'enum'
              ? enumNumber(field.enum, json(item))
              : scalar(field.scalar, field.longAsString, json(item));
        return this.kept(`[${field.parent.typeName}.${field.name}]@`, cursor, () =>
          [...new ReadArrayList(cursor)].map(item),
        );
      }
      case 'map': {
        const entries: Record<string, unknown> = {};
        for (const pair of new ReadHashMap(cursor)) {
          const { keyCursor, valueCursor } = pair.readKeyValuePair();
          const key = decoder.decode(keyCursor.readBytes());
          entries[key] =
            field.mapKind === 'message'
              ? this.message(field.message, valueCursor)
              : field.mapKind === 'enum'
                ? enumNumber(field.enum, json(valueCursor))
                : scalar(field.scalar, false, json(valueCursor));
        }
        return entries;
      }
      default:
        return this.singular(field, cursor);
    }
  }

  private singular(field: DescField, cursor: ReadCursor): unknown {
    switch (field.fieldKind) {
      case 'message':
        // A well-known type pbjson writes as a scalar, like a Timestamp's RFC 3339 string.
        return cursor.slot().tag === HASH_MAP
          ? this.message(field.message, cursor)
          : fromJson(field.message, json(cursor));
      case 'enum':
        return enumNumber(field.enum, json(cursor));
      case 'scalar':
        return scalar(field.scalar, field.longAsString, json(cursor));
      default:
        throw new Error(`${field.toString()} is not a singular field`);
    }
  }
}

// Defines `key` on `target` as a property read by `read` the first time it's used, and kept.
function lazy(target: Record<string, unknown>, key: string, read: () => unknown) {
  Object.defineProperty(target, key, {
    enumerable: true,
    configurable: true,
    get() {
      const value = read();
      Object.defineProperty(target, key, { value, enumerable: true, configurable: true, writable: true });
      return value;
    },
  });
}

const defaults = new WeakMap<DescMessage, Record<string, unknown>>();

// A new message of type `schema`, which has each field protobuf-es always sets, at its default.
function defaultsOf(schema: DescMessage): Record<string, unknown> {
  let made = defaults.get(schema);
  if (!made) defaults.set(schema, (made = create(schema) as unknown as Record<string, unknown>));
  return made;
}

// A default list or map is not shared between messages.
function fresh(value: unknown): unknown {
  if (Array.isArray(value)) return [];
  if (value && typeof value === 'object' && !(value instanceof Uint8Array)) return {};
  return value;
}

// The value at `cursor`, unless it's missing or empty, as a field left out is.
function set(cursor: ReadCursor | null): ReadCursor | undefined {
  return cursor && cursor.slot().tag !== NONE ? cursor : undefined;
}

// An enum's number, from the name pbjson writes, or the number itself.
function enumNumber(desc: DescEnum, value: JsonValue): number {
  if (typeof value === 'number') return value;
  return desc.values.find((enumValue) => enumValue.name === value)?.number ?? 0;
}

// A scalar in protobuf-es's form, from protobuf's JSON form.
function scalar(type: ScalarType, longAsString: boolean, value: JsonValue): unknown {
  switch (type) {
    case ScalarType.INT64:
    case ScalarType.UINT64:
    case ScalarType.SINT64:
    case ScalarType.FIXED64:
    case ScalarType.SFIXED64:
      return longAsString ? String(value) : BigInt(value as string | number);
    case ScalarType.BOOL:
      return value === true || value === 'true';
    case ScalarType.STRING:
      return String(value);
    case ScalarType.BYTES:
      return base64Decode(String(value));
    default:
      // Doubles and floats can be "NaN" or "Infinity".
      return Number(value);
  }
}

const decoder = new TextDecoder();

// The scalar at `cursor` as protobuf's JSON has it: bytes as a string, or a bool when tagged `bl`,
// and integers as numbers, or decimal strings past 2^53.
export function json(cursor: ReadCursor): JsonValue {
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
    case NONE:
      return null;
    default:
      throw new Error(`a value tagged ${slot.tag} where a scalar was expected`);
  }
}

function integer(value: bigint): number | string {
  const safe = value <= BigInt(Number.MAX_SAFE_INTEGER) && value >= BigInt(Number.MIN_SAFE_INTEGER);
  return safe ? Number(value) : value.toString();
}

// xitdb's tags, as numbers: its `Tag` is a const enum, which a bundler that compiles a file at a
// time can't inline.
const NONE = 0;
const HASH_MAP = 4;
const BYTES = 6;
const SHORT_BYTES = 7;
const UINT = 8;
const INT = 9;
const FLOAT = 10;
// The format tag of a bool, `bl`.
const BOOL = [0x62, 0x6c];
