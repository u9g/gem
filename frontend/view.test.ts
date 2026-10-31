// Replays bytes the server's view wrote into a replica, and checks it shows what the server wrote.
// The server's test writes them:
//
//   cd server && GEM_VIEW_FIXTURE=<dir> cargo test writes_a_fixture_for_the_frontend
//   cd frontend && GEM_VIEW_FIXTURE=<dir> npx tsx --test view.test.ts
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { Hasher } from 'xitdb';

import { Replica, viewBytes } from './view';

// On the web, Metro resolves the node crypto xitdb's hasher imports to an empty module, so the
// replica must never hash.
Hasher.prototype.digest = () => {
  throw new Error('the replica hashed');
};

const dir = process.env.GEM_VIEW_FIXTURE;

test('a replica shows what the server wrote', { skip: !dir && 'GEM_VIEW_FIXTURE is not set' }, () => {
  const read = (name: string) => new Uint8Array(readFileSync(join(dir!, name)));
  const fixture = JSON.parse(readFileSync(join(dir!, 'fixture.json'), 'utf8'));
  const full = viewBytes(fixture.full)!;
  const delta = viewBytes(fixture.delta)!;
  // The parts of a response the server writes, as pbjson leaves them out when empty.
  const shown = (response: { clusters?: unknown[]; login?: string; search?: string; agents?: unknown[] }) => ({
    clusters: response.clusters ?? [],
    login: response.login ?? '',
    search: response.search ?? '',
    agents: response.agents ?? [],
  });

  const replica = new Replica();
  assert.equal(replica.state(), null);
  // A change doesn't follow on from nothing.
  assert.equal(replica.apply(delta, read('delta.bin')), 'gap');
  assert.equal(replica.apply(full, read('full.bin')), 'applied');
  assert.deepEqual(replica.response(), shown(fixture.first));
  assert.equal(replica.state()?.committedSize, full.end);
  assert.equal(replica.fileId, full.fileId);

  assert.equal(replica.apply(delta, read('delta.bin')), 'applied');
  assert.deepEqual(replica.response(), shown(fixture.second));
  // Bytes it has already are not applied again, nor is the older whole copy.
  assert.equal(replica.apply(delta, read('delta.bin')), 'had');
  assert.equal(replica.apply(full, read('full.bin')), 'had');
  // Bytes of another file are a gap, until a whole copy of it replaces the replica.
  assert.equal(replica.apply({ ...delta, fileId: '1' }, read('delta.bin')), 'gap');
  assert.equal(replica.apply({ ...full, fileId: '1' }, read('full.bin')), 'applied');
  assert.deepEqual(replica.response(), shown(fixture.first));
});
