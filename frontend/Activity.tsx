import type { LucideIcon } from 'lucide-react';
import {
  Check,
  CircleDashed,
  CircleX,
  Eye,
  EyeOff,
  GitBranch,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequestClosed,
  GitPullRequestDraft,
  MessageSquare,
  Pencil,
  RotateCcw,
  Undo2,
  X,
} from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { timeAgo } from './ClusterView';
import { ChecksIcon } from './PullRequestList';
import { Html } from './surfaces';
import { Button, Icon, t3, usePhone } from './t3';
import type { Cluster, Event, EventKind, PullRequest } from './view-types';

// Every open PR's activity on one screen, as GitHub's feed and a PR's Conversation tab would show
// it together: one timeline of what happened across all of them, newest day first, each line
// naming who did what on which PR, with the comment or review under it; and beside it the PRs,
// each with its checks, review and latest event, which filter the timeline to one. A phone puts
// the PRs in a row of chips, wrapping, above the timeline.

const ref = (pr: PullRequest) => `${pr.repository?.name ?? ''}#${pr.number}`;

// GitHub's colors for a review's state and a PR's state label.
const approved = '#3FB950';
const changes = '#F85149';
const merged = '#A371F7';
const link = '#8FA8F5';

// What each kind of event looks like on the rail, and the verb that follows the actor.
const kinds: Record<EventKind, { icon: LucideIcon; tone?: string; verb: string; filter: Filter }> = {
  committed: { icon: GitCommitHorizontal, verb: 'pushed to', filter: 'commits' },
  forcePushed: { icon: GitCommitHorizontal, tone: t3.warning, verb: 'force-pushed', filter: 'commits' },
  commented: { icon: MessageSquare, tone: link, verb: 'commented on', filter: 'comments' },
  reviewCommented: { icon: MessageSquare, tone: link, verb: 'commented on a line of', filter: 'comments' },
  approved: { icon: Check, tone: approved, verb: 'approved', filter: 'reviews' },
  changesRequested: { icon: X, tone: changes, verb: 'requested changes on', filter: 'reviews' },
  reviewed: { icon: Eye, tone: link, verb: 'reviewed', filter: 'reviews' },
  reviewDismissed: { icon: Undo2, tone: t3.warning, verb: 'dismissed a review on', filter: 'reviews' },
  reviewRequested: { icon: Eye, verb: 'requested a review of', filter: 'status' },
  readyForReview: { icon: Eye, tone: approved, verb: 'marked ready for review', filter: 'status' },
  convertedToDraft: { icon: GitPullRequestDraft, verb: 'converted to draft', filter: 'status' },
  retargeted: { icon: GitBranch, tone: t3.warning, verb: 'changed the base of', filter: 'status' },
  renamed: { icon: Pencil, verb: 'renamed', filter: 'status' },
  closed: { icon: GitPullRequestClosed, tone: changes, verb: 'closed', filter: 'status' },
  reopened: { icon: RotateCcw, tone: approved, verb: 'reopened', filter: 'status' },
  merged: { icon: GitMerge, tone: merged, verb: 'merged', filter: 'status' },
};

// GitHub's filter on a PR's timeline: by what kind of thing happened.
const filters = ['all', 'comments', 'reviews', 'commits', 'status'] as const;
type Filter = (typeof filters)[number];
const filterLabel: Record<Filter, string> = {
  all: 'All',
  comments: 'Comments',
  reviews: 'Reviews',
  commits: 'Commits',
  status: 'Status changes',
};

type Entry = { event: Event; pr: PullRequest; cluster: Cluster };

