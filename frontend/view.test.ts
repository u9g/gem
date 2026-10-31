// Replays bytes the server's view wrote into a replica, and reads them as the components do. The
// server's test writes them:
//
//   cd server && GEM_VIEW_FIXTURE=<dir> cargo test writes_a_fixture_for_the_frontend
//   cd frontend && GEM_VIEW_FIXTURE=<dir> npx tsx --test view.test.ts
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { Hasher } from 'xitdb';

import { BrowserSha1, type Db, type Kept, same, select } from './db';
import { MemoryDb } from './memory';
import type { Cluster, Moment } from './shapes';
import { blank } from './stories/fixtures';
import { Replica, shown, type ViewBytes } from './view';

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

// The bytes the server wrote, and the JSON of the view's structs before and after its change.
function fixture() {
  const json = JSON.parse(readFileSync(join(dir!, 'fixture.json'), 'utf8'));
  return {
    full: { sent: { ...json.full, start: Number(json.full.start), end: Number(json.full.end) }, bytes: new Uint8Array(readFileSync(join(dir!, 'full.bin'))) },
    delta: { sent: { ...json.delta, start: Number(json.delta.start), end: Number(json.delta.end) }, bytes: new Uint8Array(readFileSync(join(dir!, 'delta.bin'))) },
    first: json.first,
    second: json.second,
  };
}

function replicaOf(...changes: { sent: ViewBytes; bytes: Uint8Array }[]) {
  const replica = new Replica();
  for (const change of changes) assert.equal(replica.apply(change.sent, change.bytes), 'applied');
  return replica;
}

test('the replica follows the bytes the server sends', { skip }, () => {
  const { full, delta } = fixture();
  const replica = new Replica();
  assert.equal(replica.state(), null);
  assert.equal(replica.moment(), undefined);
  // A change doesn't follow on from nothing.
  assert.equal(replica.apply(delta.sent, delta.bytes), 'gap');
  assert.equal(replica.apply(full.sent, full.bytes), 'applied');
  assert.equal(replica.state()?.committedSize, full.sent.end);
  assert.equal(replica.fileId, full.sent.fileId);
  assert.equal(replica.apply(delta.sent, delta.bytes), 'applied');
  // Bytes it has already are not applied again, nor is the older whole copy.
  assert.equal(replica.apply(delta.sent, delta.bytes), 'had');
  assert.equal(replica.apply(full.sent, full.bytes), 'had');
  // Bytes of another file are a gap, until a whole copy of it replaces the replica.
  assert.equal(replica.apply({ ...delta.sent, fileId: '1' }, delta.bytes), 'gap');
  assert.equal(replica.apply({ ...full.sent, fileId: '1' }, full.bytes), 'applied');
});

// `value` as JSON reads it, with the nulls of the Rust structs' unset `Option`s left out, as the
// view leaves them out.
const json = (value: unknown) => JSON.parse(JSON.stringify(value, (_, field) => field ?? undefined));

test('the reader reads every stored form as the server wrote it', { skip }, () => {
  const { full, first } = fixture();
  const moment = replicaOf(full).moment()!;
  const { clusters, agents, login, search } = shown(moment);
  assert.equal(login, first.login);
  assert.equal(search, first.search);
  // Each whole cluster, and the agents, as the server's structs serialize, read through the maps.
  assert.deepEqual(json(clusters), json(first.clusters));
  assert.deepEqual(json(agents), json(first.agents));
  const cluster = clusters[0];
  const [even, odd] = cluster.pullRequests;
  // Enums by name, numbers, bools both ways, times in milliseconds, lists of strings.
  assert.equal(cluster.mergeBy, 'admin');
  assert.equal(even.mergeBy, 'approval');
  assert.equal(cluster.agents[0].state, 'waiting');
  assert.equal(agents[0].state, 'working');
  assert.equal(odd.number, 101);
  assert.equal(even.approved, true);
  assert.equal(odd.approved, false);
  assert.equal(odd.createdAt, first.clusters[0].pullRequests[1].createdAt);
  assert.equal(odd.proposals[0].checkFailure!.checks[0].workflowRunId, Number(1152921504606846977n));
  assert.deepEqual(cluster.agents[0].pullRequestUrls, [cluster.pullRequests[0].url]);
  // Nested maps, and lists that are empty rather than left out.
  assert.equal(odd.repository?.name, 'gem');
  assert.deepEqual(odd.files, []);
  // An unset `Option` is missing: undefined, and not among the map's keys.
  assert.equal(odd.proposals[0].comment, undefined);
  assert.ok(!('comment' in odd.proposals[0]));
  assert.ok(!Object.keys(odd.proposals[0]).includes('comment'));
  assert.ok(Object.keys(odd).includes('noProposalReason'));
  // The clusters map is looked up by key.
  assert.equal(moment.clusters[cluster.pullRequests[0].url], cluster);
  assert.equal(moment.clusters['https://github.com/o/r/pull/9/0'], undefined);
  // Read-only, as plain objects read.
  const { title, url } = odd;
  assert.deepEqual([title, url], [first.clusters[0].pullRequests[1].title, first.clusters[0].pullRequests[1].url]);
  assert.throws(() => {
    (odd as { title: string }).title = 'changed';
  });
  assert.throws(() => (cluster.pullRequests as unknown[]).push(odd));
  // The same stored value is the same object.
  assert.equal(cluster.pullRequests[1], odd);
});

