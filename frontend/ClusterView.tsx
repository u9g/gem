import { timestampDate } from '@bufbuild/protobuf/wkt';
import { StyleSheet, Text, View } from 'react-native';

import { ready, units } from './ClusterRow';
import type { PullRequestCluster } from './gen/gem_pb';
import { Composer } from './InvestigationView';
import { bySubject, ProposalGroup } from './ProposalView';
import { PullRequestList } from './PullRequestList';
import { Button } from './t3';

const relativeTime = new Intl.RelativeTimeFormat('en', { numeric: 'always' });

// Largest unit that fits, truncated: 59 minutes stays "59 minutes ago", not "1 hour ago".
export function timeAgo(date: Date): string {
  const seconds = (date.getTime() - Date.now()) / 1000;
  const [unit, size] = units.find(([, size]) => Math.abs(seconds) >= size) ?? units[units.length - 1];
  return relativeTime.format(Math.trunc(seconds / size), unit);
}

// A cluster's title, its PRs, the conversations waiting on u9g, and a box to ask an agent about
// it.
export function ClusterView({
  cluster,
  busy,
  onApprove,
  onDismiss,
  onRestart,
  onRebase,
  onInvestigate,
  onMessage,
}: {
  cluster: PullRequestCluster;
  busy?: string;
  onApprove: (id: string, reply: string, followUp: string) => void;
  onDismiss: (id: string) => void;
  onRestart: (cluster: PullRequestCluster) => void;
  onRebase: (url: string) => void;
  onInvestigate: (id: string) => void;
  // Starts an agent on the message, which reads the cluster through gem's MCP server. Resolves
  // once the agent started.
  onMessage: (message: string) => Promise<unknown>;
}) {
  return (
    <View style={styles.column}>
      <View style={styles.header}>
        <View style={styles.titleBlock}>
          <Text style={[styles.title, ready(cluster) && styles.ready]}>{cluster.name}</Text>
          <Text style={styles.meta}>
            {cluster.updatedAt && timeAgo(timestampDate(cluster.updatedAt))}
          </Text>
        </View>
        <Button
          label={busy === cluster.pullRequests[0].url ? 'Restarting…' : 'Restart investigation'}
          disabled={busy !== undefined}
          onPress={() => onRestart(cluster)}
        />
      </View>
      <View style={styles.pullRequests}>
        <PullRequestList pullRequests={cluster.pullRequests} busy={busy} onRebase={onRebase} />
      </View>
      {bySubject(cluster.pullRequests.flatMap((pr) => pr.proposals)).map((group) => (
        <ProposalGroup
          key={group[0].id}
          proposals={group}
          busy={busy}
          onApprove={onApprove}
          onDismiss={onDismiss}
          onInvestigate={onInvestigate}
        />
      ))}
      <Composer placeholder="Ask an agent about this cluster…" onSend={onMessage} />
    </View>
  );
}

const styles = StyleSheet.create({
  column: {
    width: '100%',
    maxWidth: 1240,
    gap: 12,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 12,
    marginBottom: 8,
  },
  titleBlock: {
    flex: 1,
    gap: 4,
  },
  pullRequests: {
    marginBottom: 16,
  },
  title: {
    color: '#D0CFD4',
    fontSize: 20,
    fontWeight: '600',
  },
  ready: {
    color: '#4ADE80',
  },
  meta: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 12,
  },
});
