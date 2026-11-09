import type { LucideIcon } from 'lucide-react';
import {
  Check,
  CircleCheck,
  CircleDot,
  CircleSlash,
  CircleX,
  Eye,
  EyeOff,
  GitBranch,
  GitCommitHorizontal,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestDraft,
  MessageSquare,
  Pencil,
  Repeat2,
} from 'lucide-react';
import { type ReactNode, useContext, useState } from 'react';
import { Image, Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { timeAgo } from './ClusterView';
import { ChecksIcon } from './PullRequestList';
import { Html, Login } from './surfaces';
import { Icon, t3, usePhone } from './t3';
import type { Cluster, Event, PullRequest } from './view-types';

const link = '#8FA8F5';

// Every open PR with everything that happened on it, each laid out as GitHub's Conversation tab:
// the PR's title, state and checks on top, then one timeline of its commits, comments, reviews and
// state changes, oldest first, with a card for anything that was said. PRs with the latest
// activity come first.
export function Activity({ clusters }: { clusters: readonly Cluster[] }) {
  const phone = usePhone();
  const prs = clusters
    .flatMap((cluster) => cluster.pullRequests)
    .sort((a, b) => lastAt(b) - lastAt(a));
  const events = prs.reduce((n, pr) => n + pr.events.length, 0);
  return (
    <View style={styles.page}>
      <View style={styles.pageHead}>
        <Text style={[styles.pageTitle, phone && styles.pageTitlePhone]}>Activity</Text>
        <Text style={styles.pageMeta}>
          {prs.length} {prs.length === 1 ? 'pull request' : 'pull requests'} · {events} {events === 1 ? 'event' : 'events'}
        </Text>
      </View>
      {prs.map((pr) => (
        <PullRequestActivity key={pr.url} pr={pr} />
      ))}
    </View>
  );
}

const lastAt = (pr: PullRequest) => pr.events.at(-1)?.at ?? pr.createdAt ?? 0;

const ref = (pr: PullRequest) => `${pr.repository?.owner}/${pr.repository?.name}#${pr.number}`;

function PullRequestActivity({ pr }: { pr: PullRequest }) {
  const phone = usePhone();
  const login = useContext(Login);
  const checks = pr.checks;
  const total = checks?.total ?? 0;
  const failed = checks?.failed ?? 0;
  const done = checks?.done ?? 0;
  return (
    <View style={styles.pr}>
      <View style={[styles.prHead, phone && styles.prHeadPhone]}>
        <View style={styles.prTitleBlock}>
          <Pressable onPress={() => Linking.openURL(pr.url)}>
            <Text style={[styles.prTitle, phone && styles.prTitlePhone]}>
              {pr.title} <Text style={styles.prNumber}>#{pr.number}</Text>
            </Text>
          </Pressable>
          <View style={styles.prMeta}>
            <View style={[styles.state, pr.merging && styles.stateMerging]}>
              <GitPullRequestArrow size={12} color="#fff" strokeWidth={2.25} />
              <Text style={styles.stateText}>{pr.merging ? 'Merging' : 'Open'}</Text>
            </View>
            <Text style={styles.metaText}>
              <Text style={styles.metaStrong}>{ref(pr)}</Text> · into <Mono>{pr.base}</Mono>
              {pr.createdAt !== undefined && ` · opened ${timeAgo(new Date(pr.createdAt))}`}
            </Text>
          </View>
        </View>
        <View style={styles.facts}>
          <View style={styles.fact}>
            <ChecksIcon pr={pr} size={14} />
            <Text style={styles.factText}>
              {total === 0 ? 'No checks' : failed > 0 ? `${failed} of ${total} checks failed` : done < total ? `${done} of ${total} checks done` : `All ${total} checks passed`}
            </Text>
          </View>
          <View style={styles.fact}>
            <Icon icon={pr.approved ? Check : CircleDot} size={14} color={pr.approved ? t3.addition : t3.faint} />
            <Text style={styles.factText}>{pr.approved ? 'Approved' : 'Review required'}</Text>
          </View>
        </View>
      </View>
      <View style={styles.timeline}>
        <View style={styles.rail} />
        <Row icon={GitPullRequest} tone={link} at={pr.createdAt} actor={login}>
          <Strong>{login}</Strong> opened this pull request
        </Row>
        {group(pr.events).map((item, i) =>
          Array.isArray(item) ? <Commits key={`commits ${i}`} commits={item} /> : <EventRow key={item.id} event={item} />,
        )}
      </View>
    </View>
  );
}

// Commits in a row by one author are one item, as GitHub lists them under "added N commits".
function group(events: readonly Event[]): (Event | Event[])[] {
  const items: (Event | Event[])[] = [];
  for (const event of events) {
    const last = items.at(-1);
    if (event.kind === 'commit' && Array.isArray(last) && last[0].actor === event.actor) last.push(event);
    else items.push(event.kind === 'commit' ? [event] : event);
  }
  return items;
}

function Commits({ commits }: { commits: Event[] }) {
  const n = commits.length;
  const list = (
    <View style={styles.commits}>
      {commits.map((commit) => (
        <Pressable key={commit.id} style={styles.commit} onPress={() => Linking.openURL(commit.url)}>
          <GitCommitHorizontal size={14} color={t3.faint} />
          <Text numberOfLines={1} style={styles.commitMessage}>
            {commit.body}
          </Text>
          <Text style={styles.sha}>{commit.detail}</Text>
        </Pressable>
      ))}
    </View>
  );
  return (
    <Row icon={GitCommitHorizontal} actor={commits[0].actor} at={commits[n - 1].at} body={list}>
      <Strong>{commits[0].actor}</Strong> added {n} {n === 1 ? 'commit' : 'commits'}
    </Row>
  );
}

// GitHub's wording and colour for each event: a line on the rail, and under it a card for
// anything that was said.
function EventRow({ event }: { event: Event }) {
  const who = <Strong>{event.actor}</Strong>;
  // What a review or dismissal said, under the line that already says who and when.
  const card = event.bodyHtml !== '' && <Card event={event} headless />;
  switch (event.kind) {
    case 'comment':
      return (
        <View style={styles.cardRow}>
          <Avatar login={event.actor} size={28} />
          <Card event={event} />
        </View>
      );
    case 'review':
      return event.detail === 'APPROVED' ? (
        <Row icon={CircleCheck} tone={t3.addition} fill actor={event.actor} at={event.at} body={card}>
          {who} approved these changes
        </Row>
      ) : event.detail === 'CHANGES_REQUESTED' ? (
        <Row icon={CircleX} tone={t3.deletion} fill actor={event.actor} at={event.at} body={card}>
          {who} requested changes
        </Row>
      ) : event.detail === 'DISMISSED' ? (
        <Row icon={CircleSlash} actor={event.actor} at={event.at} body={card}>
          {who} left a review that was later dismissed
        </Row>
      ) : (
        <Row icon={Eye} actor={event.actor} at={event.at} body={card}>
          {who} reviewed
        </Row>
      );
    case 'reviewComment':
      return (
        <View style={styles.cardRow}>
          <Avatar login={event.actor} size={28} />
          <Card event={event} path={event.detail} />
        </View>
      );
    case 'commit':
      return <Commits commits={[event]} />;
    case 'forcePush':
      return (
        <Row icon={Repeat2} actor={event.actor} at={event.at}>
          {who} force-pushed to <Mono>{event.detail}</Mono>
        </Row>
      );
    case 'readyForReview':
      return (
        <Row icon={GitPullRequest} tone={t3.addition} fill actor={event.actor} at={event.at}>
          {who} marked this pull request as ready for review
        </Row>
      );
    case 'draft':
      return (
        <Row icon={GitPullRequestDraft} actor={event.actor} at={event.at}>
          {who} marked this pull request as draft
        </Row>
      );
    case 'reviewRequested':
      return (
        <Row icon={Eye} actor={event.actor} at={event.at}>
          {who} requested a review from <Strong>{event.detail}</Strong>
        </Row>
      );
    case 'reviewDismissed':
      return (
        <Row icon={EyeOff} actor={event.actor} at={event.at} body={card}>
          {who} dismissed a review
        </Row>
      );
    case 'retargeted':
      return (
        <Row icon={GitBranch} actor={event.actor} at={event.at}>
          {who} changed the base branch to <Mono>{event.detail}</Mono>
        </Row>
      );
    case 'renamed':
      return (
        <Row icon={Pencil} actor={event.actor} at={event.at}>
          {who} changed the title to <Text style={styles.titleText}>{event.detail}</Text>
        </Row>
      );
    case 'closed':
      return (
        <Row icon={CircleSlash} tone={t3.deletion} fill actor={event.actor} at={event.at}>
          {who} closed this
        </Row>
      );
    case 'reopened':
      return (
        <Row icon={CircleDot} tone={t3.addition} fill actor={event.actor} at={event.at}>
          {who} reopened this
        </Row>
      );
  }
}

// One line of the timeline: a badge on the rail, the actor's avatar, what happened and when;
// `body` hangs under it beside the rail. A filled badge, as GitHub gives approvals and closes, is
// white on its tone.
function Row({
  icon,
  tone,
  fill = false,
  actor,
  at,
  children,
  body,
}: {
  icon: LucideIcon;
  tone?: string;
  fill?: boolean;
  actor: string;
  at?: number;
  children: ReactNode;
  body?: ReactNode;
}) {
  return (
    <View style={styles.event}>
      <View style={styles.eventLine}>
        <View style={[styles.badge, tone !== undefined && (fill ? { backgroundColor: tone, borderColor: tone } : { borderColor: tone })]}>
          <Icon icon={icon} size={14} color={fill ? '#fff' : (tone ?? t3.muted)} />
        </View>
        <Avatar login={actor} size={20} />
        <Text style={styles.eventText}>
          {children}
          {at !== undefined && <Text style={styles.when}> · {timeAgo(new Date(at))}</Text>}
        </Text>
      </View>
      {body && <View style={styles.eventBody}>{body}</View>}
    </View>
  );
}

// A comment as GitHub boxes it: who said it and when in a header bar, and the file it is on for a
// review comment, then the body as GitHub renders it.
function Card({ event, path, headless = false }: { event: Event; path?: string; headless?: boolean }) {
  return (
    <View style={styles.card}>
      {!headless && (
        <Pressable style={styles.cardHead} onPress={() => event.url && Linking.openURL(event.url)}>
          <Text style={styles.cardHeadText}>
            <Strong>{event.actor}</Strong> commented
            {at(event)}
          </Text>
          {path !== undefined && (
            <Text numberOfLines={1} style={styles.path}>
              {path}
            </Text>
          )}
        </Pressable>
      )}
      <View style={styles.cardBody}>
        <Html html={event.bodyHtml} />
      </View>
    </View>
  );
}

const at = (event: Event) => event.at !== undefined && <Text style={styles.when}> · {timeAgo(new Date(event.at))}</Text>;

// GitHub's avatar for a login; its first letter when GitHub has none, as for some bots.
function Avatar({ login, size }: { login: string; size: number }) {
  const [failed, setFailed] = useState(false);
  const round = { width: size, height: size, borderRadius: size / 2 };
  if (failed || login === '') {
    return (
      <View style={[styles.avatarFallback, round]}>
        <Text style={[styles.avatarLetter, { fontSize: size / 2 }]}>{login.slice(0, 1).toUpperCase()}</Text>
      </View>
    );
  }
  return <Image source={{ uri: `https://github.com/${login}.png?size=${size * 2}` }} style={round} onError={() => setFailed(true)} />;
}

const Strong = ({ children }: { children: ReactNode }) => <Text style={styles.strong}>{children}</Text>;
const Mono = ({ children }: { children: ReactNode }) => <Text style={styles.mono}>{children}</Text>;

const BADGE = 28;

const styles = StyleSheet.create({
  page: {
    width: '100%',
    maxWidth: 960,
    alignSelf: 'center',
    gap: 28,
  },
  pageHead: {
    gap: 4,
    paddingBottom: 16,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  pageTitle: {
    color: t3.foreground,
    fontSize: 22,
    fontWeight: '600',
    lineHeight: 28,
  },
  pageTitlePhone: {
    fontSize: 20,
    lineHeight: 26,
  },
  pageMeta: {
    color: t3.faint,
    fontSize: 12,
  },
  pr: {
    gap: 16,
  },
  prHead: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 16,
  },
  prHeadPhone: {
    flexDirection: 'column',
    gap: 10,
  },
  prTitleBlock: {
    flex: 1,
    gap: 8,
  },
  prTitle: {
    color: t3.foreground,
    fontSize: 18,
    fontWeight: '600',
    lineHeight: 24,
  },
  prTitlePhone: {
    fontSize: 16,
    lineHeight: 22,
  },
  prNumber: {
    color: t3.faint,
    fontWeight: '400',
  },
  prMeta: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 8,
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
  metaText: {
    color: t3.muted,
    fontSize: 13,
  },
  metaStrong: {
    color: t3.foreground,
    fontWeight: '600',
  },
  facts: {
    gap: 4,
    alignItems: 'flex-start',
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
  timeline: {
    gap: 14,
  },
  // Runs through the badges' centers, under them.
  rail: {
    position: 'absolute',
    top: 8,
    bottom: 8,
    left: BADGE / 2 - 1,
    width: 2,
    backgroundColor: t3.border,
  },
  event: {
    gap: 10,
  },
  eventLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    minHeight: BADGE,
  },
  eventText: {
    flex: 1,
    color: t3.muted,
    fontSize: 13,
    lineHeight: 19,
    paddingTop: 4,
  },
  eventBody: {
    marginLeft: BADGE + 8,
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
  commits: {
    gap: 4,
  },
  commit: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  commitMessage: {
    flex: 1,
    color: t3.foreground,
    fontSize: 13,
  },
  sha: {
    color: link,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  cardRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 8,
  },
  card: {
    flex: 1,
    minWidth: 0,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    overflow: 'hidden',
  },
  cardHead: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  cardHeadText: {
    color: t3.muted,
    fontSize: 13,
  },
  path: {
    flexShrink: 1,
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  cardBody: {
    padding: 12,
  },
  avatarFallback: {
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: t3.input,
  },
  avatarLetter: {
    color: t3.foreground,
    fontWeight: '600',
  },
  strong: {
    color: t3.foreground,
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
  titleText: {
    color: t3.foreground,
  },
  when: {
    color: t3.faint,
  },
});
