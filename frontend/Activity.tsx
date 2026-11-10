import type { LucideIcon } from 'lucide-react';
import {
  Bot,
  Check,
  ChevronDown,
  ChevronUp,
  Eye,
  EyeOff,
  GitBranch,
  GitMerge,
  GitPullRequestClosed,
  GitPullRequestDraft,
  MessageSquare,
  Pencil,
  RotateCcw,
  Undo2,
  X,
} from 'lucide-react';
import { type ComponentRef, type ReactNode, useContext, useLayoutEffect, useRef, useState } from 'react';
import { ActivityIndicator, Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { type ClusterView, timeAgo } from './ClusterView';
import { bySubject, ProposalGroup, Stage } from './ProposalView';
import { Html, Login } from './surfaces';
import { Button, Icon, t3, usePhone } from './t3';
import type { Cluster, Event, EventKind, PullRequest } from './view-types';

// Every open PR's activity on one screen, as GitHub's feed and a PR's Conversation tab would show
// it together: one timeline of what happened across all of them, newest day first, each line
// naming who did what on which PR, with the comment or review under it. Under a comment waiting
// on the user is the agent's answer to it: its proposals, to approve or dismiss in place, or that
// it is answering.
//
// Off a phone, the change a proposal makes is a stage: under the proposal's band it runs from the
// window's left edge to its right one, across the column the page is otherwise in, and the column
// goes on under it.

const ref = (pr: PullRequest) => `${pr.repository?.name ?? ''}#${pr.number}`;

// GitHub's colors for a review's state and a PR's state label.
const approved = '#3FB950';
const changes = '#F85149';
const merged = '#A371F7';
const link = '#8FA8F5';

// What each kind of event looks like on the rail, and the verb that follows the actor.
type Look = { icon: LucideIcon; tone?: string; verb: string; filter: Filter };
const kinds: Record<EventKind, Look> = {
  commented: { icon: MessageSquare, tone: link, verb: 'commented on', filter: 'comments' },
  // Only inside its review's card, never a line of its own.
  reviewCommented: { icon: MessageSquare, tone: link, verb: 'commented on a line of', filter: 'reviews' },
  approved: { icon: Check, tone: approved, verb: 'approved', filter: 'reviews' },
  changesRequested: { icon: X, tone: changes, verb: 'requested changes on', filter: 'reviews' },
  reviewed: { icon: Eye, tone: link, verb: 'reviewed', filter: 'reviews' },
  reviewDismissed: { icon: Undo2, tone: t3.warning, verb: 'dismissed a review on', filter: 'reviews' },
  readyForReview: { icon: Eye, tone: approved, verb: 'marked ready for review', filter: 'status' },
  convertedToDraft: { icon: GitPullRequestDraft, verb: 'converted to draft', filter: 'status' },
  retargeted: { icon: GitBranch, tone: t3.warning, verb: 'changed the base of', filter: 'status' },
  renamed: { icon: Pencil, verb: 'renamed', filter: 'status' },
  closed: { icon: GitPullRequestClosed, tone: changes, verb: 'closed', filter: 'status' },
  reopened: { icon: RotateCcw, tone: approved, verb: 'reopened', filter: 'status' },
  merged: { icon: GitMerge, tone: merged, verb: 'merged', filter: 'status' },
};

// GitHub's filter on a PR's timeline: by what kind of thing happened.
const filters = ['all', 'comments', 'reviews', 'status'] as const;
type Filter = (typeof filters)[number];
const filterLabel: Record<Filter, string> = {
  all: 'All',
  comments: 'Comments',
  reviews: 'Reviews',
  status: 'Status changes',
};

type Entry = { event: Event; look: Look; pr: PullRequest; cluster: Cluster };

type Actions = Pick<Parameters<typeof ClusterView>[0], 'busy' | 'onApprove' | 'onDismiss' | 'onInvestigate'>;

// One event after another on the same cluster reads as a chat app shows messages from one sender:
// the first line of the stretch leads it, at full size and ending in a chip that names the cluster;
// the lines after it follow, closer together under a smaller badge and ending in only the chip's
// dot, which says "the cluster above".

// A hue per cluster, by its place in the list, so a cluster that comes back further down the
// timeline is recognizably the one from before. Light enough to read on the background, and none
// of them a color the badges and PR links already speak in.
const hues = ['#56D4BC', '#F28BB8', '#C3D66B', '#E8A87C', '#6FD3F2', '#B9A2F5'];

// A cluster of one PR has no name of its own; its PR's title stands for it.
const nameOf = (cluster: Cluster) => cluster.name || cluster.pullRequests[0]?.title || '';

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

export function Activity({
  clusters,
  onOpenCluster,
  ...actions
}: {
  clusters: readonly Cluster[];
  // Opens the cluster by its key, the URL of its oldest PR.
  onOpenCluster: (key: string) => void;
} & Actions) {
  const phone = usePhone();
  const login = useContext(Login);
  const [filter, setFilter] = useState<Filter>('all');

  const prs = clusters.flatMap((cluster) => cluster.pullRequests.map((pr) => ({ pr, cluster })));
  const entries: Entry[] = prs
    .flatMap(({ pr, cluster }) => (pr.events ?? []).map((event) => ({ event, look: kinds[event.kind], pr, cluster })))
    // What the user wrote themself is no news to them: their comments, and their replies on lines,
    // which GitHub wraps in a review.
    .filter(({ event }) => !(event.actor === login && (event.kind === 'commented' || event.kind === 'reviewed')))
    .sort((a, b) => (b.event.at ?? 0) - (a.event.at ?? 0));
  const listed = entries.filter(({ look }) => filter === 'all' || look.filter === filter);
  const counts = Object.fromEntries(
    filters.map((f) => [f, entries.filter(({ look }) => f === 'all' || look.filter === f).length]),
  ) as Record<Filter, number>;
  const days: { day: string; entries: Entry[] }[] = [];
  for (const entry of listed) {
    const day = dayOf(entry.event.at);
    const last = days[days.length - 1];
    if (last?.day === day) last.entries.push(entry);
    else days.push({ day, entries: [entry] });
  }
  const hueOf = (cluster: Cluster) => hues[clusters.indexOf(cluster) % hues.length];
  const latest = entries[0]?.event.at;

  // The stages down the page, in order. Each is as wide as the view that scrolls the page; how far
  // that reaches to either side of a proposal only the browser's layout knows, so it is read from
  // there and written back whenever it changes, before the browser paints.
  const page = useRef<ComponentRef<typeof View>>(null);
  const [order, setOrder] = useState<string[]>([]);
  const scroller = () => {
    let element = (page.current as unknown as HTMLElement | null)?.parentElement;
    while (element && !/auto|scroll/.test(getComputedStyle(element).overflowY)) element = element.parentElement;
    return element ?? undefined;
  };
  const measure = () => {
    const frame = scroller();
    if (!frame) return;
    const stages = [...frame.querySelectorAll<HTMLElement>('[data-stage]')];
    const left = frame.getBoundingClientRect().left;
    const lefts = stages.map((stage) => stage.getBoundingClientRect().left);
    stages.forEach((stage, i) => {
      const surface = stage.firstElementChild as HTMLElement;
      surface.style.marginLeft = `${left - lefts[i]}px`;
      surface.style.width = `${frame.clientWidth}px`;
    });
    const ids = stages.map((stage) => stage.dataset.stage!);
    setOrder((old) => (old.join('\n') === ids.join('\n') ? old : ids));
  };
  // A stage that comes, goes or moves sideways changes what the page holds, how tall it is or how
  // wide its window is.
  useLayoutEffect(measure);
  useLayoutEffect(() => {
    const frame = scroller();
    if (!frame) return;
    const observer = new ResizeObserver(measure);
    observer.observe(frame);
    observer.observe(page.current as unknown as HTMLElement);
    return () => observer.disconnect();
  }, []);

  // What a stage's bar says for the page, which is scrolled away above it: who commented on which
  // PR and the cluster that is in, then which change this is and the way to the ones before and
  // after it.
  const bar = (id: string, pullRequestUrl: string) => {
    const found = prs.find(({ pr }) => pr.proposals.some((proposal) => proposal.id === id));
    if (!found) return null;
    const { pr, cluster } = found;
    const at = order.indexOf(id);
    const go = (to: number) => () =>
      scroller()
        ?.querySelector(`[data-stage="${CSS.escape(order[to])}"]`)
        ?.scrollIntoView({ block: 'start', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
    return (
      <>
        <View {...({ dataSet: { gather: '' } } as object)} style={styles.barContext}>
          <Text numberOfLines={1} style={styles.barText}>
            <Text style={styles.strong}>{pr.proposals.find((proposal) => proposal.id === id)?.comment?.author}</Text> on{' '}
            <Text style={styles.ref}>{ref(pr)}</Text>
            {pullRequestUrl !== pr.url && ` · changes ${pullRequestUrl.split('/').slice(-3).filter((part) => part !== 'pull').join('#')}`}
          </Text>
          <Text numberOfLines={1} style={[styles.clusterChip, styles.barChip]}>
            <Text style={{ color: hueOf(cluster) }}>● </Text>
            {nameOf(cluster)}
          </Text>
        </View>
        <View style={styles.barSteps}>
          <Text style={styles.barText}>
            Change {at + 1} of {order.length}
          </Text>
          <Step icon={ChevronUp} label="Previous change" disabled={at <= 0} onPress={go(at - 1)} />
          <Step icon={ChevronDown} label="Next change" disabled={at >= order.length - 1} onPress={go(at + 1)} />
        </View>
      </>
    );
  };

  const feed = (
    <View ref={page} style={styles.page}>
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

      <View style={styles.main}>
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
                {entries.map((entry, i) => (
                  <EventLine
                    key={`${entry.pr.url} ${entry.event.id}`}
                    entry={entry}
                    // Which line is above is asked of what is listed, so a filter that takes away
                    // the events between two stretches of a cluster joins them.
                    // A day's first line always leads: its heading has come between, and the
                    // line it would follow may be scrolled away.
                    place={i === 0 ? 'first' : entries[i - 1].cluster === entry.cluster ? 'follows' : 'leads'}
                    hue={hueOf(entry.cluster)}
                    onOpenCluster={onOpenCluster}
                    {...actions}
                  />
                ))}
              </View>
            ))}
          </View>
        )}
      </View>
    </View>
  );
  return <Stage value={phone ? undefined : bar}>{feed}</Stage>;
}

