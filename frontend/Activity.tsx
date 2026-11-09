import type { LucideIcon } from 'lucide-react';
import {
  Check,
  CircleX,
  Eye,
  EyeOff,
  GitBranch,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  MessageSquare,
} from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { timeAgo } from './ClusterView';
import { ChecksIcon } from './PullRequestList';
import { Clamp, Html } from './surfaces';
import { Button, Icon, t3, usePhone } from './t3';
import { Badge, Card, Dim, Event, Mono, ref, Strong, Timeline, When } from './Timeline';
import type { Cluster, Event as EventShape, EventKind, PullRequest } from './view-types';

// Every open pull request's activity on one screen, as GitHub's Conversation tab shows one PR's:
// each PR under a header like a row of GitHub's pull request list, then its timeline, what was
// pushed, said, reviewed and changed about it, oldest first. PRs with the latest activity come
// first, and the chips on top narrow every timeline to one kind of event, as GitHub's notification
// filters do.

// GitHub's filters, each the event kinds it keeps; `all` keeps every kind.
type Filter = 'all' | 'comments' | 'reviews' | 'pushes' | 'status';
const filters: Record<Filter, readonly EventKind[] | null> = {
  all: null,
  comments: ['commented', 'reviewed'],
  reviews: ['approved', 'changesRequested', 'reviewRequested', 'reviewDismissed'],
  pushes: ['committed', 'forcePushed'],
  status: ['readyForReview', 'convertedToDraft', 'baseChanged', 'closed', 'reopened', 'merged'],
};
const filterLabel: Record<Filter, string> = {
  all: 'All',
  comments: 'Comments',
  reviews: 'Reviews',
  pushes: 'Pushes',
  status: 'Status changes',
};

const merged = '#A371F7';

export function Activity({ clusters, onOpenCluster }: { clusters: readonly Cluster[]; onOpenCluster: (key: string) => void }) {
  const phone = usePhone();
  const [filter, setFilter] = useState<Filter>('all');
  const prs = clusters
    .flatMap((cluster) => cluster.pullRequests)
    .map((pr) => ({ pr, events: pr.events.filter((event) => filters[filter]?.includes(event.kind) ?? true) }))
    .filter(({ events }) => filter === 'all' || events.length > 0)
    .sort((a, b) => latest(b.pr) - latest(a.pr));
  const total = clusters.reduce((n, cluster) => n + cluster.pullRequests.length, 0);
  const count = (f: Filter) =>
    clusters.reduce(
      (n, cluster) => n + cluster.pullRequests.flatMap((pr) => pr.events).filter((event) => filters[f]?.includes(event.kind) ?? true).length,
      0,
    );

  return (
    <View style={styles.page}>
      <View style={[styles.header, phone && styles.headerPhone]}>
        <View style={styles.titleBlock}>
          <Text style={[styles.title, phone && styles.titlePhone]}>Activity</Text>
          <Text style={styles.meta}>
            {filter === 'all' ? `${total} open ${total === 1 ? 'pull request' : 'pull requests'}` : `${prs.length} of ${total} pull requests`}
            {` · ${count(filter)} ${count(filter) === 1 ? 'event' : 'events'}`}
          </Text>
        </View>
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.chips}>
          {(Object.keys(filters) as Filter[]).map((f) => (
            <Pressable key={f} onPress={() => setFilter(f)} style={[styles.chip, filter === f && styles.chipSelected]}>
              <Text style={[styles.chipText, filter === f && styles.chipTextSelected]}>{filterLabel[f]}</Text>
              <Text style={styles.chipCount}>{count(f)}</Text>
            </Pressable>
          ))}
        </ScrollView>
      </View>
      {prs.length === 0 ? (
        <Text style={styles.empty}>Nothing of this kind happened on an open pull request.</Text>
      ) : (
        prs.map(({ pr, events }) => (
          <PullRequestActivity key={pr.url} pr={pr} events={events} onOpen={() => onOpenCluster(pr.url)} />
        ))
      )}
    </View>
  );
}

// When the PR last had activity: its last event, or its opening.
function latest(pr: PullRequest): number {
  return pr.events[pr.events.length - 1]?.at ?? pr.createdAt ?? 0;
}

