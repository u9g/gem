import { Check, ChevronLeft, ChevronRight, X } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { fileLines, lineOf, type Section, sectionsOf } from './ClusterFiles';
import type { Proposal, PullRequestCluster } from './gen/gem_pb';
import { approval, ProposalGroup } from './ProposalView';
import type { DiffLine } from './surfaces';
import { Icon, t3, visibleMarkdown } from './t3';

type Thread = { section: Section; group: Proposal[] };

type Handlers = {
  busy?: ReadonlySet<string>;
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  onInvestigate: (id: string) => void;
};

const HIGHLIGHT = 'thread-sheet-line';

// The conversations waiting on the user, as a phone shows them: a card each, in file and line
// order, with where it is, who wrote what, the lines of code it is on, and Dismiss / Approve when
// approving needs no choice between alternatives. A card opens its thread in a sheet over its file.
export function ThreadCards({ cluster, ...handlers }: { cluster: PullRequestCluster } & Handlers) {
  const threads: Thread[] = cluster.pullRequests
    .flatMap(sectionsOf)
    .flatMap((section) => section.groups.map((group) => ({ section, group })));
  const [open, setOpen] = useState<string>();
  // Approving or dismissing the open thread removes it, which closes the sheet.
  const index = threads.findIndex((t) => t.group[0].id === open);
  return (
    <View style={styles.cards}>
      {threads.map((thread) => (
        <Card key={thread.group[0].id} thread={thread} onOpen={() => setOpen(thread.group[0].id)} {...handlers} />
      ))}
      {index >= 0 && (
        <ThreadSheet
          threads={threads}
          index={index}
          onMove={(i) => setOpen(threads[i].group[0].id)}
          onClose={() => setOpen(undefined)}
          {...handlers}
        />
      )}
    </View>
  );
}

function where({ section, group }: Thread): string {
  const ref = `${section.pr.repository?.name}#${section.pr.number}`;
  const proposal = group[0];
  if (section.path) return `${ref} · ${section.path.split('/').pop()}${proposal.comment?.line ? `:${proposal.comment.line}` : ''}`;
  return `${ref} · ${proposal.checkFailure ? 'checks' : 'conversation'}`;
}

