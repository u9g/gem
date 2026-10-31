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

import { BrowserSha1, type Db, type Kept, same, select, shape, str, View } from './db';
import { MemoryDb } from './memory';
import { clusterKey, type Moment, PullRequest, shown } from './shapes';
import { Replica, viewBytes } from './view';

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

// The bytes the server wrote, and pbjson's JSON of the view before and after its change.
function fixture() {
  const read = (name: string) => new Uint8Array(readFileSync(join(dir!, name)));
  const json = JSON.parse(readFileSync(join(dir!, 'fixture.json'), 'utf8'));
  return {
    full: { sent: viewBytes(json.full)!, bytes: read('full.bin') },
    delta: { sent: viewBytes(json.delta)!, bytes: read('delta.bin') },
    first: json.first,
    second: json.second,
  };
}

function replicaOf(...changes: { sent: ReturnType<typeof viewBytes>; bytes: Uint8Array }[]) {
  const replica = new Replica();
  for (const change of changes) assert.equal(replica.apply(change.sent!, change.bytes), 'applied');
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

test('views read every field kind as the server wrote it', { skip }, () => {
  const { full, first } = fixture();
  const { clusters, agents, login, search } = shown(replicaOf(full).moment());
  assert.equal(login, first.login);
  assert.equal(search, first.search);
  assert.deepEqual(clusters.map(clusterKey), first.clusters.map((c: any) => c.pullRequests[0].url));
  const cluster = clusters[0];
  const expected = first.clusters[0];
  // Enums by name, and the first value when left out.
  assert.equal(cluster.mergeBy, 'MERGE_BY_ADMIN');
  const [even, odd] = cluster.pullRequests;
  assert.equal(even.mergeBy, 'MERGE_BY_APPROVAL');
  assert.equal(cluster.agents[0].state, 'AGENT_STATE_WAITING');
  assert.equal(agents[0].state, 'AGENT_STATE_WORKING');
  // Timestamps.
  assert.equal(cluster.updatedAt?.getTime(), Date.parse(expected.updatedAt));
  assert.equal(odd.createdAt?.getTime(), Date.parse(expected.pullRequests[1].createdAt));
  // `bl`-tagged bools, and false when left out.
  assert.equal(even.approved, true);
  assert.equal(odd.approved, false);
  assert.equal(odd.checksFailing, true);
  // 32-bit integers, and 64-bit ones past 2^53.
  assert.equal(odd.number, 101);
  assert.equal(odd.checks?.failed, 1);
  assert.equal(even.checks?.failed, 0);
  const { workflowRunId } = odd.proposals[0].checkFailure!.checks[0];
  assert.equal(workflowRunId, 1152921504606846977n);
  // Nested messages, lists and lists of strings.
  assert.equal(odd.repository?.name, 'gem');
  assert.deepEqual(cluster.agents[0].pullRequestUrls, [clusterKey(cluster)]);
  assert.equal(odd.proposals[0].reply, expected.pullRequests[1].proposals[0].reply);
  // Fields left out: their defaults, and no message.
  assert.equal(odd.noProposalReason, '');
  assert.equal(odd.rebasing, false);
  assert.deepEqual(odd.files, []);
  assert.equal(odd.proposals[0].comment, undefined);
  assert.equal(agents[0].startedAt?.getTime(), Date.parse(first.agents[0].startedAt));
  // They behave as plain objects: destructuring, mapping, spreading, and they can't be changed.
  const { title, url } = odd;
  assert.deepEqual([title, url], [expected.pullRequests[1].title, expected.pullRequests[1].url]);
  assert.deepEqual({ ...odd.repository }, { owner: 'u9g', name: 'gem' });
  assert.throws(() => {
    (odd as { title: string }).title = 'changed';
  });
  assert.throws(() => (cluster.pullRequests as PullRequest[]).push(odd));
  // The same stored value is the same view, and the same list the same array.
  assert.equal(cluster.pullRequests[1], odd);
  assert.equal(shown(replicaOf(full).moment()).clusters[0].pullRequests.length, 3);
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
  const cluster = (key: string) => (moment: Moment | undefined) => moment?.clusters.get(key);
  const untouched = selecting(replica, cluster('https://github.com/o/r/pull/2/0'));
  const renamed = selecting(replica, cluster('https://github.com/o/r/pull/1/0'));
  const order = selecting(replica, (moment) => moment?.order);
  const name = selecting(replica, (moment) => moment?.clusters.get('https://github.com/o/r/pull/2/0')?.name);
  const before = { untouched: untouched()!, renamed: renamed()!, order: order(), name: name() };
  // Asked again with nothing applied, each is what it was.
  assert.equal(untouched(), before.untouched);
  assert.equal(order(), before.order);

  assert.equal(replica.apply(delta.sent, delta.bytes), 'applied');
  assert.equal(notified, 1);
  // The cluster the change didn't touch is the same view, as are its lists and nested messages,
  // and what was read of it the same value, so a component that read it isn't rendered again.
  assert.equal(untouched(), before.untouched);
  assert.equal(untouched()!.pullRequests, before.untouched.pullRequests);
  assert.equal(untouched()!.pullRequests[1].repository, before.untouched.pullRequests[1].repository);
  assert.equal(name(), before.name);
  // The cluster it changed, its list of pull requests and the one it changed, and the order, are
  // new; the pull request in it that didn't change is the same view.
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
    // Read again, each is kept on its view.
    void clusters[0].name;
  } finally {
    BrowserSha1.prototype.digest = digest;
  }
  // One key looked up for each field read, and nothing else.
  assert.equal(lookups, 3);
  // The fields not read are still getters.
  const unread = (view: object, field: string) => Object.getOwnPropertyDescriptor(view, field)?.get !== undefined;
  assert.ok(unread(clusters[0], 'agents'));
  assert.ok(unread(clusters[0].pullRequests[1], 'title'));
  assert.ok(clusters.slice(1).every((other) => unread(other, 'name') && unread(other, 'pullRequests')));
});