// One PR: its header, then its timeline, opening with the PR itself.
function PullRequestActivity({ pr, events, onOpen }: { pr: PullRequest; events: EventShape[]; onOpen: () => void }) {
  const phone = usePhone();
  return (
    <View style={styles.section}>
      <View style={[styles.prHeader, phone && styles.prHeaderPhone]}>
        <View style={styles.prTitleBlock}>
          <View style={styles.prLine}>
            <View style={[styles.state, pr.merging && styles.stateMerging]}>
              <GitPullRequestArrow size={12} color="#fff" strokeWidth={2.25} />
              <Text style={styles.stateText}>{pr.merging ? 'Merging' : 'Open'}</Text>
            </View>
            <Pressable style={styles.refChip} onPress={() => Linking.openURL(pr.url)}>
              <ChecksIcon pr={pr} size={12} />
              <Text style={styles.refText}>{ref(pr)}</Text>
            </Pressable>
            {pr.approved && (
              <View style={styles.fact}>
                <Icon icon={Check} size={12} color={t3.addition} />
                <Text style={[styles.factText, { color: t3.addition }]}>Approved</Text>
              </View>
            )}
          </View>
          <Pressable onPress={onOpen}>
            <Text style={styles.prTitle}>{pr.title}</Text>
          </Pressable>
          <Text style={styles.meta}>
            {pr.createdAt !== undefined && `opened ${timeAgo(new Date(pr.createdAt))} `}into <Mono>{pr.base}</Mono>
            {pr.events.length > 0 && ` · last activity ${timeAgo(new Date(latest(pr)))}`}
          </Text>
        </View>
        <Button label="Open" variant="ghost" onPress={onOpen} />
      </View>
      <Timeline>
        <Event icon={GitPullRequest} tone="#8FA8F5">
          <Strong>{ref(pr)}</Strong> opened into <Mono>{pr.base}</Mono>
          <When at={pr.createdAt} />
        </Event>
        {events.map((event, i) => (
          <ActivityEvent key={`${event.url}#${i}`} event={event} />
        ))}
      </Timeline>
    </View>
  );
}

// GitHub's badge and words for each kind of event.
const kinds: Record<EventKind, { icon: LucideIcon; tone?: string; says: (event: EventShape) => ReactNode }> = {
  unspecified: { icon: GitPullRequest, says: () => 'did something' },
  committed: {
    icon: GitCommitHorizontal,
    says: (event) => {
      const [oid, ...headline] = event.detail.split(' ');
      return (
        <>
          pushed <Mono>{oid}</Mono> {headline.join(' ')}
        </>
      );
    },
  },
  forcePushed: {
    icon: GitCommitHorizontal,
    tone: t3.warning,
    says: (event) => (
      <>
        force-pushed to <Mono>{event.detail}</Mono>
      </>
    ),
  },
  commented: { icon: MessageSquare, says: () => 'commented' },
  reviewed: {
    icon: Eye,
    says: (event) =>
      event.comments.length > 0
        ? `reviewed · ${event.comments.length} ${event.comments.length === 1 ? 'comment' : 'comments'} on the diff`
        : 'reviewed',
  },
  approved: { icon: Check, tone: t3.addition, says: () => <Text style={{ color: t3.addition }}>approved these changes</Text> },
  changesRequested: {
    icon: CircleX,
    tone: t3.deletion,
    says: () => <Text style={{ color: t3.deletion }}>requested changes</Text>,
  },
  reviewRequested: {
    icon: Eye,
    says: (event) => (
      <>
        requested a review from <Strong>{event.detail}</Strong>
      </>
    ),
  },
  reviewDismissed: {
    icon: EyeOff,
    tone: t3.warning,
    says: (event) => (
      <>
        dismissed a review{event.detail !== '' && <Dim> · {event.detail}</Dim>}
      </>
    ),
  },
  readyForReview: { icon: GitPullRequest, tone: t3.addition, says: () => 'marked this pull request as ready for review' },
  convertedToDraft: { icon: GitPullRequestDraft, says: () => 'converted this to a draft' },
  baseChanged: {
    icon: GitBranch,
    says: (event) => {
      const [from, to] = event.detail.split(' ');
      return (
        <>
          changed the base branch from <Mono>{from}</Mono> to <Mono>{to}</Mono>
        </>
      );
    },
  },
  closed: { icon: GitPullRequestClosed, tone: t3.deletion, says: () => 'closed this' },
  reopened: { icon: GitPullRequest, tone: t3.addition, says: () => 'reopened this' },
  merged: { icon: GitMerge, tone: merged, says: () => <Text style={{ color: merged }}>merged this</Text> },
};

