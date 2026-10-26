import { timestampDate } from '@bufbuild/protobuf/wkt';
import { StyleSheet, Text, View } from 'react-native';

import { units } from './ClusterRow';
import type { PullRequestCluster } from './gen/gem_pb';
import { Composer } from './InvestigationView';
import { bySubject, ProposalGroup } from './ProposalView';
import { PullRequestList } from './PullRequestList';
import { Button, usePhone } from './t3';

const relativeTime = new Intl.RelativeTimeFormat('en', { numeric: 'always' });

// Largest unit that fits, truncated: 59 minutes stays "59 minutes ago", not "1 hour ago".
export function timeAgo(date: Date): string {
  const seconds = (date.getTime() - Date.now()) / 1000;
  const [unit, size] = units.find(([, size]) => Math.abs(seconds) >= size) ?? units[units.length - 1];
  return relativeTime.format(Math.trunc(seconds / size), unit);
}

// A cluster's title, its PRs, a button to merge them once every one is approved, or the user
// administers its repository, and passing, the
// conversations waiting on u9g, and a box to ask an agent about it unless the phone docks that box
// below.
export function ClusterView({
  cluster,
  busy,
  onApprove,
  onDismiss,
  onRestart,
  onRebase,
  onMerge,
  onInvestigate,
  onMessage,
}: {
  cluster: PullRequestCluster;
  busy?: ReadonlySet<string>;
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  onRestart: (cluster: PullRequestCluster) => void;
  onRebase: (url: string) => void;
  onMerge: (cluster: PullRequestCluster) => void;
  onInvestigate: (id: string) => void;
  // Starts an agent on the message, which reads the cluster through gem's MCP server. Resolves
  // once the agent started.
  onMessage?: (message: string) => Promise<unknown>;
}) {
  const phone = usePhone();
  const restarting = busy?.has(cluster.pullRequests[0].url);
  const merging = busy?.has(`merge ${cluster.pullRequests[0].url}`);
  return (
    <View style={styles.column}>
      <View style={styles.header}>
        <View style={styles.titleBlock}>
          <Text style={[styles.title, phone && styles.phoneTitle, cluster.mergeable && styles.ready]}>{cluster.name}</Text>
          <Text style={styles.meta}>
            {cluster.updatedAt && timeAgo(timestampDate(cluster.updatedAt))}
          </Text>
        </View>
        <Button
          label={restarting ? 'Restarting…' : 'Restart investigation'}
          disabled={restarting || merging}
          onPress={() => onRestart(cluster)}
        />
      </View>
      <View style={styles.pullRequests}>
        <PullRequestList pullRequests={cluster.pullRequests} busy={busy} onRebase={onRebase} />
      </View>
      {cluster.mergeable && !cluster.pullRequests.some((pr) => pr.merge) && (
        <View style={[styles.merge, phone && styles.mergeStacked]}>
          <Text style={[styles.mergeText, phone && styles.mergeTextStacked]}>Every pull request is approved or yours to merge as an admin, and its checks pass.</Text>
          <Button
              label={merging ? 'Starting an agent…' : 'Merge cluster'}
              variant="default"
              large={phone}
              disabled={restarting || merging}
              onPress={() => onMerge(cluster)}
            />
        </View>
      )}
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
      {onMessage && <Composer placeholder="Ask an agent about this cluster…" onSend={onMessage} />}
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
  phoneTitle: {
    fontSize: 24,
    fontWeight: '700',
    letterSpacing: -0.3,
  },
  ready: {
    color: '#4ADE80',
  },
  merge: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    marginBottom: 4,
    padding: 12,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: 'rgba(16, 185, 129, 0.3)',
    backgroundColor: 'rgba(16, 185, 129, 0.06)',
  },
  // The text takes a line of its own, and the large button grows along the next.
  mergeStacked: {
    flexWrap: 'wrap',
  },
  mergeTextStacked: {
    width: '100%',
  },
  mergeText: {
    flexGrow: 1,
    flexShrink: 1,
    color: '#D0CFD4',
    fontSize: 13,
  },
  meta: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 12,
  },
});
