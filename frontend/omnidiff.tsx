import type { ThemedToken } from '@pierre/highlights';
import { useCallback, useMemo } from 'react';
import { Text } from 'react-native';

import { useDb } from './db';
import { tokenStyle, Tokens } from './highlight';
import { diffKey } from './patch';
import { t3 } from './t3';
import type { DiffSummary, FileHighlights, Moment, SideHighlights } from './view-types';

// What omnidiff (https://github.com/ivankovic/omnidiff) makes of the diffs gem shows, as the server
// keeps it in the view: in each line a diff deletes or adds, the parts that changed and the parts
// that only moved, which a diff marks rather than leaving the whole line to say it changed. A diff
// shows as before until omnidiff has read it, and always where the server has no omnidiff.

// What omnidiff made of each file of `diff`, once the server has it.
export function useHighlights(diff: string): readonly FileHighlights[] | undefined {
  const key = useMemo(() => diffKey(diff), [diff]);
  // An older server's view has no `diffs`.
  const read = useCallback((moment: Moment | undefined) => moment?.diffs?.[key]?.files, [key]);
  return useDb(read);
}

// A marked part of a line: where it starts and ends in the line's text, and whether it moved.
export type Mark = { start: number; end: number; moved: boolean };

// One file's marks on each side, by line number, each line's in order.
export type Marks = { before: ReadonlyMap<number, Mark[]>; after: ReadonlyMap<number, Mark[]> };

export function marksOf(file: FileHighlights | undefined): Marks | undefined {
  if (!file) return undefined;
  const side = ({ changed, moved }: SideHighlights) => {
    const lines = new Map<number, Mark[]>();
    for (const [triples, isMoved] of [
      [changed, false],
      [moved, true],
    ] as const) {
      for (let i = 0; i + 2 < triples.length; i += 3) {
        const marks = lines.get(triples[i]) ?? [];
        marks.push({ start: triples[i + 1], end: triples[i + 2], moved: isMoved });
        lines.set(triples[i], marks);
      }
    }
    for (const marks of lines.values()) marks.sort((a, b) => a.start - b.start);
    return lines;
  };
  return { before: side(file.before), after: side(file.after) };
}

// The tint of a deleted or added line, fainter when omnidiff found nothing on it that changed, as
// on a line the diff only realigned, or one whose brackets alone moved.
export function lineTint(kind: 'add' | 'del', quiet: boolean): string {
  const [r, g, b] = kind === 'add' ? [16, 185, 129] : [242, 109, 109];
  return `rgba(${r}, ${g}, ${b}, ${quiet ? 0.05 : 0.12})`;
}

const MOVED = 'rgba(96, 165, 250, 0.3)';
const MOVED_TEXT = '#93C5FD';

// A deleted or added line's text, its marks set off in its side's color and moved ones in blue,
// in the tokens' colors once they're ready. A mark over the whole of an inserted or deleted line
// says no more than the line's tint, so it isn't set off.
export function Painted({
  tokens,
  text,
  marks,
  kind,
}: {
  tokens?: ThemedToken[];
  text: string;
  marks?: readonly Mark[];
  kind: 'add' | 'del';
}) {
  const shown = setOff(text, marks ?? []);
  if (!shown.length) return <Tokens tokens={tokens} text={text} />;
  const color = kind === 'add' ? 'rgba(16, 185, 129, 0.35)' : 'rgba(242, 109, 109, 0.35)';
  // The tokens, or the line as one until they're ready, each cut where a mark starts or ends.
  const pieces: { content: string; token?: ThemedToken }[] = tokens?.map((token) => ({ content: token.content, token })) ?? [
    { content: text },
  ];
  const out = [];
  let [at, m] = [0, 0];
  for (const { content, token } of pieces) {
    const end = at + content.length;
    for (let from = at; from < end; ) {
      while (m < shown.length && shown[m].end <= from) m++;
      const mark = shown[m];
      const inside = mark !== undefined && mark.start <= from;
      const to = Math.min(end, inside ? mark.end : (mark?.start ?? end));
      out.push(
        <Text
          key={out.length}
          style={[tokenStyle(token), inside && { backgroundColor: mark.moved ? MOVED : color }]}
        >
          {content.slice(from - at, to - at)}
        </Text>,
      );
      from = to;
    }
    at = end;
  }
  return out;
}

// The marks to set off, apart and in order.
function setOff(text: string, marks: readonly Mark[]): Mark[] {
  const first = text.search(/\S/);
  const last = text.trimEnd().length;
  const shown: Mark[] = [];
  for (const mark of marks) {
    if (!mark.moved && mark.start <= first && mark.end >= last) continue;
    const start = Math.max(mark.start, shown[shown.length - 1]?.end ?? 0);
    if (start < mark.end) shown.push({ ...mark, start });
  }
  return shown;
}

function summaryLabel(summary: DiffSummary): string | undefined {
  switch (summary) {
    case 'whitespaceOnly':
      return 'whitespace only';
    case 'commentsOnly':
      return 'comments only';
    case 'movesOnly':
      return 'moves only';
  }
}

// What a file's header says of omnidiff's reading: the change's kind, when it's all of one, and
// that blue marks moved code, when some moved.
export function HighlightNote({ file }: { file?: FileHighlights }) {
  if (!file) return null;
  const label = summaryLabel(file.summary);
  const moved = file.before.moved.length > 0 || file.after.moved.length > 0;
  if (!label && !moved) return null;
  return (
    <Text numberOfLines={1} style={{ color: moved ? MOVED_TEXT : t3.faint, fontSize: 11, marginRight: 4 }}>
      {label ?? 'moved'}
    </Text>
  );
}
