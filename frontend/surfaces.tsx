import { CheckCheck, FileCode, Hammer } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';

import 'github-markdown-css/github-markdown-dark.css';

import type { Comment, Proposal } from './gen/gem_pb';
import { DiffStat, diffFiles, Icon, t3 } from './t3';

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
        // Wide enough for the file's longest line number, so numbers never wrap.
        const digits = String(Math.max(0, ...file.lines.map((line) => line.number ?? 0))).length;
        const gutter = { width: digits * 7 + 14 };
        return (
          <View key={file.path} style={styles.diffSurface}>
            <View style={styles.diffHeader}>
              <Icon icon={FileCode} size={14} />
              <Text numberOfLines={1} style={styles.diffPath}>
                {file.path.split('/').slice(-2).join('/')}
              </Text>
              {header ?? <DiffStat added={stat.added} removed={stat.removed} />}
            </View>
            <ScrollView horizontal contentContainerStyle={styles.diffLines}>
              <View style={styles.diffLines}>
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
                      <Text numberOfLines={1} style={[styles.gutter, gutter]}>
                        {line.number ?? ''}
                      </Text>
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

// T3 Code's message bubble: rounded-2xl on the message surface, at most 80% wide. u9g's messages
// sit on the right and everyone else's on the left, as in a chat.
export function Bubble({ children, side = 'right' }: { children: ReactNode; side?: 'left' | 'right' }) {
  return (
    <View style={{ alignItems: side === 'right' ? 'flex-end' : 'flex-start' }}>
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

// A comment in the conversation, as a message from its author: u9g's on the right like the reply,
// everyone else's on the left. The body is GitHub's own rendering, styled with GitHub's stylesheet,
// cut to `max` pixels when given.
export function Theirs({ comment, trailing, max }: { comment: Comment; trailing?: ReactNode; max?: number }) {
  const side = comment.author === 'u9g' ? 'right' : 'left';
  return (
    <View style={{ gap: 4 }}>
      <View style={[styles.theirsHeader, side === 'right' && styles.theirsHeaderRight]}>
        <Text style={styles.meta}>{comment.author}</Text>
        {trailing}
      </View>
      <Bubble side={side}>
        {max ? (
          <Clamp max={max}>
            <Html html={comment.bodyHtml} />
          </Clamp>
        ) : (
          <Html html={comment.bodyHtml} />
        )}
      </Bubble>
    </View>
  );
}

// Cuts its content to `max` pixels, fading out the last lines, with Show more under it when
// anything was cut.
export function Clamp({ max, children }: { max: number; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [height, setHeight] = useState(0);
  // Cutting less than a couple of lines saves nothing.
  const cut = height > max + 24;
  return (
    <View>
      <View style={[{ overflow: 'hidden' }, cut && !open && ({ maxHeight: max, ...fade } as object)]}>
        <View onLayout={(event) => setHeight(event.nativeEvent.layout.height)}>{children}</View>
      </View>
      {cut && (
        <Pressable onPress={() => setOpen(!open)} style={{ alignSelf: 'flex-start' }}>
          <Text style={styles.more}>{open ? 'Show less' : 'Show more'}</Text>
        </Pressable>
      )}
    </View>
  );
}

const fade = { maskImage: 'linear-gradient(to bottom, black 60%, transparent)' };

// GitHub's rendering of a body, styled with GitHub's stylesheet. Links leave the app for a new
// tab; a click anywhere else calls onPress.
export function Html({ html, onPress }: { html: string; onPress?: () => void }) {
  return (
    <div
      className="markdown-body"
      style={markdownBody}
      dangerouslySetInnerHTML={{ __html: html }}
      onClick={(event) => {
        const link = (event.target as Element).closest('a');
        if (!link) return onPress?.();
        event.preventDefault();
        window.open(link.href, '_blank');
      }}
    />
  );
}

const markdownBody = {
  backgroundColor: 'transparent',
  color: t3.foreground,
  fontSize: 13,
  lineHeight: 1.55,
  minWidth: 0,
};

// A reply as both versions: the agent's suggestion and u9g's rewrite of it.
export type ReplyDraft = { yours: string; showing: 'suggested' | 'yours' };

// The reply's bubble, always editable. Once the text differs from the suggestion, a Suggested /
// Yours switch under it keeps both versions one click away; editing always edits Yours.
export function EditableReply({
  suggested,
  draft,
  onChange,
  quote,
}: {
  suggested: string;
  draft: ReplyDraft;
  onChange: (draft: ReplyDraft) => void;
  // Shown above the reply, for a reply that quotes the comment it answers.
  quote?: string;
}) {
  const [focused, setFocused] = useState(false);
  const hasYours = draft.yours.trim() !== suggested.trim();
  return (
    <View>
      <View style={{ alignItems: 'flex-end' }}>
        <View
          style={[styles.bubble, styles.editableBubble, focused && styles.bubbleFocused]}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        >
          {quote !== undefined && <Quote lines={3}>{quote}</Quote>}
          <TextInput
            multiline
            value={draft.showing === 'yours' ? draft.yours : suggested.trim()}
            onChangeText={(yours) => onChange({ yours, showing: 'yours' })}
            style={styles.input}
          />
        </View>
      </View>
      {hasYours && <Versions draft={draft} onChange={onChange} />}
    </View>
  );
}

// Switches between the agent's suggestion and u9g's rewrite of it.
function Versions({ draft, onChange }: { draft: ReplyDraft; onChange: (draft: ReplyDraft) => void }) {
  return (
    <View style={styles.versions}>
      <View style={styles.segmented}>
        {(['suggested', 'yours'] as const).map((version) => (
          <Pressable
            key={version}
            onPress={() => onChange({ ...draft, showing: version })}
            style={[styles.segment, draft.showing === version && styles.segmentOn]}
          >
            <Text style={[styles.segmentText, draft.showing === version && { color: t3.foreground }]}>
              {version === 'suggested' ? 'Suggested' : 'Yours'}
            </Text>
          </Pressable>
        ))}
      </View>
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
  more: {
    color: t3.muted,
    fontSize: 12,
    paddingTop: 4,
    textDecorationLine: 'underline',
  },
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
  // Tinted lines reach the right edge even when the code is narrower than the surface.
  diffLines: {
    flexGrow: 1,
  },
  gutter: {
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
  bubble: {
    maxWidth: '80%',
    gap: 8,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 16,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  editableBubble: {
    width: '80%',
    borderWidth: 1,
    borderColor: 'transparent',
  },
  bubbleFocused: {
    borderColor: 'rgba(55, 103, 232, 0.7)',
  },
  input: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
    padding: 0,
    outlineStyle: 'solid',
    outlineWidth: 0,
    fieldSizing: 'content',
    resize: 'none',
  } as object,
  versions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 6,
  },
  segmented: {
    flexDirection: 'row',
    padding: 2,
    borderRadius: 8,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  segment: {
    paddingHorizontal: 8,
    height: 20,
    justifyContent: 'center',
    borderRadius: 6,
  },
  segmentOn: {
    backgroundColor: 'rgba(255, 255, 255, 0.08)',
  },
  segmentText: {
    color: t3.muted,
    fontSize: 11,
  },
  theirsHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    paddingLeft: 14,
  },
  theirsHeaderRight: {
    justifyContent: 'flex-end',
    paddingLeft: 0,
    paddingRight: 14,
  },
  theirs: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
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
