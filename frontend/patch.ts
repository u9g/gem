// A patch cut into one piece per file, each starting at its file's header lines. Joining the pieces
// with newlines gives back the patch exactly.
export function splitFiles(diff: string): string[] {
  const lines = diff.split('\n');
  const starts = [0];
  // Lines left in the current hunk on each side, from its header; within them, every line is code.
  let [oldLeft, newLeft] = [0, 0];
  lines.forEach((line, i) => {
    if (oldLeft > 0 || newLeft > 0) {
      if (line.startsWith('+')) newLeft--;
      else if (line.startsWith('-')) oldLeft--;
      else if (!line.startsWith('\\')) [oldLeft, newLeft] = [oldLeft - 1, newLeft - 1];
      return;
    }
    const hunk = /^@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))?/.exec(line);
    if (hunk) [oldLeft, newLeft] = [Number(hunk[1] ?? 1), Number(hunk[2] ?? 1)];
    if (!line.startsWith('+++ ') || !lines[i - 1]?.startsWith('--- ')) return;
    let start = i - 1;
    const previous = starts[starts.length - 1];
    while (start > previous && !/^([ +\-@\\]|$)/.test(lines[start - 1])) start--;
    if (start > previous) starts.push(start);
  });
  return starts.map((start, i) => lines.slice(start, starts[i + 1]).join('\n'));
}

type Hunk = { oldStart: number; oldCount: number; newCount: number; context: string; body: string[]; trailer: string[] };
type Line = { text: string; noEol: boolean };

// A file's header lines and hunks, or undefined unless every hunk parses and the file still exists
// after the patch. `trailer` holds the lines after a hunk's counted body, such as the patch's final
// empty line.
function parseFile(piece: string): { header: string[]; hunks: Hunk[] } | undefined {
  const lines = piece.split('\n');
  let i = lines.findIndex((line) => line.startsWith('@@'));
  if (i < 0 || !lines.some((line) => line.startsWith('+++ ')) || lines.includes('+++ /dev/null')) return undefined;
  const header = lines.slice(0, i);
  const hunks: Hunk[] = [];
  while (i < lines.length) {
    const match = /^@@ -(\d+)(?:,(\d+))? \+\d+(?:,(\d+))? @@(.*)$/.exec(lines[i++]);
    if (!match) return undefined;
    const [, oldStart, oldCount = '1', newCount = '1', context] = match;
    const hunk: Hunk = { oldStart: +oldStart, oldCount: +oldCount, newCount: +newCount, context, body: [], trailer: [] };
    let old = 0;
    let now = 0;
    while (i < lines.length && (old < hunk.oldCount || now < hunk.newCount || lines[i].startsWith('\\'))) {
      const line = lines[i++];
      hunk.body.push(line);
      if (line.startsWith('\\')) continue;
      if (!line.startsWith('+')) old++;
      if (!line.startsWith('-')) now++;
    }
    while (i < lines.length && !lines[i].startsWith('@@')) hunk.trailer.push(lines[i++]);
    hunks.push(hunk);
  }
  return { header, hunks };
}

// A hunk's lines before and after the patch. A line followed by "\ No newline at end of file" ends
// its file without a newline.
function sides(body: string[]): { before: Line[]; after: Line[] } {
  const before: Line[] = [];
  const after: Line[] = [];
  let previous = ' ';
  for (const line of body) {
    if (line.startsWith('\\')) {
      if (previous !== '+') before[before.length - 1].noEol = true;
      if (previous !== '-') after[after.length - 1].noEol = true;
      continue;
    }
    previous = line[0] ?? ' ';
    if (previous !== '+') before.push({ text: line.slice(1), noEol: false });
    if (previous !== '-') after.push({ text: line.slice(1), noEol: false });
  }
  return { before, after };
}

// A hunk body turning `before` into `after`, by longest common subsequence, with each run of
// deletions ahead of its additions as git writes them.
function body(before: Line[], after: Line[]): string[] {
  const key = (line: Line) => (line.noEol ? `${line.text}\n` : line.text);
  const lcs = before.map(() => after.map(() => 0));
  const at = (i: number, j: number) => (i < before.length && j < after.length ? lcs[i][j] : 0);
  for (let i = before.length - 1; i >= 0; i--)
    for (let j = after.length - 1; j >= 0; j--)
      lcs[i][j] = key(before[i]) === key(after[j]) ? at(i + 1, j + 1) + 1 : Math.max(at(i + 1, j), at(i, j + 1));
  const out: string[] = [];
  const push = (prefix: string, line: Line) => {
    out.push(prefix + line.text);
    if (line.noEol) out.push('\\ No newline at end of file');
  };
  let i = 0;
  let j = 0;
  while (i < before.length || j < after.length) {
    if (i < before.length && j < after.length && key(before[i]) === key(after[j])) push(' ', after[j++]), i++;
    else if (i < before.length && (j === after.length || at(i + 1, j) >= at(i, j + 1))) push('-', before[i++]);
    else push('+', after[j++]);
  }
  return out;
}

// Each hunk's new start line follows from its old start and the lines the hunks above it add.
function newStarts(hunks: Hunk[]): number[] {
  let shift = 0;
  return hunks.map((hunk) => {
    const start = hunk.oldStart + (hunk.oldCount ? 0 : 1) + shift - (hunk.newCount ? 0 : 1);
    shift += hunk.newCount - hunk.oldCount;
    return start;
  });
}

// Each hunk of a file as the file reads after the patch, or undefined for a file that can't be
// edited this way.
export function newSides(piece: string): { start: number; context: string; lines: string[] }[] | undefined {
  const file = parseFile(piece);
  if (!file) return undefined;
  const starts = newStarts(file.hunks);
  return file.hunks.map((hunk, i) => ({
    start: starts[i],
    context: hunk.context.replace(/^ /, ''),
    lines: sides(hunk.body).after.map((line) => line.text),
  }));
}

// The file with hunk `index` rewritten so the file reads `lines` there after the patch. Every hunk
// still applies to the same lines before the patch.
export function editHunk(piece: string, index: number, lines: string[]): string {
  const file = parseFile(piece)!;
  const hunk = file.hunks[index];
  const { before, after } = sides(hunk.body);
  const edited = lines.map((text) => ({ text, noEol: false }));
  if (after[after.length - 1]?.noEol && edited.length) edited[edited.length - 1].noEol = true;
  hunk.body = body(before, edited);
  hunk.newCount = edited.length;
  const starts = newStarts(file.hunks);
  return [
    ...file.header,
    ...file.hunks.flatMap((h, i) => [`@@ -${h.oldStart},${h.oldCount} +${starts[i]},${h.newCount} @@${h.context}`, ...h.body, ...h.trailer]),
  ].join('\n');
}
