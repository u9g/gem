import type { LucideIcon } from 'lucide-react';
import {
  Check,
  CircleX,
  Eye,
  EyeOff,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  MessageSquare,
  Undo2,
  UserRoundPlus,
} from 'lucide-react';
import { useContext, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { Event as Line, Rail, ref, Strong, When } from './ClusterTimeline';
import { timeAgo } from './ClusterView';
import { Login } from './surfaces';
import { Icon, t3, usePhone } from './t3';
import type { Cluster, Event, EventKind, PullRequest } from './view-types';

// Every open pull request's conversation on one page, as GitHub's Conversation tab would read
// laid end to end: under each PR, a timeline of who commented, reviewed, asked for a review or
// changed its state, newest PR first. GitHub's notifications leave out what you did yourself, and
// so does this until asked.

type Tab = 'all' | 'comments' | 'reviews' | 'status';

const tabs: [Tab, string][] = [
  ['all', 'All'],
  ['comments', 'Comments'],
  ['reviews', 'Reviews'],
  ['status', 'Status'],
];

const tabOf: Record<EventKind, Tab> = {
  unspecified: 'status',
  comment: 'comments',
  approved: 'reviews',
  changesRequested: 'reviews',
  reviewed: 'reviews',
  reviewDismissed: 'reviews',
  readyForReview: 'status',
  convertedToDraft: 'status',
  reviewRequested: 'status',
  reviewRequestRemoved: 'status',
  reopened: 'status',
  closed: 'status',
};

const link = '#8FA8F5';

// How each kind is drawn on the rail, and what its actor did, in GitHub's words.
const kinds: Record<EventKind, { icon: LucideIcon; tone?: string; verb: string }> = {
  unspecified: { icon: MessageSquare, verb: 'did something' },
  comment: { icon: MessageSquare, verb: 'commented' },
  approved: { icon: Check, tone: t3.addition, verb: 'approved these changes' },
  changesRequested: { icon: CircleX, tone: t3.deletion, verb: 'requested changes' },
  reviewed: { icon: Eye, verb: 'reviewed' },
  reviewDismissed: { icon: Undo2, tone: t3.warning, verb: 'dismissed a review' },
  readyForReview: { icon: GitPullRequest, tone: link, verb: 'marked this pull request as ready for review' },
  convertedToDraft: { icon: GitPullRequestDraft, verb: 'converted this pull request to draft' },
  reviewRequested: { icon: UserRoundPlus, verb: 'requested a review from' },
  reviewRequestRemoved: { icon: EyeOff, verb: 'removed the review request for' },
  reopened: { icon: GitPullRequestArrow, tone: t3.addition, verb: 'reopened this' },
  closed: { icon: GitPullRequestClosed, tone: t3.deletion, verb: 'closed this' },
};

// A comment's prose, without the HTML comments bots open with.
const prose = (markdown: string) => markdown.replace(/<!--[\s\S]*?-->/g, '').trim();

export function Activity({
  clusters,
  onOpenCluster,
}: {
  clusters: readonly Cluster[];
  // Opens the cluster holding the PR with this URL.
  onOpenCluster: (key: string) => void;
}) {
  const phone = usePhone();
  const login = useContext(Login);
  const [tab, setTab] = useState<Tab>('all');
  const [mine, setMine] = useState(false);

  const prs = clusters.flatMap((cluster) => cluster.pullRequests);
  const others = (event: Event) => mine || event.actor !== login;
  const counts: Record<Tab, number> = { all: 0, comments: 0, reviews: 0, status: 0 };
  for (const event of prs.flatMap((pr) => pr.events).filter(others)) {
    counts.all++;
    counts[tabOf[event.kind]]++;
  }
  const shown = prs
    .map((pr) => ({ pr, events: pr.events.filter((e) => others(e) && (tab === 'all' || tabOf[e.kind] === tab)) }))
    .filter(({ events }) => events.length > 0)
    .sort((a, b) => (b.events.at(-1)?.at ?? 0) - (a.events.at(-1)?.at ?? 0));
  const quiet = prs.filter((pr) => !shown.some((s) => s.pr === pr));

  return (
    <View style={styles.page}>
      <View style={[styles.header, phone && styles.headerPhone]}>
        <View style={styles.titleBlock}>
          <Text style={[styles.title, phone && styles.titlePhone]}>Activity</Text>
          <Text style={styles.meta}>
            {counts.all} {counts.all === 1 ? 'event' : 'events'} on {shown.length} of {prs.length}{' '}
            {prs.length === 1 ? 'pull request' : 'pull requests'}
          </Text>
        </View>
        <Pressable accessibilityRole="switch" accessibilityState={{ checked: mine }} style={styles.toggle} onPress={() => setMine(!mine)}>
          <View style={[styles.check, mine && styles.checkOn]}>{mine && <Icon icon={Check} size={11} color="#fff" />}</View>
          <Text style={styles.toggleText}>Include mine</Text>
        </Pressable>
      </View>

      <View style={styles.tabs}>
        {tabs.map(([key, label]) => (
          <Pressable key={key} onPress={() => setTab(key)} style={[styles.tab, tab === key && styles.tabOn]}>
            <Text style={[styles.tabText, tab === key && styles.tabTextOn]}>{label}</Text>
            <Text style={styles.tabCount}>{counts[key]}</Text>
          </Pressable>
        ))}
      </View>

      {shown.length === 0 && (
        <View style={styles.empty}>
          <Icon icon={MessageSquare} size={28} color={t3.faint} />
          <Text style={styles.emptyTitle}>Nothing yet</Text>
          <Text style={styles.emptyText}>
            {mine ? 'Nobody has said or changed anything on these pull requests.' : 'Nobody else has said or changed anything on these pull requests.'}
          </Text>
        </View>
      )}
      {shown.map(({ pr, events }) => (
        <PullRequestActivity key={pr.url} pr={pr} events={events} onOpen={() => onOpenCluster(pr.url)} />
      ))}
      {quiet.length > 0 && (
        <Text style={styles.quiet}>
          Nothing {tab === 'all' ? '' : `under ${tabs.find(([key]) => key === tab)![1].toLowerCase()} `}on{' '}
          {quiet.map((pr, i) => (
            <Text key={pr.url}>
              {i > 0 && ', '}
              <Text style={styles.quietRef} onPress={() => onOpenCluster(pr.url)}>
                {ref(pr)}
              </Text>
            </Text>
          ))}
        </Text>
      )}
    </View>
  );
}

// One PR as a row of GitHub's pull request list, with its timeline open under it.
function PullRequestActivity({ pr, events, onOpen }: { pr: PullRequest; events: Event[]; onOpen: () => void }) {
  const latest = events.at(-1)?.at;
  return (
    <View style={styles.card}>
      <View style={styles.cardHead}>
        <Icon icon={GitPullRequest} size={16} color="#3FB950" />
        <View style={styles.cardTitle}>
          <Text style={styles.prTitle} onPress={onOpen}>
            {pr.title}
          </Text>
          <Text style={styles.prMeta}>
            <Text style={styles.prRef} onPress={() => Linking.openURL(pr.url)}>
              {ref(pr)}
            </Text>
            {pr.createdAt !== undefined && ` · opened ${timeAgo(new Date(pr.createdAt))}`}
            {latest !== undefined && ` · last ${timeAgo(new Date(latest))}`}
          </Text>
        </View>
        <ReviewPill pr={pr} />
      </View>
      <View style={styles.cardBody}>
        <Rail>
          {events.map((event) => (
            <EventLine key={event.id} event={event} />
          ))}
        </Rail>
      </View>
    </View>
  );
}

// GitHub's review state label on the PR list: approved, changes requested, or review required.
function ReviewPill({ pr }: { pr: PullRequest }) {
  const changes = pr.events.findLast((e) => e.kind === 'approved' || e.kind === 'changesRequested')?.kind === 'changesRequested';
  const [color, label] = pr.approved
    ? [t3.addition, 'Approved']
    : changes
      ? [t3.deletion, 'Changes requested']
      : [t3.faint, 'Review required'];
  return (
    <Text numberOfLines={1} style={[styles.pill, { color, borderColor: `${color}55` }]}>
      {label}
    </Text>
  );
}

function EventLine({ event }: { event: Event }) {
  const { icon, tone, verb } = kinds[event.kind];
  const said = event.kind === 'comment' || event.kind === 'approved' || event.kind === 'changesRequested' || event.kind === 'reviewed';
  const body = said ? prose(event.body) : '';
  const named = event.kind === 'reviewRequested' || event.kind === 'reviewRequestRemoved';
  return (
    <Line
      icon={icon}
      tone={tone}
      body={
        body !== '' && (
          <Pressable onPress={() => Linking.openURL(event.url)} style={styles.quote}>
            <Text numberOfLines={3} style={styles.quoteText}>
              {body}
            </Text>
          </Pressable>
        )
      }
    >
      <Strong>{event.actor}</Strong> {verb}
      {named && (
        <>
          {' '}
          <Strong>{event.body}</Strong>
        </>
      )}
      {event.comments > 0 && (
        <Text style={styles.faint}>
          {' '}
          · {event.comments} inline {event.comments === 1 ? 'comment' : 'comments'}
        </Text>
      )}
      <When at={event.at} />
    </Line>
  );
}

const styles = StyleSheet.create({
  page: {
    width: '100%',
    maxWidth: 900,
    alignSelf: 'center',
    gap: 16,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 16,
    paddingRight: 24,
  },
  headerPhone: {
    flexDirection: 'column',
    paddingRight: 0,
    gap: 10,
  },
  titleBlock: {
    flex: 1,
    gap: 4,
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
  toggle: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 28,
  },
  check: {
    width: 16,
    height: 16,
    borderRadius: 4,
    borderWidth: 1,
    borderColor: t3.muted,
    alignItems: 'center',
    justifyContent: 'center',
  },
  checkOn: {
    backgroundColor: t3.primary,
    borderColor: t3.primary,
  },
  toggleText: {
    color: t3.muted,
    fontSize: 13,
  },
  // GitHub's underline nav.
  tabs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  tab: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginBottom: -1,
    borderBottomWidth: 2,
    borderBottomColor: 'transparent',
  },
  tabOn: {
    borderBottomColor: '#F78166',
  },
  tabText: {
    color: t3.muted,
    fontSize: 13,
  },
  tabTextOn: {
    color: t3.foreground,
    fontWeight: '600',
  },
  tabCount: {
    color: t3.muted,
    fontSize: 11,
    paddingHorizontal: 6,
    borderRadius: 999,
    backgroundColor: t3.input,
    fontVariant: ['tabular-nums'],
  },
  card: {
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    overflow: 'hidden',
  },
  cardHead: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 10,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  cardTitle: {
    flex: 1,
    minWidth: 0,
    gap: 2,
  },
  prTitle: {
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '600',
    lineHeight: 20,
  },
  prMeta: {
    color: t3.faint,
    fontSize: 12,
  },
  prRef: {
    color: link,
    fontFamily: t3.mono,
  },
  pill: {
    fontSize: 11,
    fontWeight: '600',
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: 999,
    borderWidth: 1,
  },
  cardBody: {
    padding: 14,
  },
  quote: {
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  quoteText: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 19,
  },
  faint: {
    color: t3.faint,
  },
  quiet: {
    color: t3.faint,
    fontSize: 12,
    lineHeight: 18,
  },
  quietRef: {
    color: t3.muted,
    fontFamily: t3.mono,
  },
  empty: {
    alignItems: 'center',
    gap: 6,
    paddingVertical: 48,
  },
  emptyTitle: {
    color: t3.foreground,
    fontSize: 15,
    fontWeight: '600',
  },
  emptyText: {
    color: t3.muted,
    fontSize: 13,
    textAlign: 'center',
  },
});
