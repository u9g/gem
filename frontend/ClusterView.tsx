import { timestampDate } from '@bufbuild/protobuf/wkt';
import { StyleSheet, Text, View } from 'react-native';

import { units } from './ClusterRow';
import { MergeBy, type PullRequestCluster } from './gen/gem_pb';
import { bySubject, ProposalGroup } from './ProposalView';
import { PullRequestList } from './PullRequestList';
import { Button, t3, usePhone } from './t3';

const relativeTime = new Intl.RelativeTimeFormat('en', { numeric: 'always' });

// Largest unit that fits, truncated: 59 minutes stays "59 minutes ago", not "1 hour ago".
export function timeAgo(date: Date): string {
  const seconds = (date.getTime() - Date.now()) / 1000;
  const [unit, size] = units.find(([, size]) => Math.abs(seconds) >= size) ?? units[units.length - 1];
  return relativeTime.format(Math.trunc(seconds / size), unit);
}

// A cluster's title, a button to rebase it, its PRs, a button to merge them once the server says they can (forcing the
// unapproved ones in as an admin, said plainly, when that is how), the
// conversations waiting on u9g. The phone docks the box to ask an agent about it below.
export function ClusterView({
  cluster,
  busy,
  onApprove,
  onDismiss,
  onRestart,
  onRebase,
  onRebaseCluster,
  onMerge,
  onInvestigate,
}: {
  cluster: PullRequestCluster;
  busy?: ReadonlySet<string>;
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  onRestart: (cluster: PullRequestCluster) => void;
  onRebase: (url: string) => void;
  onRebaseCluster: (cluster: PullRequestCluster) => void;
  // `admin`: the user chose to force the unapproved pull requests in as an admin.
  onMerge: (cluster: PullRequestCluster, admin: boolean) => void;
  onInvestigate: (id: string) => void;
}) {
  const phone = usePhone();
  const restarting = busy?.has(cluster.pullRequests[0].url);
  const merging = busy?.has(`merge ${cluster.pullRequests[0].url}`);
  const admin = cluster.mergeBy === MergeBy.ADMIN;
  return (
    <View style={styles.column}>
      <View style={styles.header}>
        <View style={styles.titleBlock}>
          <Text style={[styles.title, phone && styles.phoneTitle, cluster.mergeBy === MergeBy.APPROVAL && styles.ready, admin && styles.forceable]}>{cluster.name}</Text>
          <Text style={styles.meta}>
            {cluster.updatedAt && timeAgo(timestampDate(cluster.updatedAt))}
          </Text>
        </View>
        <RebaseClusterButton cluster={cluster} busy={busy} onRebaseCluster={onRebaseCluster} />
        <Button
          label={restarting ? 'Restarting…' : 'Restart investigation'}
          disabled={restarting || merging}
          onPress={() => onRestart(cluster)}
        />
      </View>
      <View style={styles.pullRequests}>
        <PullRequestList pullRequests={cluster.pullRequests} busy={busy} onRebase={onRebase} />
      </View>
      <MergeBanner cluster={cluster} busy={busy} onMerge={onMerge} />
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
    </View>
  );
}

// Starts one agent rebasing every PR of the cluster, shown while one of them waits on a rebase or is
// being rebased.
export function RebaseClusterButton({
  cluster,
  busy,
  onRebaseCluster,
}: {
  cluster: PullRequestCluster;
  busy?: ReadonlySet<string>;
  onRebaseCluster: (cluster: PullRequestCluster) => void;
}) {
  const starting = busy?.has(`rebase ${cluster.pullRequests[0].url}`);
  if (!cluster.pullRequests.some((pr) => pr.behind || pr.rebase)) return null;
  return (
    <Button
      label={starting ? 'Starting an agent…' : 'Rebase cluster'}
      disabled={starting || cluster.pullRequests.some((pr) => pr.rebase || busy?.has(pr.url))}
      onPress={() => onRebaseCluster(cluster)}
    />
  );
}

// A button to merge the cluster once the server says it can, forcing the unapproved PRs in as an
// admin, said plainly, when that is how. `stacked` puts the button under the text, as on a phone.
export function MergeBanner({
  cluster,
  busy,
  onMerge,
  stacked = false,
}: {
  cluster: PullRequestCluster;
  busy?: ReadonlySet<string>;
  onMerge: (cluster: PullRequestCluster, admin: boolean) => void;
  stacked?: boolean;
}) {
  const phone = usePhone();
  stacked ||= phone;
  const restarting = busy?.has(cluster.pullRequests[0].url);
  const merging = busy?.has(`merge ${cluster.pullRequests[0].url}`);
  const admin = cluster.mergeBy === MergeBy.ADMIN;
  const forced = cluster.pullRequests.filter((pr) => pr.mergeBy === MergeBy.ADMIN).length;
  const one = forced === 1;
  if (cluster.mergeBy === MergeBy.UNSPECIFIED || cluster.pullRequests.some((pr) => pr.merge)) return null;
  return (
    <View style={[styles.merge, admin && styles.adminMerge, stacked && styles.mergeStacked]}>
      <Text style={[styles.mergeText, stacked && styles.mergeTextStacked]}>
        {admin
          ? `${forced} of ${cluster.pullRequests.length} pull requests ${one ? 'is' : 'are'} not approved. Every check passes, and you are an admin of ${one ? 'its repository' : 'their repositories'}, so you can force ${one ? 'it' : 'them'} in past review.`
          : 'Every pull request is approved and its checks pass.'}
      </Text>
      <Button
        label={merging ? 'Starting an agent…' : admin ? 'Force-merge with agent' : 'Merge with agent'}
        variant="default"
        large={phone}
        disabled={restarting || merging}
        onPress={() => onMerge(cluster, admin)}
      />
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
  forceable: {
    color: t3.warning,
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
  adminMerge: {
    borderColor: 'rgba(245, 158, 11, 0.35)',
    backgroundColor: 'rgba(245, 158, 11, 0.06)',
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
