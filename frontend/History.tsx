import { timestampMs } from '@bufbuild/protobuf/wkt';
import type { LucideIcon } from 'lucide-react';
import {
  Check,
  CircleCheck,
  CircleDashed,
  CircleDot,
  CircleX,
  Eye,
  FilePen,
  GitBranch,
  GitCommitHorizontal,
  GitMerge,
  MessageSquare,
  RefreshCw,
  UserPlus,
} from 'lucide-react';
import { useEffect, useState } from 'react';
import { ActivityIndicator, Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { Card, Event, Mono, Rail, ref, Strong, When } from './ClusterTimeline';
import type { PullRequestEvent, PullRequestHistory } from './gen/gem_pb';
import { ChecksIcon } from './PullRequestList';
import { Html } from './surfaces';
import { Icon, t3, usePhone } from './t3';
import type { Cluster } from './view-types';

// Every open PR's history on GitHub, cluster by cluster as the inbox orders them: each push and
// the checks on it, review, comment, review request, and change of state or base, oldest first.
// GitHub is asked again whenever a cluster updates; what was shown stays until it answers.
export function History({
  clusters,
  load,
}: {
  clusters: readonly Cluster[];
  load: (urls: string[]) => Promise<PullRequestHistory[]>;
}) {
  const phone = usePhone();
  const prs = clusters.flatMap((cluster) => cluster.pullRequests);
  // Changes when a cluster updates or its PRs change, and only then.
  const version = JSON.stringify(clusters.map((c) => [c.updatedAt, c.pullRequests.map((pr) => pr.url)]));
  const [histories, setHistories] = useState<ReadonlyMap<string, PullRequestHistory>>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let current = true;
    const urls = (JSON.parse(version) as [number, string[]][]).flatMap(([, urls]) => urls);
    load(urls).then(
      (loaded) => {
        if (!current) return;
        setHistories(new Map(loaded.map((h) => [h.url, h])));
        setError(undefined);
      },
      (e: Error) => current && setError(e.message),
    );
    return () => {
      current = false;
    };
  }, [version, load]);

  return (
    <View style={styles.page}>
      <View style={[styles.header, phone && styles.headerPhone]}>
        <Text style={[styles.title, phone && styles.titlePhone]}>History</Text>
        <Text style={styles.meta}>
          {prs.length} open {prs.length === 1 ? 'pull request' : 'pull requests'} · every push, review, comment
          and change of state
        </Text>
        {error && <Text style={styles.error}>GitHub gave no history: {error}</Text>}
      </View>
      {prs.map((pr) => {
        const history = histories?.get(pr.url);
        return (
          <View key={pr.url} style={styles.section}>
            <Pressable style={styles.prHead} onPress={() => Linking.openURL(pr.url)}>
              <ChecksIcon pr={pr} />
              <Text style={styles.prRef}>{ref(pr)}</Text>
              <Text numberOfLines={1} style={styles.prTitle}>
                {pr.title}
              </Text>
            </Pressable>
            {!history ? (
              <ActivityIndicator size="small" color={t3.muted} style={styles.loading} />
            ) : history.error ? (
              <Text style={styles.error}>{history.error}</Text>
            ) : history.events.length === 0 ? (
              <Text style={styles.meta}>Nothing yet.</Text>
            ) : (
              <Rail>
                {history.events.map((event, i) => (
                  <HistoryEvent key={`${event.url}${i}`} event={event} />
                ))}
              </Rail>
            )}
          </View>
        );
      })}
    </View>
  );
}

// A commit's checks, as GitHub rolls them up.
function ChecksState({ state }: { state: string }) {
  if (!state) return null;
  const [icon, color]: [LucideIcon, string] =
    state === 'SUCCESS'
      ? [CircleCheck, t3.addition]
      : state === 'FAILURE' || state === 'ERROR'
        ? [CircleX, t3.deletion]
        : [CircleDashed, t3.warning];
  return <Icon icon={icon} size={14} color={color} />;
}

const reviewed: Record<string, [LucideIcon, string, string]> = {
  APPROVED: [Check, t3.addition, 'approved'],
  CHANGES_REQUESTED: [CircleX, t3.deletion, 'requested changes'],
  DISMISSED: [Eye, t3.faint, 'reviewed · dismissed'],
};