// A comment's or review's body hangs under its line as GitHub boxes it, and a review's comments on
// lines under that, each with the line it is on.
function ActivityEvent({ event }: { event: EventShape }) {
  const kind = kinds[event.kind];
  const open = event.url !== '' ? () => Linking.openURL(event.url) : undefined;
  const body = (event.bodyHtml !== '' || event.comments.length > 0) && (
    <>
      {event.bodyHtml !== '' && (
        <Card
          head={
            <Pressable onPress={open} style={styles.cardHeadRow}>
              <Strong>{event.actor}</Strong>
              <Dim>{event.kind === 'commented' ? 'commented' : 'reviewed'}</Dim>
              <When at={event.at} />
            </Pressable>
          }
        >
          <Clamp max={160}>
            <Html html={event.bodyHtml} />
          </Clamp>
        </Card>
      )}
      {event.comments.map((comment) => (
        <Card
          key={comment.id}
          head={
            <Pressable onPress={() => Linking.openURL(comment.url)} style={styles.cardHeadRow}>
              <Strong>{comment.author}</Strong>
              <Dim>on</Dim>
              <Text style={styles.path}>
                {comment.path}
                {comment.line > 0 && `:${comment.line}`}
              </Text>
            </Pressable>
          }
        >
          <Clamp max={160}>
            <Html html={comment.bodyHtml} />
          </Clamp>
        </Card>
      ))}
    </>
  );
  return (
    <Event
      icon={kind.icon}
      tone={kind.tone}
      body={body}
      trailing={open && !body ? <Button label="View" variant="ghost" onPress={open} /> : undefined}
    >
      <Strong>{event.actor || 'someone'}</Strong> {kind.says(event)}
      <When at={event.at} />
    </Event>
  );
}

const styles = StyleSheet.create({
  page: {
    width: '100%',
    maxWidth: 1240,
    alignSelf: 'center',
    gap: 24,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    justifyContent: 'space-between',
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
    lineHeight: 18,
  },
  chips: {
    flexDirection: 'row',
    gap: 6,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    height: 28,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.border,
  },
  chipSelected: {
    backgroundColor: t3.input,
    borderColor: 'rgba(255, 255, 255, 0.14)',
  },
  chipText: {
    color: t3.muted,
    fontSize: 12,
    fontWeight: '600',
  },
  chipTextSelected: {
    color: t3.foreground,
  },
  chipCount: {
    color: t3.faint,
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  empty: {
    color: t3.faint,
    fontSize: 13,
    paddingVertical: 24,
    textAlign: 'center',
  },
  section: {
    gap: 16,
  },
  prHeader: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 12,
    padding: 14,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  prHeaderPhone: {
    padding: 12,
  },
  prTitleBlock: {
    flex: 1,
    minWidth: 0,
    gap: 6,
  },
  prLine: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 6,
  },
  prTitle: {
    color: t3.foreground,
    fontSize: 16,
    fontWeight: '600',
    lineHeight: 22,
  },
  state: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 10,
    height: 24,
    borderRadius: 999,
    backgroundColor: '#238636',
  },
  stateMerging: {
    backgroundColor: '#8957E5',
  },
  stateText: {
    color: '#fff',
    fontSize: 12,
    fontWeight: '600',
  },
  refChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 8,
    height: 24,
    borderRadius: 999,
    backgroundColor: t3.input,
  },
  refText: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  fact: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  factText: {
    fontSize: 12,
  },
  cardHeadRow: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 6,
  },
  path: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
});