function Step({ icon, label, disabled, onPress }: { icon: LucideIcon; label: string; disabled: boolean; onPress: () => void }) {
  return (
    <Pressable accessibilityLabel={label} disabled={disabled} onPress={onPress} style={[styles.step, disabled && { opacity: 0.35 }]}>
      <Icon icon={icon} size={14} color={t3.foreground} />
    </Pressable>
  );
}

// One line of the timeline: a badge on the rail, who did what on which PR, when; and under it the
// comment or review as GitHub renders it, a review with its comments on lines; and under each
// comment, the agent's answer to it.
function EventLine({
  entry,
  place,
  hue,
  onOpenCluster,
  ...actions
}: {
  entry: Entry;
  // The day's first line, one that leads a new cluster's stretch, or one that follows the line
  // above on the same cluster.
  place: 'first' | 'leads' | 'follows';
  hue: string;
  onOpenCluster: (key: string) => void;
} & Actions) {
  const { event, look, pr, cluster } = entry;
  const actor = event.actor || 'Someone';
  const comments = event.comments;
  const said = event.bodyHtml !== '' || comments.length > 0;
  const open = () => (event.url ? Linking.openURL(event.url) : onOpenCluster(cluster.pullRequests[0].url));
  const follows = place === 'follows';
  const phone = usePhone();
  return (
    <View style={[styles.event, place === 'leads' && styles.leads, follows && styles.follows]}>
      <View style={[styles.eventLine, follows && styles.eventLineFollows]}>
        <View style={styles.badgeCell}>
          <View style={[styles.badge, follows && styles.badgeFollows, look.tone !== undefined && { borderColor: look.tone }]}>
            <Icon icon={look.icon} size={follows ? 11 : 14} color={look.tone ?? t3.muted} />
          </View>
        </View>
        <Text style={styles.eventText}>
          <Text style={styles.strong}>{actor}</Text> {look.verb}{' '}
          <Text style={styles.ref} onPress={() => onOpenCluster(cluster.pullRequests[0].url)}>
            {ref(pr)}
          </Text>
          {event.kind === 'retargeted' && event.subject !== '' && (
            <Text>
              {' '}
              to <Text style={styles.mono}>{event.subject}</Text>
            </Text>
          )}
          {event.kind === 'renamed' && event.subject !== '' && <Text> to “{event.subject}”</Text>}
          {event.at !== undefined && <Text style={styles.when}> · {timeAgo(new Date(event.at))}</Text>}{' '}
          {follows ? (
            <Text accessibilityLabel={nameOf(cluster)} style={[styles.sameCluster, { color: hue }]}>
              ●
            </Text>
          ) : (
            <Text style={styles.clusterChip} onPress={() => onOpenCluster(cluster.pullRequests[0].url)}>
              <Text style={{ color: hue }}>● </Text>
              {nameOf(cluster)}
            </Text>
          )}
        </Text>
        {event.url !== '' && <Button label="View" variant="ghost" onPress={open} />}
      </View>
      {said && (
        <View style={styles.eventBody}>
          <View style={[styles.card, phone && styles.cardPhone]}>
            <View style={styles.cardHead}>
              <Text style={styles.strong}>{actor}</Text>
              <Text numberOfLines={1} style={styles.cardTitle}>
                {pr.title}
              </Text>
            </View>
            {event.bodyHtml !== '' && (
              <View style={styles.cardBody}>
                <Html html={event.bodyHtml} />
                <Answer event={event} pr={pr} {...actions} />
              </View>
            )}
            {comments.map((comment, i) => (
              <View key={comment.id} style={(i > 0 || event.bodyHtml !== '') && styles.lineComment}>
                <Pressable
                  disabled={comment.url === ''}
                  onPress={() => Linking.openURL(comment.url)}
                  style={styles.lineCommentHead}
                >
                  <Icon icon={MessageSquare} size={12} color={t3.muted} />
                  <Text numberOfLines={1} style={[styles.mono, styles.path]}>
                    {comment.subject || 'a line'}
                  </Text>
                </Pressable>
                <View style={styles.cardBody}>
                  <Html html={comment.bodyHtml} />
                  <Answer event={comment} pr={pr} {...actions} />
                </View>
              </View>
            ))}
          </View>
        </View>
      )}
    </View>
  );
}