// The day an event is in, as GitHub's feed heads each day.
function dayOf(at: number | undefined): string {
  if (at === undefined) return 'Undated';
  const date = new Date(at);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();
  if (sameDay(date, today)) return 'Today';
  if (sameDay(date, yesterday)) return 'Yesterday';
  return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

// How a PR's review stands, from its latest review event and GitHub's decision.
function reviewState(pr: PullRequest): { icon: LucideIcon; color: string; text: string } {
  if (pr.approved) return { icon: Check, color: approved, text: 'Approved' };
  const last = [...(pr.events ?? [])].reverse().find((e) => e.kind === 'approved' || e.kind === 'changesRequested');
  if (last?.kind === 'changesRequested') return { icon: X, color: changes, text: 'Changes requested' };
  return { icon: CircleDashed, color: t3.faint, text: 'Review required' };
}

export function Activity({
  clusters,
  onOpenCluster,
}: {
  clusters: readonly Cluster[];
  // Opens the cluster by its key, the URL of its oldest PR.
  onOpenCluster: (key: string) => void;
}) {
  const phone = usePhone();
  const [filter, setFilter] = useState<Filter>('all');
  const [only, setOnly] = useState<string>();

  const prs = clusters.flatMap((cluster) => cluster.pullRequests.map((pr) => ({ pr, cluster })));
  const entries: Entry[] = prs
    .flatMap(({ pr, cluster }) => (pr.events ?? []).map((event) => ({ event, pr, cluster })))
    .sort((a, b) => (b.event.at ?? 0) - (a.event.at ?? 0));
  const listed = entries.filter(
    ({ event, pr }) => (filter === 'all' || kinds[event.kind].filter === filter) && (!only || pr.url === only),
  );
  const counts = Object.fromEntries(
    filters.map((f) => [f, entries.filter(({ event, pr }) => (f === 'all' || kinds[event.kind].filter === f) && (!only || pr.url === only)).length]),
  ) as Record<Filter, number>;
  const days: { day: string; entries: Entry[] }[] = [];
  for (const entry of listed) {
    const day = dayOf(entry.event.at);
    const last = days[days.length - 1];
    if (last?.day === day) last.entries.push(entry);
    else days.push({ day, entries: [entry] });
  }
  const latest = entries[0]?.event.at;
  const chosen = prs.find(({ pr }) => pr.url === only);

  const sidebar = phone ? (
    <View style={styles.prChips}>
      {prs.map(({ pr }) => (
        <Pressable
          key={pr.url}
          onPress={() => setOnly(only === pr.url ? undefined : pr.url)}
          style={[styles.prChip, only === pr.url && styles.prChipSelected]}
        >
          <ChecksIcon pr={pr} size={12} />
          <Text style={styles.refText}>{ref(pr)}</Text>
          <Text style={styles.count}>{(pr.events ?? []).length}</Text>
        </Pressable>
      ))}
    </View>
  ) : (
    <View style={styles.sidebar}>
      <Text style={styles.sideTitle}>Pull requests · {prs.length}</Text>
      {prs.map(({ pr, cluster }) => (
        <PullRequestRow
          key={pr.url}
          pr={pr}
          cluster={cluster}
          selected={only === pr.url}
          onPress={() => setOnly(only === pr.url ? undefined : pr.url)}
        />
      ))}
    </View>
  );

  return (
    <View style={styles.page}>
      <View style={[styles.header, phone && styles.headerPhone]}>
        <View style={styles.titleBlock}>
          <Text style={[styles.title, phone && styles.titlePhone]}>Activity</Text>
          <Text style={styles.meta}>
            {prs.length} open {prs.length === 1 ? 'pull request' : 'pull requests'} · {entries.length}{' '}
            {entries.length === 1 ? 'event' : 'events'}
            {latest !== undefined && ` · latest ${timeAgo(new Date(latest))}`}
          </Text>
        </View>
        <View style={styles.filters}>
          {filters.map((f) => (
            <Pressable key={f} onPress={() => setFilter(f)} style={[styles.chip, filter === f && styles.chipSelected]}>
              <Text style={[styles.chipText, filter === f && styles.chipTextSelected]}>{filterLabel[f]}</Text>
              <Text style={styles.count}>{counts[f]}</Text>
            </Pressable>
          ))}
        </View>
      </View>

      <View style={[styles.body, phone && styles.bodyPhone]}>
        {phone && sidebar}
        <View style={styles.main}>
          {chosen && (
            <View style={styles.only}>
              <Pressable accessibilityLabel="Show every pull request" onPress={() => setOnly(undefined)} style={styles.onlyChip}>
                <Text style={styles.onlyText}>
                  only <Text style={styles.strong}>{ref(chosen.pr)}</Text> · {chosen.pr.title}
                </Text>
                <Icon icon={X} size={12} color={t3.muted} />
              </Pressable>
              <Button label="Open cluster" variant="ghost" onPress={() => onOpenCluster(chosen.cluster.pullRequests[0].url)} />
            </View>
          )}
          {days.length === 0 ? (
            <View style={styles.empty}>
              <Icon icon={EyeOff} size={24} color={t3.faint} />
              <Text style={styles.emptyText}>
                {entries.length === 0 ? 'Nothing has happened on these pull requests yet.' : 'Nothing matches this filter.'}
              </Text>
            </View>
          ) : (
            <View style={styles.timeline}>
              <View style={styles.rail} />
              {days.map(({ day, entries }) => (
                <View key={day} style={styles.day}>
                  <Divider label={day} />
                  {entries.map((entry) => (
                    <EventLine key={`${entry.pr.url} ${entry.event.id}`} entry={entry} showPr={!only} onOpenCluster={onOpenCluster} />
                  ))}
                </View>
              ))}
            </View>
          )}
        </View>
        {!phone && sidebar}
      </View>
    </View>
  );
}

// One line of the timeline: a badge on the rail, who did what on which PR, when; and under it the
// comment or review as GitHub renders it, or a commit's message.
function EventLine({ entry, showPr, onOpenCluster }: { entry: Entry; showPr: boolean; onOpenCluster: (key: string) => void }) {
  const { event, pr, cluster } = entry;
  const look = kinds[event.kind];
  const actor = event.actor || 'Someone';
  const said = event.bodyHtml !== '' || (event.kind === 'committed' && event.body !== '');
  const open = () => (event.url ? Linking.openURL(event.url) : onOpenCluster(cluster.pullRequests[0].url));
  return (
    <View style={styles.event}>
      <View style={styles.eventLine}>
        <View style={[styles.badge, look.tone !== undefined && { borderColor: look.tone }]}>
          <Icon icon={look.icon} size={14} color={look.tone ?? t3.muted} />
        </View>
        <Text style={styles.eventText}>
          <Text style={styles.strong}>{actor}</Text> {look.verb}{' '}
          {showPr && (
            <Text style={styles.ref} onPress={() => onOpenCluster(cluster.pullRequests[0].url)}>
              {ref(pr)}
            </Text>
          )}
          {!showPr && <Text style={styles.strong}>it</Text>}
          {event.kind === 'reviewRequested' && event.subject !== '' && (
            <Text>
              {' '}
              from <Text style={styles.strong}>{event.subject}</Text>
            </Text>
          )}
          {event.kind === 'retargeted' && event.subject !== '' && (
            <Text>
              {' '}
              to <Text style={styles.mono}>{event.subject}</Text>
            </Text>
          )}
          {event.kind === 'renamed' && event.subject !== '' && <Text> to “{event.subject}”</Text>}
          {event.at !== undefined && <Text style={styles.when}> · {timeAgo(new Date(event.at))}</Text>}
        </Text>
        {event.url !== '' && <Button label="View" variant="ghost" onPress={open} />}
      </View>
      {said && (
        <View style={styles.eventBody}>
          {event.kind === 'committed' ? (
            <Pressable onPress={open} style={styles.commit}>
              <Text style={styles.mono}>{event.id.slice(0, 7)}</Text>
              <Text numberOfLines={1} style={styles.commitText}>
                {event.body}
              </Text>
            </Pressable>
          ) : (
            <View style={styles.card}>
              <View style={styles.cardHead}>
                <Text style={styles.strong}>{actor}</Text>
                {event.kind === 'reviewCommented' && event.subject !== '' && (
                  <Text numberOfLines={1} style={[styles.mono, styles.path]}>
                    {event.subject}
                  </Text>
                )}
                <Text numberOfLines={1} style={styles.cardTitle}>
                  {pr.title}
                </Text>
              </View>
              <View style={styles.cardBody}>
                <Html html={event.bodyHtml} />
              </View>
            </View>
          )}
        </View>
      )}
    </View>
  );
}

function Divider({ label }: { label: string }) {
  return (
    <View style={styles.divider}>
      <Text style={styles.dividerText}>{label}</Text>
      <View style={styles.dividerLine} />
    </View>
  );
}

// A PR in the sidebar: its checks, title, review and latest event, as GitHub's PR list rows them.
function PullRequestRow({
  pr,
  cluster,
  selected,
  onPress,
}: {
  pr: PullRequest;
  cluster: Cluster;
  selected: boolean;
  onPress: () => void;
}) {
  const events = pr.events ?? [];
  const last = events[events.length - 1];
  const review = reviewState(pr);
  const failed = pr.checks?.failed ?? 0;
  return (
    <Pressable onPress={onPress} style={[styles.prRow, selected && styles.prRowSelected]}>
      <View style={styles.prHead}>
        <ChecksIcon pr={pr} size={14} />
        <Text style={styles.ref}>{ref(pr)}</Text>
        {cluster.name !== '' && cluster.pullRequests.length > 1 && (
          <Text numberOfLines={1} style={styles.clusterName}>
            {cluster.name}
          </Text>
        )}
        <Text style={styles.count}>{events.length}</Text>
      </View>
      <Text numberOfLines={2} style={styles.prTitle}>
        {pr.title}
      </Text>
      <View style={styles.facts}>
        <Fact icon={review.icon} color={review.color} text={review.text} />
        {failed > 0 && <Fact icon={CircleX} color={t3.deletion} text={`${failed} failed`} />}
      </View>
      {last && (
        <Text numberOfLines={1} style={styles.lastEvent}>
          {last.actor || 'Someone'} {kinds[last.kind].verb.replace(/ (on|of|to)$/, '')}
          {last.at !== undefined && ` · ${timeAgo(new Date(last.at))}`}
        </Text>
      )}
    </Pressable>
  );
}

function Fact({ icon, color, text }: { icon: LucideIcon; color: string; text: string }) {
  return (
    <View style={styles.fact}>
      <Icon icon={icon} size={12} color={color} />
      <Text style={styles.factText}>{text}</Text>
    </View>
  );
}

const BADGE = 28;
const SIDEBAR = 300;

const styles = StyleSheet.create({
  page: {
    width: '100%',
    maxWidth: 1240,
    alignSelf: 'center',
    gap: 20,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 16,
    paddingBottom: 16,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  headerPhone: {
    flexDirection: 'column',
    alignItems: 'stretch',
    gap: 12,
  },
  titleBlock: {
    flex: 1,
    gap: 6,
  },
  title: {
    color: t3.foreground,
    fontSize: 22,
    fontWeight: '600',
    lineHeight: 28,
  },
  titlePhone: {
    fontSize: 20,
    lineHeight: 26,
  },
  meta: {
    color: t3.faint,
    fontSize: 12,
  },
  filters: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 6,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    height: 28,
    paddingHorizontal: 10,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.border,
  },
  chipSelected: {
    borderColor: 'rgba(55, 103, 232, 0.6)',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
  },
  chipText: {
    color: t3.muted,
    fontSize: 12,
  },
  chipTextSelected: {
    color: t3.foreground,
    fontWeight: '600',
  },
  count: {
    color: t3.faint,
    fontSize: 11,
  },
  body: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 32,
  },
  bodyPhone: {
    flexDirection: 'column',
    alignItems: 'stretch',
    gap: 16,
  },
  main: {
    flex: 1,
    minWidth: 0,
    gap: 16,
  },
  only: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 8,
  },
  onlyChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 28,
    paddingHorizontal: 10,
    borderRadius: 999,
    backgroundColor: t3.input,
    flexShrink: 1,
  },
  onlyText: {
    color: t3.muted,
    fontSize: 12,
    flexShrink: 1,
  },
  empty: {
    alignItems: 'center',
    gap: 8,
    paddingVertical: 48,
  },
  emptyText: {
    color: t3.muted,
    fontSize: 13,
  },
  timeline: {
    gap: 16,
  },
  rail: {
    position: 'absolute',
    top: 8,
    bottom: 8,
    left: BADGE / 2 - 1,
    width: 2,
    backgroundColor: t3.border,
  },
  day: {
    gap: 14,
  },
  divider: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  dividerText: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    paddingHorizontal: 6,
    backgroundColor: t3.background,
    marginLeft: -6,
  },
  dividerLine: {
    flex: 1,
    height: 1,
    backgroundColor: t3.border,
  },
  event: {
    gap: 10,
  },
  eventLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    minHeight: BADGE,
  },
  badge: {
    width: BADGE,
    height: BADGE,
    borderRadius: BADGE / 2,
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: '#222C37',
  },
  eventText: {
    flex: 1,
    color: t3.muted,
    fontSize: 13,
    lineHeight: 19,
  },
  eventBody: {
    marginLeft: BADGE + 10,
  },
  strong: {
    color: t3.foreground,
    fontWeight: '600',
  },
  ref: {
    color: link,
    fontFamily: t3.mono,
    fontSize: 12,
    fontWeight: '600',
  },
  mono: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
    paddingHorizontal: 4,
    borderRadius: 4,
    backgroundColor: t3.input,
  },
  when: {
    color: t3.faint,
  },
  commit: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  commitText: {
    flex: 1,
    color: t3.muted,
    fontSize: 12,
  },
  card: {
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    overflow: 'hidden',
  },
  cardHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  path: {
    flexShrink: 1,
  },
  cardTitle: {
    flex: 1,
    color: t3.faint,
    fontSize: 12,
    textAlign: 'right',
  },
  cardBody: {
    padding: 12,
  },
  sidebar: {
    position: 'sticky',
    top: 0,
    width: SIDEBAR,
    flexShrink: 0,
    gap: 8,
  } as object,
  sideTitle: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    marginBottom: 2,
  },
  prRow: {
    gap: 5,
    padding: 10,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: 'transparent',
  },
  prRowSelected: {
    borderColor: 'rgba(55, 103, 232, 0.6)',
    backgroundColor: 'rgba(55, 103, 232, 0.12)',
  },
  prHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  clusterName: {
    flex: 1,
    color: t3.faint,
    fontSize: 11,
  },
  prTitle: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 18,
  },
  facts: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    columnGap: 12,
    rowGap: 3,
  },
  fact: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  factText: {
    color: t3.muted,
    fontSize: 12,
  },
  lastEvent: {
    color: t3.faint,
    fontSize: 12,
  },
  prChips: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 6,
  },
  prChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    height: 28,
    paddingHorizontal: 10,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.border,
  },
  prChipSelected: {
    borderColor: 'rgba(55, 103, 232, 0.6)',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
  },
  refText: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
});