// The first line of what the conversation is about, as plain text.
function gist(proposal: Proposal): string {
  if (!proposal.comment) return proposal.summary;
  const body = visibleMarkdown(proposal.comment.body).replace(/<[^>]+>/g, '');
  return body.split('\n').map((l) => l.replace(/[#*`>]/g, '').trim()).find(Boolean) ?? proposal.summary;
}

function Card({ thread, onOpen, busy, onApprove, onDismiss }: { thread: Thread; onOpen: () => void } & Handlers) {
  const { section, group } = thread;
  const proposal = group[0];
  const lines = fileLines(section.pr, section.path);
  const at = lineOf(lines, proposal);
  const disabled = group.some((p) => busy?.has(p.id));
  return (
    <Pressable style={styles.card} onPress={onOpen}>
      <View style={styles.cardHead}>
        <Text numberOfLines={1} style={styles.where}>
          {where(thread)}
        </Text>
        {group.length > 1 ? (
          <Text style={styles.meta}>{group.length} options</Text>
        ) : (
          <>
            <Pressable
              accessibilityLabel="Dismiss"
              disabled={disabled}
              style={[styles.icon, disabled && styles.off]}
              onPress={() => onDismiss(proposal.id)}
            >
              <Icon icon={X} size={14} />
            </Pressable>
            {!proposal.requestedTool && (
              <Pressable
                accessibilityLabel="Approve"
                disabled={disabled}
                style={[styles.icon, styles.approve, disabled && styles.off]}
                onPress={() => onApprove(proposal.id, ...approval(proposal))}
              >
                <Icon icon={Check} size={14} color="#fff" />
              </Pressable>
            )}
          </>
        )}
      </View>
      <Text numberOfLines={2} style={styles.gist}>
        {proposal.comment && <Text style={styles.meta}>{proposal.comment.author} </Text>}
        {gist(proposal)}
      </Text>
      {at >= 0 && <Code lines={lines.slice(Math.max(0, at - 3), at + 2)} highlight={Math.min(at, 3)} />}
    </Pressable>
  );
}

// The thread at `index` in full, in a sheet under its file's diff scrolled to the line it is on.
function ThreadSheet({
  threads,
  index,
  onMove,
  onClose,
  ...handlers
}: { threads: Thread[]; index: number; onMove: (index: number) => void; onClose: () => void } & Handlers) {
  const { section, group } = threads[index];
  const lines = fileLines(section.pr, section.path);
  const at = lineOf(lines, group[0]);
  useEffect(() => {
    document.getElementById(HIGHLIGHT)?.scrollIntoView({ block: 'center' });
  }, [group]);
  return (
    <View style={styles.overlay}>
      <View style={styles.bar}>
        <Pressable accessibilityLabel="Close" onPress={onClose} style={styles.barButton}>
          <Icon icon={ChevronLeft} size={20} color={t3.primary} />
        </Pressable>
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text numberOfLines={1} style={styles.barTitle}>
            {section.path ? section.path.split('/').slice(-2).join('/') : 'Conversation'}
          </Text>
          <Text numberOfLines={1} style={styles.where}>
            {where(threads[index])}
          </Text>
        </View>
        <Pressable
          accessibilityLabel="Previous thread"
          disabled={index === 0}
          onPress={() => onMove(index - 1)}
          style={[styles.barButton, index === 0 && styles.off]}
        >
          <Icon icon={ChevronLeft} size={18} />
        </Pressable>
        <Text style={styles.meta}>
          {index + 1} of {threads.length}
        </Text>
        <Pressable
          accessibilityLabel="Next thread"
          disabled={index === threads.length - 1}
          onPress={() => onMove(index + 1)}
          style={[styles.barButton, index === threads.length - 1 && styles.off]}
        >
          <Icon icon={ChevronRight} size={18} />
        </Pressable>
      </View>
      {lines.length > 0 && (
        <ScrollView style={styles.file}>
          <Code lines={lines} highlight={at} />
        </ScrollView>
      )}
      <View style={[styles.sheet, lines.length === 0 && styles.sheetFull]}>
        <View style={styles.grab} />
        <ScrollView contentContainerStyle={styles.sheetContent}>
          <ProposalGroup proposals={group} inline {...handlers} />
        </ScrollView>
      </View>
    </View>
  );
}

// Diff lines, dense and unwrapped, with the new side's line numbers; `highlight` marks one.
function Code({ lines, highlight }: { lines: DiffLine[]; highlight: number }) {
  return (
    <View style={styles.code}>
      {lines.map((line, i) =>
        line.kind === 'hunk' ? (
          <Text key={i} numberOfLines={1} style={styles.hunk}>
            ⋯ {line.text}
          </Text>
        ) : (
          <View
            key={i}
            nativeID={i === highlight ? HIGHLIGHT : undefined}
            style={[
              styles.line,
              line.kind === 'add' && styles.add,
              line.kind === 'del' && styles.del,
              i === highlight && styles.highlight,
            ]}
          >
            <Text style={styles.number}>{line.kind === 'del' ? '' : line.number}</Text>
            <Text
              numberOfLines={1}
              style={[
                styles.text,
                line.kind === 'add' && { color: '#B7F5D8' },
                line.kind === 'del' && { color: '#FFC9C9' },
              ]}
            >
              {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ' '} {line.text}
            </Text>
          </View>
        ),
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  cards: {
    gap: 8,
  },
  card: {
    gap: 4,
    padding: 10,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  cardHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  where: {
    flex: 1,
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
  },
  meta: {
    color: t3.muted,
    fontSize: 12,
  },
  icon: {
    width: 30,
    height: 30,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: t3.input,
  },
  approve: {
    backgroundColor: t3.primary,
  },
  off: {
    opacity: 0.4,
  },
  gist: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 18,
  },
  code: {
    marginTop: 4,
    borderRadius: 6,
    overflow: 'hidden',
    backgroundColor: '#0D1117',
  },
  hunk: {
    color: 'rgba(130, 170, 255, 0.7)',
    backgroundColor: 'rgba(55, 103, 232, 0.08)',
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
    paddingHorizontal: 8,
  },
  line: {
    flexDirection: 'row',
  },
  add: {
    backgroundColor: 'rgba(16, 185, 129, 0.12)',
  },
  del: {
    backgroundColor: 'rgba(242, 109, 109, 0.12)',
  },
  highlight: {
    boxShadow: `inset 2px 0 ${t3.warning}`,
  } as object,
  number: {
    width: 30,
    paddingRight: 6,
    textAlign: 'right',
    color: 'rgba(208, 207, 212, 0.28)',
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
    userSelect: 'none',
  } as object,
  text: {
    flex: 1,
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
    whiteSpace: 'pre',
  } as object,
  // Covers the cluster, under the browser's own bars.
  overlay: {
    position: 'fixed',
    top: 0,
    bottom: 0,
    left: 0,
    right: 0,
    zIndex: 10,
    backgroundColor: t3.background,
  } as object,
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 48,
    paddingHorizontal: 6,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  barButton: {
    padding: 6,
  },
  barTitle: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 13,
    fontWeight: '600',
  },
  file: {
    flex: 1,
    backgroundColor: '#0D1117',
  },
  // The thread takes at most the lower half, leaving the code it is about in view above it.
  sheet: {
    maxHeight: '55%',
    borderTopLeftRadius: 14,
    borderTopRightRadius: 14,
    backgroundColor: '#1D2630',
    boxShadow: '0 -12px 30px rgba(0, 0, 0, 0.45)',
  } as object,
  sheetFull: {
    flex: 1,
    maxHeight: '100%',
  } as object,
  grab: {
    alignSelf: 'center',
    width: 36,
    height: 4,
    borderRadius: 2,
    marginVertical: 6,
    backgroundColor: 'rgba(255, 255, 255, 0.2)',
  },
  sheetContent: {
    paddingHorizontal: 10,
    paddingBottom: 24,
  },
});
