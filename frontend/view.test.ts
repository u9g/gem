// Replays bytes the server's view wrote into a replica, and checks the lazy views read from it
// are the messages the server wrote. The server's test writes them:
//
//   cd server && GEM_VIEW_FIXTURE=<dir> cargo test writes_a_fixture_for_the_frontend
//   cd frontend && GEM_VIEW_FIXTURE=<dir> npx tsx --test view.test.ts
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { create, fromJson } from '@bufbuild/protobuf';
import { StructSchema } from '@bufbuild/protobuf/wkt';
import { Bytes, CoreMemory, Database, Float, Hasher, WriteHashMap } from 'xitdb';

import { AgentState, ListOpenPullRequestsResponseSchema, MergeBy } from './gen/gem_pb';
import { Views } from './lazy';
import { BrowserSha1, Replica, viewBytes } from './view';

// On the web, Metro resolves the node crypto xitdb's hasher imports to an empty module, as here:
// the replica looks keys up with its own SHA-1 alone.
Hasher.prototype.digest = () => {
  throw new Error("the replica hashed with node's crypto");
};

test('the browser SHA-1 is SHA-1', () => {
  for (const text of ['', 'abc', 'clusters', 'https://github.com/u9g/gem/pull/125', 'x'.repeat(1000)]) {
    const bytes = new TextEncoder().encode(text);
    assert.deepEqual(new BrowserSha1().digest(bytes), new Uint8Array(createHash('sha1').update(bytes).digest()));
  }
});

const dir = process.env.GEM_VIEW_FIXTURE;
const skip = !dir && 'GEM_VIEW_FIXTURE is not set';

function fixture() {
  const read = (name: string) => new Uint8Array(readFileSync(join(dir!, name)));
  const json = JSON.parse(readFileSync(join(dir!, 'fixture.json'), 'utf8'));
  return {
    full: { sent: viewBytes(json.full)!, bytes: read('full.bin') },
    delta: { sent: viewBytes(json.delta)!, bytes: read('delta.bin') },
    first: fromJson(ListOpenPullRequestsResponseSchema, json.first),
    second: fromJson(ListOpenPullRequestsResponseSchema, json.second),
  };
}

test('lazy views are the messages the server wrote', { skip }, () => {
  const { full, delta, first, second } = fixture();
  const replica = new Replica();
  assert.equal(replica.state(), null);
  // A change doesn't follow on from nothing.
  assert.equal(replica.apply(delta.sent, delta.bytes), 'gap');
  assert.equal(replica.apply(full.sent, full.bytes), 'applied');
  assert.equal(replica.state()?.committedSize, full.sent.end);
  assert.equal(replica.fileId, full.sent.fileId);

  const before = replica.view()!;
  assert.equal(before.login, first.login);
  assert.equal(before.search, first.search);
  // Every field kind as protobuf-es has it: enums as numbers, 64-bit integers as bigints,
  // Timestamps as messages, bools both ways, fields left out at their defaults, and nested and
  // repeated messages.
  const pr = before.clusters[0].pullRequests[1];
  assert.equal(pr.$typeName, 'gem.v1.PullRequest');
  assert.equal(pr.mergeBy, MergeBy.APPROVAL);
  assert.equal(before.clusters[0].mergeBy, MergeBy.ADMIN);
  assert.equal(pr.createdAt?.seconds, 1_790_000_001n);
  assert.equal(pr.approved, false);
  assert.equal(pr.checksFailing, true);
  assert.equal(pr.rebasing, false);
  assert.equal(pr.files.length, 0);
  assert.equal(pr.proposals[0].checkFailure?.checks[0].workflowRunId, 1152921504606846977n);
  assert.equal(before.clusters[0].agents[0].state, AgentState.WAITING);
  assert.equal(before.agents[0].state, AgentState.WORKING);
  assert.deepStrictEqual(before.clusters, first.clusters);
  assert.deepStrictEqual(before.agents, first.agents);

  assert.equal(replica.apply(delta.sent, delta.bytes), 'applied');
  const after = replica.view()!;
  assert.deepStrictEqual(after.clusters, second.clusters);
  // A message field left out is left out, as protobuf-es leaves it out.
  assert.equal('checks' in after.clusters[1].pullRequests[2], false);
  // A cluster the change didn't touch is the object it was.
  assert.equal(after.clusters[0], before.clusters[2]);
  assert.notEqual(after.clusters[1], before.clusters[1]);

  // Bytes it has already are not applied again, nor is the older whole copy.
  assert.equal(replica.apply(delta.sent, delta.bytes), 'had');
  assert.equal(replica.apply(full.sent, full.bytes), 'had');
  // Bytes of another file are a gap, until a whole copy of it replaces the replica.
  assert.equal(replica.apply({ ...delta.sent, fileId: '1' }, delta.bytes), 'gap');
  assert.equal(replica.apply({ ...full.sent, fileId: '1' }, full.bytes), 'applied');
  assert.deepStrictEqual(replica.view()!.clusters, first.clusters);
});

test('reading one cluster reads nothing of the others', { skip }, () => {
  const { full } = fixture();
  const replica = new Replica();
  replica.apply(full.sent, full.bytes);
  const { clusters } = replica.view()!;
  const unread = (message: object, field: string) =>
    Object.getOwnPropertyDescriptor(message, field)?.get !== undefined;
  assert.ok(clusters[0].pullRequests[0].title);
  assert.ok(!unread(clusters[0], 'pullRequests'));
  assert.ok(unread(clusters[0], 'name'));
  assert.ok(unread(clusters[0].pullRequests[1], 'title'));
  for (const other of clusters.slice(1)) {
    assert.ok(['pullRequests', 'name', 'agents', 'mergeBy'].every((field) => unread(other, field)));
  }
});

// gem's messages have no map fields, nor oneofs, so a Struct, which has both, is written here as
// the server's codec would write it: a map of its fields, each a Value with one case of its kind.
test('lazy views read maps and oneofs', () => {
  const db = new Database(new CoreMemory(), new BrowserSha1());
  db.runContext((cursor) => {
    const struct = new WriteHashMap(cursor);
    const fields = new WriteHashMap(struct.putCursor('fields'));
    new WriteHashMap(fields.putCursor('n')).put('numberValue', new Float(1.5));
    new WriteHashMap(fields.putCursor('b')).put('boolValue', new Bytes('true', 'bl'));
    new WriteHashMap(fields.putCursor('s')).put('stringValue', new Bytes('x'));
    new WriteHashMap(fields.putCursor('z')).put('nullValue', new Bytes('NULL_VALUE'));
  });
  const view = new Views().message(StructSchema, db.rootCursor());
  const expected = fromJson(StructSchema, { n: 1.5, b: true, s: 'x', z: null });
  assert.deepStrictEqual(view, expected);
  assert.deepStrictEqual(new Views().message(StructSchema, emptyMap()), create(StructSchema));
});

function emptyMap() {
  const db = new Database(new CoreMemory(), new BrowserSha1());
  db.runContext((cursor) => void new WriteHashMap(cursor));
  return db.rootCursor();
}