// What `useDb` gives a component: React re-renders it when this is another value.
function selecting<T>(db: Db, read: (moment: Moment | undefined) => T) {
  let kept: Kept<T> | undefined;
  return () => (kept = select(db, read, kept)).value;
}

test('a read changes only when what it read changed', { skip }, () => {
  const { full, delta, second } = fixture();
  const replica = replicaOf(full);
  let notified = 0;
  replica.subscribe(() => notified++);
  const untouched = selecting(replica, (moment) => moment?.clusters['https://github.com/o/r/pull/2/0']);
  const renamed = selecting(replica, (moment) => moment?.clusters['https://github.com/o/r/pull/1/0']);
  const order = selecting(replica, (moment) => moment?.order);
  const name = selecting(replica, (moment) => moment?.clusters['https://github.com/o/r/pull/2/0']?.name);
  const before = { untouched: untouched()!, renamed: renamed()!, order: order(), name: name() };
  // Asked again with nothing applied, each is what it was.
  assert.equal(untouched(), before.untouched);
  assert.equal(order(), before.order);

  assert.equal(replica.apply(delta.sent, delta.bytes), 'applied');
  assert.equal(notified, 1);
  // The cluster the change didn't touch is the same object, as are its lists and nested maps,
  // and what was read of it the same value, so a component that read it isn't rendered again.
  assert.equal(untouched(), before.untouched);
  assert.equal(untouched()!.pullRequests, before.untouched.pullRequests);
  assert.equal(untouched()!.pullRequests[1].repository, before.untouched.pullRequests[1].repository);
  assert.equal(name(), before.name);
  // The cluster it changed, its list of pull requests and the one it changed, and the order, are
  // new; the pull request in it that didn't change is the same object.
  const changed = renamed()!;
  assert.notEqual(changed, before.renamed);
  assert.equal(changed.name, 'renamed');
  assert.notEqual(changed.pullRequests, before.renamed.pullRequests);
  assert.notEqual(changed.pullRequests[0], before.renamed.pullRequests[0]);
  assert.equal(changed.pullRequests[1], before.renamed.pullRequests[1]);
  assert.equal(changed.pullRequests[2].checks, undefined);
  assert.notEqual(order(), before.order);
  assert.deepEqual(order(), second.clusters.map((c: any) => c.pullRequests[0].url));
});

test('reading one cluster reads nothing of the others', { skip }, () => {
  const { full } = fixture();
  const replica = replicaOf(full);
  const { clusters } = shown(replica.moment());
  let lookups = 0;
  const digest = BrowserSha1.prototype.digest;
  BrowserSha1.prototype.digest = function (data) {
    lookups++;
    return digest.call(this, data);
  };
  try {
    void clusters[0].name;
    void clusters[0].pullRequests[0].title;
    // Read again, each is kept.
    void clusters[0].name;
  } finally {
    BrowserSha1.prototype.digest = digest;
  }
  // One key looked up for each field read, and nothing else.
  assert.equal(lookups, 3);
});

test('the fixture writer writes what the reader reads', () => {
  const value = { text: 'a', yes: true, no: false, count: 3, below: -2, ratio: 0.5, empty: [], left: undefined, nested: { list: ['x', 1] } };
  const read = new MemoryDb().put(value);
  assert.deepEqual(json(read), json(value));
  assert.equal(read.left, undefined);
  assert.ok(Object.isFrozen(read.nested.list));
  assert.ok(!Object.keys(read).includes('left'));
});

test('an action going pending, and done, changes what reads it', () => {
  const clusters: Cluster[] = [{ ...blank.cluster, pullRequests: [{ ...blank.pullRequest, url: 'u' }] }];
  const db = new MemoryDb({ clusters });
  const approving = selecting(db, (moment) => moment !== undefined && 'p1' in moment.pending);
  const keys = selecting(db, (moment) => Object.keys(moment?.pending ?? {}).sort());
  assert.equal(approving(), false);
  const before = keys();
  db.show({ clusters, pending: ['p1', 'merge u'] });
  assert.equal(approving(), true);
  assert.deepEqual(keys(), ['merge u', 'p1']);
  assert.notEqual(keys(), before);
  db.show({ clusters });
  assert.equal(approving(), false);
  assert.deepEqual(keys(), []);
});

test('same compares what reads return', () => {
  assert.ok(same([1, 'a', { b: [true] }], [1, 'a', { b: [true] }]));
  assert.ok(!same({ a: 1 }, { a: 1, b: 2 }));
  assert.ok(!same([1], [2]));
});
