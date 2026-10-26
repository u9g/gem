import { Check, ChevronLeft, ChevronRight, X } from 'lucide-react';
import { type ComponentRef, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { fileLines, lineOf, type Section, sectionsOf } from './ClusterFiles';
import type { Proposal, PullRequestCluster } from './gen/gem_pb';
import { approval, ProposalGroup, subjectOf } from './ProposalView';
import type { DiffLine } from './surfaces';
import { Icon, t3 } from './t3';

type Thread = { section: Section; group: Proposal[] };

type Handlers = {
  busy?: ReadonlySet<string>;
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  onInvestigate: (id: string) => void;
};

// The conversations waiting on the user, as a phone shows them: a card each, in file and line
// order, with where it is, who wrote what, the lines of code it is on, and Dismiss / Approve when
// approving needs no choice between alternatives. A card opens its thread in a sheet over its file.
// The open sheet is a history entry holding its cluster and conversation, marked `open` like the
// cluster's own, so back and forward close and reopen it, and it survives a remount.
export function ThreadCards({ cluster, ...handlers }: { cluster: PullRequestCluster } & Handlers) {
  const threads: Thread[] = cluster.pullRequests
    .flatMap(sectionsOf)
    .flatMap((section) => section.groups.map((group) => ({ section, group })));
  const url = cluster.pullRequests[0].url;
  // The open conversation's subject, from an entry of this cluster's: it stays open while any
  // proposal answers it.
  const ours = (state: { cluster?: string; thread?: string } | null) =>
    state?.cluster === url ? state.thread : undefined;
  const [open, setOpen] = useState(() => ours(history.state));
  const index = open ? threads.findIndex((t) => subjectOf(t.group[0]) === open) : -1;
  const entry = (thread: Thread) => ({ open: true, cluster: url, thread: subjectOf(thread.group[0]) });
  // Set from leaving a vanished thread's entry until the browser lands on the one below it.
  const leaving = useRef(false);
  useEffect(() => {
    const land = (event: PopStateEvent) => {
      leaving.current = false;
      setOpen(ours(event.state));
    };
    addEventListener('popstate', land);
    return () => removeEventListener('popstate', land);
  }, [url]);
  // Approving or dismissing the open conversation's last proposal removes it; back leaves its entry.
  useEffect(() => {
    if (open && index < 0 && !leaving.current) {
      leaving.current = true;
      history.back();
    }
  }, [open, index]);
  return (
    <View style={styles.cards}>
      {threads.map((thread) => (
        <Card
          key={thread.group[0].id}
          thread={thread}
          onOpen={() => {
            history.pushState(entry(thread), '');
            setOpen(entry(thread).thread);
          }}
          {...handlers}
        />
      ))}
      {index >= 0 &&
        // Out of the cluster's ScrollView, whose transform would make it the sheet's frame.
        createPortal(
          <ThreadSheet
            threads={threads}
            index={index}
            onMove={(i) => {
              history.replaceState(entry(threads[i]), '');
              setOpen(entry(threads[i]).thread);
            }}
            onClose={() => history.back()}
            {...handlers}
          />,
          document.body,
        )}
    </View>
  );
}

function where({ section, group }: Thread): string {
  const ref = `${section.pr.repository?.name}#${section.pr.number}`;
  const proposal = group[0];
  const subject = proposal.checkFailure ? ' · checks' : '';
  if (section.path) return `${ref} · ${section.path.split('/').pop()}${proposal.comment?.line ? `:${proposal.comment.line}` : ''}${subject}`;
  return `${ref} · ${subject ? 'checks' : 'conversation'}`;
}

// The first line of the text GitHub's rendering of some markdown shows.
function plain(html: string): string {
  const text = new DOMParser().parseFromString(html, 'text/html').body.textContent ?? '';
  return text.split('\n').map((l) => l.trim()).find(Boolean) ?? '';
}

// What the conversation is about: its comment, else what approving does.
function gist(proposal: Proposal): string {
  return (proposal.comment && plain(proposal.comment.bodyHtml)) || plain(proposal.summaryHtml);
}

function Card({ thread, onOpen, busy, onApprove, onDismiss }: { thread: Thread; onOpen: () => void } & Handlers) {
  const { section, group } = thread;
  const proposal = group[0];
  const lines = fileLines(section.pr, section.path);
  const at = lineOf(lines, proposal);
  const disabled = group.some((p) => busy?.has(p.id));
  const failed = group.find((p) => p.error);
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
                accessibilityLabel={proposal.error ? 'Retry' : 'Approve'}
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
      {group.length === 1 && proposal.comment && (
        <Text numberOfLines={1} style={styles.meta}>
          → {plain(proposal.summaryHtml)}
        </Text>
      )}
      {failed && (
        <Text numberOfLines={2} style={styles.error}>
          {failed.error}
        </Text>
      )}
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
      {at >= 0 && (
        <ScrollView style={styles.file}>
          <Code key={group[0].id} lines={lines} highlight={at} full />
        </ScrollView>
      )}
      <View style={[styles.sheet, at < 0 && styles.sheetFull]}>
        <View style={styles.grab} />
        <ScrollView contentContainerStyle={styles.sheetContent}>
          {/* Off the diff, the thread shows its comment's own hunk instead. */}
          <ProposalGroup key={group[0].id} proposals={group} anchored inline={at >= 0} {...handlers} />
        </ScrollView>
      </View>
    </View>
  );
}

// Diff lines, dense, with the new side's line numbers; `highlight` marks one. `full` wraps long
// lines and scrolls the marked one into view; otherwise lines are cut to one.
function Code({ lines, highlight, full = false }: { lines: DiffLine[]; highlight: number; full?: boolean }) {
  const marked = useRef<ComponentRef<typeof View>>(null);
  useEffect(() => {
    if (full) (marked.current as unknown as HTMLElement | null)?.scrollIntoView({ block: 'center' });
  }, [full, highlight]);
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
            ref={i === highlight ? marked : undefined}
            style={[
              styles.line,
              line.kind === 'add' && styles.add,
              line.kind === 'del' && styles.del,
              i === highlight && styles.highlight,
            ]}
          >
            <Text style={styles.number}>{line.kind === 'del' ? '' : line.number}</Text>
            <Text
              numberOfLines={full ? undefined : 1}
              style={[
                styles.text,
                full && styles.wrap,
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
  error: {
    color: t3.deletion,
    fontFamily: t3.mono,
    fontSize: 11,
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
  wrap: {
    whiteSpace: 'pre-wrap',
    wordBreak: 'break-all',
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