const changed: Record<string, [LucideIcon, string | undefined, string]> = {
  ReadyForReviewEvent: [Eye, '#8FA8F5', 'marked it ready for review'],
  ConvertToDraftEvent: [FilePen, undefined, 'marked it as a draft'],
  ClosedEvent: [CircleX, t3.deletion, 'closed it'],
  ReopenedEvent: [CircleDot, t3.addition, 'reopened it'],
  MergedEvent: [GitMerge, '#A371F7', 'merged it'],
  AutoMergeEnabledEvent: [GitMerge, undefined, 'enabled auto-merge'],
  AutoMergeDisabledEvent: [GitMerge, undefined, 'disabled auto-merge'],
};

function HistoryEvent({ event }: { event: PullRequestEvent }) {
  const at = event.at && timestampMs(event.at);
  const who = <Strong>{event.actor}</Strong>;
  const open = event.url ? () => Linking.openURL(event.url) : undefined;
  switch (event.kind) {
    case 'PullRequestCommit':
      return (
        <Event icon={GitCommitHorizontal} trailing={<ChecksState state={event.state} />}>
          {who} committed{' '}
          <Text onPress={open}>
            <Mono>{event.oid}</Mono>
          </Text>{' '}
          {event.text}
          <When at={at} />
        </Event>
      );
    case 'HeadRefForcePushedEvent':
      return (
        <Event icon={RefreshCw} tone={t3.warning} trailing={<ChecksState state={event.state} />}>
          {who} force-pushed to <Mono>{event.oid}</Mono>
          <When at={at} />
        </Event>
      );
    case 'PullRequestReview': {
      const [icon, tone, verb] = reviewed[event.state] ?? [MessageSquare, undefined, 'reviewed'];
      const comments = event.comments.length;
      return (
        <Event
          icon={icon}
          tone={tone}
          body={
            (event.bodyHtml || comments > 0) && (
              <>
                {event.bodyHtml && (
                  <Card title={event.actor} meta={verb}>
                    <Html html={event.bodyHtml} onPress={open} />
                  </Card>
                )}
                {event.comments.map((comment) => (
                  <Card key={comment.url} title={<Mono>{comment.path}</Mono>}>
                    <Html html={comment.bodyHtml} onPress={() => Linking.openURL(comment.url)} />
                  </Card>
                ))}
              </>
            )
          }
        >
          {who} {verb}
          {comments > 0 && ` with ${comments} ${comments === 1 ? 'comment' : 'comments'}`}
          <When at={at} />
        </Event>
      );
    }
    case 'IssueComment':
      return (
        <Event
          icon={MessageSquare}
          body={
            <Card title={event.actor} meta="commented">
              <Html html={event.bodyHtml} onPress={open} />
            </Card>
          }
        >
          {who} commented
          <When at={at} />
        </Event>
      );
    case 'ReviewRequestedEvent':
      return (
        <Event icon={UserPlus}>
          {who} requested a review from <Strong>{event.text}</Strong>
          <When at={at} />
        </Event>
      );
    case 'ReviewDismissedEvent':
      return (
        <Event icon={Eye} tone={t3.faint}>
          {who} dismissed <Strong>{event.text}</Strong>'s review
          <When at={at} />
        </Event>
      );
    case 'BaseRefChangedEvent':
      return (
        <Event icon={GitBranch}>
          {who} changed the base <Mono>{event.text}</Mono>
          <When at={at} />
        </Event>
      );
    default: {
      const [icon, tone, verb] = changed[event.kind] ?? [CircleDot, undefined, event.kind];
      return (
        <Event icon={icon} tone={tone}>
          {who} {verb}
          <When at={at} />
        </Event>
      );
    }
  }
}

const styles = StyleSheet.create({
  page: {
    width: '100%',
    maxWidth: 960,
    alignSelf: 'center',
    gap: 24,
  },
  header: {
    gap: 6,
    paddingBottom: 16,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  headerPhone: {
    paddingBottom: 12,
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
  error: {
    color: t3.deletion,
    fontSize: 12,
  },
  section: {
    gap: 12,
  },
  prHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  prRef: {
    color: '#8FA8F5',
    fontFamily: t3.mono,
    fontSize: 13,
  },
  prTitle: {
    flex: 1,
    color: t3.foreground,
    fontSize: 15,
    fontWeight: '600',
  },
  loading: {
    alignSelf: 'flex-start',
  },
});
