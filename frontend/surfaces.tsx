import { CheckCheck, FileCode, Hammer } from 'lucide-react';
import type { ReactNode } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from './gen/gem_pb';
import { DiffStat, diffFiles, Icon, t3, visibleMarkdown } from './t3';

// T3 Code's surfaces for what a proposal does, rebuilt in React Native.

type DiffLine = { kind: 'add' | 'del' | 'ctx' | 'hunk'; text: string; number?: number };

function parseDiff(diff: string): { path: string; lines: DiffLine[] }[] {
  const files: { path: string; lines: DiffLine[] }[] = [];
  let next = 0;
  for (const line of diff.split('\n')) {
    if (line.startsWith('+++ ')) files.push({ path: line.slice(6), lines: [] });
    else if (line.startsWith('@@')) {
      next = Number(/\+(\d+)/.exec(line)?.[1] ?? 0);
      files[files.length - 1].lines.push({ kind: 'hunk', text: line.replace(/^@@[^@]*@@ ?/, '') });
    } else if (!files.length || line.startsWith('---') || line === '') continue;
    else if (line.startsWith('+')) files[files.length - 1].lines.push({ kind: 'add', text: line.slice(1), number: next++ });
    else if (line.startsWith('-')) files[files.length - 1].lines.push({ kind: 'del', text: line.slice(1) });
    else files[files.length - 1].lines.push({ kind: 'ctx', text: line.slice(1), number: next++ });
  }
  return files;
}

// T3 Code's diff surface: code background, tinted add/delete lines, a new-line-number gutter.
// `header` replaces the file header's stats, e.g. with an error mark.
export function DiffSurface({ diff, header }: { diff: string; header?: ReactNode }) {
  return (
    <View style={{ gap: 8 }}>
      {parseDiff(diff).map((file) => {
        const stat = diffFiles(diff).find((f) => f.path === file.path)!;
        return (
          <View key={file.path} style={styles.diffSurface}>
            <View style={styles.diffHeader}>
              <Icon icon={FileCode} size={14} />
              <Text numberOfLines={1} style={styles.diffPath}>
                {file.path.split('/').slice(-2).join('/')}
              </Text>
              {header ?? <DiffStat added={stat.added} removed={stat.removed} />}
            </View>
            <ScrollView horizontal>
              <View>
                {file.lines.map((line, i) =>
                  line.kind === 'hunk' ? (
                    <Text key={i} style={styles.hunk}>
                      {'  ⋯  '}
                      {line.text}
                    </Text>
                  ) : (
                    <View
                      key={i}
                      style={[
                        styles.diffLine,
                        line.kind === 'add' && { backgroundColor: 'rgba(16, 185, 129, 0.12)' },
                        line.kind === 'del' && { backgroundColor: 'rgba(242, 109, 109, 0.12)' },
                      ]}
                    >
                      <Text style={styles.gutter}>{line.number ?? ''}</Text>
                      <Text
                        style={[
                          styles.diffText,
                          line.kind === 'add' && { color: '#6EE7B7' },
                          line.kind === 'del' && { color: '#FCA5A5' },
                        ]}
                      >
                        {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ' '} {line.text}
                      </Text>
                    </View>
                  ),
                )}
              </View>
            </ScrollView>
          </View>
        );
      })}
    </View>
  );
}

// T3 Code's message bubble: rounded-2xl on the message surface, at most 80% wide, right-aligned.
export function Bubble({ children }: { children: ReactNode }) {
  return (
    <View style={styles.bubbleRow}>
      <View style={styles.bubble}>{children}</View>
    </View>
  );
}

// T3 Code's markdown blockquote: a 2px rule at 35% of the text color.
export function Quote({ children, lines }: { children: string; lines?: number }) {
  return (
    <View style={styles.quote}>
      <Text numberOfLines={lines} style={styles.quoteText}>
        {children}
      </Text>
    </View>
  );
}

// The comment being answered, as the other side of the conversation.
export function Theirs({ author, body, trailing }: { author: string; body: string; trailing?: ReactNode }) {
  return (
    <View style={{ gap: 2, marginBottom: 8 }}>
      <View style={styles.theirsHeader}>
        <Text style={styles.meta}>{author}</Text>
        {trailing}
      </View>
      <Text numberOfLines={3} style={styles.theirs}>
        {visibleMarkdown(body).replace(/[*#]/g, '').replace(/\n\s*\n/g, '\n')}
      </Text>
    </View>
  );
}

export function ResolvesTag() {
  return (
    <View style={styles.inlineTag}>
      <Icon icon={CheckCheck} size={12} />
      <Text style={styles.inlineTagText}>resolves</Text>
    </View>
  );
}

// T3 Code's warning banner: amber label, the tool's name in mono, why it is needed below.
export function ToolRequestBanner({ proposal }: { proposal: Proposal }) {
  const [name] = proposal.summary.split(':');
  return (
    <View style={styles.warning}>
      <Icon icon={Hammer} color="#F59E0B" />
      <View style={{ flex: 1, gap: 4 }}>
        <Text style={styles.meta}>
          <Text style={{ color: '#F59E0B', fontWeight: '500' }}>Tool request</Text>
          {'  '}
          {proposal.comment?.author}
        </Text>
        <Text style={styles.mono}>{name}</Text>
        <Text style={styles.meta}>{proposal.requestedTool.trim()}</Text>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  meta: {
    color: t3.muted,
    fontSize: 11,
    lineHeight: 16,
  },
  mono: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  diffSurface: {
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
    overflow: 'hidden',
  },
  diffHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 6,
    height: 30,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  diffPath: {
    flex: 1,
    color: t3.foreground,
    fontSize: 12,
  },
  hunk: {
    color: t3.faint,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 20,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  diffLine: {
    flexDirection: 'row',
    minHeight: 18,
  },
  gutter: {
    width: 32,
    paddingRight: 8,
    textAlign: 'right',
    color: t3.faint,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 18,
  },
  diffText: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 18,
    paddingRight: 12,
    whiteSpace: 'pre',
  } as object,
  bubbleRow: {
    alignItems: 'flex-end',
  },
  bubble: {
    maxWidth: '80%',
    gap: 8,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 16,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  theirsHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
  },
  theirs: {
    color: t3.muted,
    fontSize: 13,
    lineHeight: 19,
  },
  quote: {
    borderLeftWidth: 2,
    borderLeftColor: 'rgba(208, 207, 212, 0.35)',
    paddingLeft: 12,
  },
  quoteText: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 19,
  },
  inlineTag: {
    flexDirection: 'row',
    alignItems: 'center',
    alignSelf: 'flex-start',
    paddingRight: 6,
    borderRadius: 999,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  inlineTagText: {
    color: t3.muted,
    fontSize: 11,
  },
  warning: {
    flexDirection: 'row',
    gap: 6,
    padding: 10,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: 'rgba(245, 158, 11, 0.28)',
    backgroundColor: 'rgba(245, 158, 11, 0.08)',
  },
});