test('a fixture written in TypeScript reads like the server writes', () => {
  const db = new MemoryDb({
    clusters: [
      {
        name: 'Retry TTS',
        updatedAt: new Date('2026-10-31T12:00:00Z'),
        mergeBy: 'MERGE_BY_APPROVAL',
        pullRequests: [
          {
            url: 'https://github.com/livekit/agents/pull/1',
            number: 1,
            approved: true,
            repository: { owner: 'livekit', name: 'agents' },
            proposals: [{ id: 'p', checkFailure: { checks: [{ workflowRunId: 1n << 60n }] } }],
          },
        ],
      },
    ],
    login: 'u9g',
  });
  let notified = 0;
  db.subscribe(() => notified++);
  const { clusters, login } = shown(db.moment());
  const [pr] = clusters[0].pullRequests;
  assert.equal(login, 'u9g');
  assert.equal(clusters[0].mergeBy, 'MERGE_BY_APPROVAL');
  assert.equal(clusters[0].updatedAt?.toISOString(), '2026-10-31T12:00:00.000Z');
  assert.equal(pr.approved, true);
  assert.equal(pr.number, 1);
  assert.equal(pr.repository?.owner, 'livekit');
  assert.equal(pr.proposals[0].checkFailure?.checks[0].workflowRunId, 1n << 60n);
  assert.equal(pr.merging, false);
  assert.ok(pr instanceof View);
  db.show({ login: 'other' });
  assert.equal(notified, 1);
  assert.equal(shown(db.moment()).login, 'other');
  // A value stored alone, as stories use one.
  assert.equal(new MemoryDb().put(shape({ title: str }), { title: 't' }).title, 't');
});

test('an action going pending, and done, changes what reads it', () => {
  const db = new MemoryDb({ clusters: [{ pullRequests: [{ url: 'u' }] }] });
  const approving = selecting(db, (moment) => moment?.pending.has('p1'));
  const keys = selecting(db, (moment) => moment?.pending.keys().sort());
  const cluster = selecting(db, (moment) => moment?.clusters.get('u'));
  assert.equal(approving(), false);
  const before = { keys: keys(), cluster: cluster() };
  db.show({ clusters: [{ pullRequests: [{ url: 'u' }] }], pending: ['p1', 'merge u'] });
  assert.equal(approving(), true);
  assert.deepEqual(keys(), ['merge u', 'p1']);
  assert.notEqual(keys(), before.keys);
  db.show({ clusters: [{ pullRequests: [{ url: 'u' }] }] });
  assert.equal(approving(), false);
  assert.deepEqual(keys(), []);
});

test('same compares what reads return', () => {
  assert.ok(same([1, 'a', { b: [true] }], [1, 'a', { b: [true] }]));
  assert.ok(same(new Date(5), new Date(5)));
  assert.ok(!same({ a: 1 }, { a: 1, b: 2 }));
  assert.ok(!same([1], [2]));
});