// The agent's answer to a comment waiting on the user: what it proposes, and whether it is still
// answering. Nothing for anything else.
function Answer({ event, pr, ...actions }: { event: Event; pr: PullRequest } & Actions) {
  const groups = bySubject(pr.proposals.filter((p) => p.comment?.id === event.id));
  if (!event.answering && groups.length === 0) return null;
  const alternatives = groups.reduce((n, group) => n + group.length, 0);
  return (
    <View style={styles.answer}>
      <View style={styles.answerHead}>
        <Icon icon={Bot} size={12} color={link} />
        <Text style={styles.answerText}>
          {groups.length > 0 && `Agent proposed ${alternatives === 1 ? 'an answer' : `${alternatives} answers`}`}
          {groups.length > 0 && event.answering && ' · '}
          {event.answering && (groups.length > 0 ? 'still answering' : 'Agent is answering')}
        </Text>
        {event.answering && <ActivityIndicator size={12} color={link} />}
      </View>
      {groups.map((group) => (
        <ProposalGroup
          key={group[0].id}
          proposals={group}
          busy={actions.busy}
          onApprove={actions.onApprove}
          onDismiss={actions.onDismiss}
          onInvestigate={actions.onInvestigate}
        />
      ))}
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

const BADGE = 28;

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
  main: {
    flex: 1,
    minWidth: 0,
    gap: 16,
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
  // On the day's gap of 14: a line that follows sits closer to the one it follows, and one that
  // leads stands off from the cluster above it.
  leads: {
    marginTop: 10,
  },
  follows: {
    marginTop: -4,
  },
  eventLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    minHeight: BADGE,
  },
  eventLineFollows: {
    minHeight: 20,
  },
  // As wide as a full badge, so the smaller one of a line that follows stays on the rail.
  badgeCell: {
    width: BADGE,
    alignItems: 'center',
  },
  badgeFollows: {
    width: 20,
    height: 20,
  },
  // In the line's text, after the time, so it wraps with it on a phone; a name too long for the
  // line is cut short rather than broken across two.
  clusterChip: {
    display: 'inline-block',
    maxWidth: '100%',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    verticalAlign: 'bottom',
    marginLeft: 4,
    paddingHorizontal: 8,
    borderRadius: 999,
    backgroundColor: t3.input,
    color: t3.foreground,
    fontSize: 11,
    fontWeight: '600',
  } as object,
  sameCluster: {
    marginLeft: 4,
    fontSize: 11,
    opacity: 0.7,
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
  // Off a phone nothing in a card is cut to it: a stage in it runs past both its edges.
  card: {
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
  },
  cardPhone: {
    overflow: 'hidden',
  },
  cardHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderTopLeftRadius: 11,
    borderTopRightRadius: 11,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  path: {
    flexShrink: 1,
  },
  lineComment: {
    borderTopWidth: 1,
    borderTopColor: t3.border,
  },
  lineCommentHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 12,
    paddingTop: 10,
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
  answer: {
    gap: 8,
    marginTop: 10,
  },
  answerHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  answerText: {
    color: t3.muted,
    fontSize: 12,
  },
  barContext: {
    flexShrink: 1,
    minWidth: 0,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  barText: {
    flexShrink: 0,
    color: t3.muted,
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  barChip: {
    flexShrink: 1,
    maxWidth: 280,
    marginLeft: 0,
    lineHeight: 20,
  },
  barSteps: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  step: {
    width: 24,
    height: 24,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 6,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
});
