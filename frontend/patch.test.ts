//   cd frontend && npx tsx --test patch.test.ts
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { sideBySide } from './patch';

const kinds = ['add', 'del', 'ctx', 'hunk'] as const;
type Kind = (typeof kinds)[number];

// Every diff of up to `length` lines, by its lines' kinds.
function diffs(length: number): Kind[][] {
  if (length === 0) return [[]];
  const shorter = diffs(length - 1);
  return [...shorter, ...shorter.filter((d) => d.length === length - 1).flatMap((d) => kinds.map((kind) => [...d, kind]))];
}

test('side by side, each side has its lines in order and no row is empty', () => {
  for (const diff of diffs(6)) {
    const lines = diff.map((kind) => ({ kind }));
    const rows = sideBySide(lines);
    const on = (side: 0 | 1) => rows.map((row) => row[side]).filter((i) => i !== undefined);
    const indexes = (not: Kind) => lines.flatMap((line, i) => (line.kind === not ? [] : [i]));
    assert.deepEqual(on(0), indexes('add'), diff.join(' '));
    assert.deepEqual(on(1), indexes('del'), diff.join(' '));
    for (const [old, now] of rows) {
      assert.ok(old !== undefined || now !== undefined, diff.join(' '));
      // A row with a line on both sides is one line unchanged, or a deletion beside an addition.
      if (old !== undefined && now !== undefined && old !== now) {
        assert.deepEqual([diff[old], diff[now]], ['del', 'add'], diff.join(' '));
      }
    }
  }
});

test('side by side, a run of deletions is beside the additions that replace it', () => {
  const rows = sideBySide(['ctx', 'del', 'del', 'add', 'del', 'add', 'add', 'ctx'].map((kind) => ({ kind: kind as Kind })));
  assert.deepEqual(rows, [
    [0, 0],
    [1, 3],
    [2, undefined],
    [4, 5],
    [undefined, 6],
    [7, 7],
  ]);
});
