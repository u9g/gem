// A patch cut into one piece per file, each starting at its file's header lines. Joining the pieces
// with newlines gives back the patch exactly.
export function splitFiles(diff: string): string[] {
  const lines = diff.split('\n');
  const starts = [0];
  lines.forEach((line, i) => {
    if (!line.startsWith('+++ ') || !lines[i - 1]?.startsWith('--- ')) return;
    let start = i - 1;
    const previous = starts[starts.length - 1];
    while (start > previous && !/^([ +\-@\\]|$)/.test(lines[start - 1])) start--;
    if (start > previous) starts.push(start);
  });
  return starts.map((start, i) => lines.slice(start, starts[i + 1]).join('\n'));
}
