import { timestampDate } from '@bufbuild/protobuf/wkt';
import { useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Investigation, PullRequest, PullRequestCluster } from './gen/gem_pb';
import { ChecksIcon } from './PullRequestList';

export const units: [Intl.RelativeTimeFormatUnit, number][] = [
  ['year', 365 * 24 * 60 * 60],
  ['month', 30 * 24 * 60 * 60],
  ['week', 7 * 24 * 60 * 60],
  ['day', 24 * 60 * 60],
  ['hour', 60 * 60],
  ['minute', 60],
  ['second', 1],
];

// "5m", "3h", "2d": the largest unit that fits, truncated, for one-line rows.
function shortAgo(date: Date): string {
  const seconds = (Date.now() - date.getTime()) / 1000;
  const [unit, size] = units.find(([, size]) => seconds >= size) ?? units[units.length - 1];
  return `${Math.trunc(seconds / size)}${unit === 'month' ? 'mo' : unit[0]}`;
}

export function ready(cluster: PullRequestCluster): boolean {
  return cluster.pullRequests.every((pr) => pr.approved && pr.checksPassing);
}

function pending(cluster: PullRequestCluster): number {
  return cluster.pullRequests.reduce((sum, pr) => sum + pr.proposals.length, 0);
}

// One line per cluster, like a thread in T3 Code's sidebar: a red dot when a PR's checks fail, the
// name (green when every PR is approved and passing), how many proposals wait, and how long ago.
export function ClusterRow({
  cluster,
  selected,
  onPress,
}: {
  cluster: PullRequestCluster;
  selected: boolean;
  onPress: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  const count = pending(cluster);
  return (
    <Pressable
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.row, (selected || hovered) && styles.rowActive]}
    >
      <View style={[styles.dot, cluster.pullRequests.some((pr) => pr.checksFailing) && styles.dotFailing]} />
      <Text numberOfLines={1} style={[styles.rowName, ready(cluster) && styles.ready]}>
        {cluster.name || cluster.pullRequests[0].title}
      </Text>
      {count > 0 && <Text style={styles.count}>{count}</Text>}
      {cluster.updatedAt && <Text style={styles.rowTime}>{shortAgo(timestampDate(cluster.updatedAt))}</Text>}
    </Pressable>
  );
}

// One PR per repository, the one whose checks look worst: failing, then running, then passing.
function worstPerRepository(cluster: PullRequestCluster): PullRequest[] {
  const rank = (pr: PullRequest) => (pr.checksFailing ? 0 : pr.checksPassing ? 2 : 1);
  const worst = new Map<string, PullRequest>();
  for (const pr of cluster.pullRequests) {
    const name = pr.repository?.name ?? '';
    const seen = worst.get(name);
    if (!seen || rank(pr) < rank(seen)) worst.set(name, pr);
  }
  return [...worst.values()];
}

// A cluster sized for a thumb: its name over each PR's checks and repository, then how many
// proposals wait and how long ago.
export function PhoneClusterRow({ cluster, onPress }: { cluster: PullRequestCluster; onPress: () => void }) {
  const count = pending(cluster);
  return (
    <Pressable onPress={onPress} style={styles.phoneRow}>
      <View style={{ flex: 1, gap: 3 }}>
        <Text numberOfLines={1} style={[styles.phoneRowName, ready(cluster) && styles.ready]}>
          {cluster.name || cluster.pullRequests[0].title}
        </Text>
        <View style={styles.phoneRepos}>
          {worstPerRepository(cluster).map((pr) => (
            <View key={pr.url} style={styles.phoneRepo}>
              <ChecksIcon pr={pr} size={11} />
              <Text style={styles.meta}>{pr.repository?.name}</Text>
            </View>
          ))}
        </View>
      </View>
      {count > 0 && <Text style={styles.count}>{count}</Text>}
      {cluster.updatedAt && <Text style={styles.rowTime}>{shortAgo(timestampDate(cluster.updatedAt))}</Text>}
    </Pressable>
  );
}

// A spinner while the agent runs; otherwise a blue dot when it proposes something.
function InvestigationMark({ investigation }: { investigation: Investigation }) {
  if (investigation.working) return <ActivityIndicator size={10} color="#8FA8F5" />;
  return <View style={[styles.dot, investigation.proposals.length > 0 && styles.dotProposals]} />;
}

// One line per investigation, like a cluster's.
export function InvestigationRow({
  investigation,
  selected,
  onPress,
}: {
  investigation: Investigation;
  selected: boolean;
  onPress: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  const count = investigation.proposals.length;
  return (
    <Pressable
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.row, (selected || hovered) && styles.rowActive]}
    >
      <InvestigationMark investigation={investigation} />
      <Text numberOfLines={1} style={styles.rowName}>
        {investigation.title}
      </Text>
      {count > 0 && <Text style={styles.count}>{count}</Text>}
      {investigation.startedAt && <Text style={styles.rowTime}>{shortAgo(timestampDate(investigation.startedAt))}</Text>}
    </Pressable>
  );
}

export function PhoneInvestigationRow({ investigation, onPress }: { investigation: Investigation; onPress: () => void }) {
  const count = investigation.proposals.length;
  return (
    <Pressable onPress={onPress} style={styles.phoneRow}>
      <InvestigationMark investigation={investigation} />
      <Text numberOfLines={1} style={[styles.phoneRowName, { flex: 1 }]}>
        {investigation.title}
      </Text>
      {count > 0 && <Text style={styles.count}>{count}</Text>}
      {investigation.startedAt && <Text style={styles.rowTime}>{shortAgo(timestampDate(investigation.startedAt))}</Text>}
    </Pressable>
  );
}

// Above the investigations and the clusters, when there are investigations.
export function SectionTitle({ children }: { children: string }) {
  return <Text style={styles.section}>{children}</Text>;
}

const styles = StyleSheet.create({
  section: {
    color: 'rgba(208, 207, 212, 0.45)',
    fontSize: 11,
    fontWeight: '600',
    letterSpacing: 0.5,
    textTransform: 'uppercase',
    paddingHorizontal: 8,
    paddingTop: 10,
    paddingBottom: 4,
  },
  phoneRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    minHeight: 64,
    paddingHorizontal: 16,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(255, 255, 255, 0.06)',
  },
  phoneRowName: {
    color: '#D0CFD4',
    fontSize: 16,
  },
  phoneRepos: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
  },
  phoneRepo: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 32,
    paddingHorizontal: 8,
    borderRadius: 8,
  },
  rowActive: {
    backgroundColor: 'rgba(255, 255, 255, 0.05)',
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  dotFailing: {
    backgroundColor: '#F87171',
  },
  dotProposals: {
    backgroundColor: '#3767E8',
  },
  rowName: {
    flex: 1,
    color: '#D0CFD4',
    fontSize: 14,
  },
  count: {
    color: '#8FA9F5',
    fontSize: 10,
    fontWeight: '600',
    paddingHorizontal: 6,
    lineHeight: 18,
    borderRadius: 4,
    borderWidth: 1,
    borderColor: 'rgba(55, 103, 232, 0.4)',
    backgroundColor: 'rgba(55, 103, 232, 0.1)',
    fontVariant: ['tabular-nums'],
  },
  rowTime: {
    color: 'rgba(208, 207, 212, 0.45)',
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  ready: {
    color: '#4ADE80',
  },
  meta: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 12,
  },
});
