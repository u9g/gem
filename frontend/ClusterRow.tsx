import { timestampDate } from '@bufbuild/protobuf/wkt';
import { useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Investigation, PullRequest, PullRequestCluster } from './gen/gem_pb';
import { t3 } from './t3';

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

// Where a cluster sits on a phone's list, by what it waits on: the user first, then its agents, then
// only a merge, its checks, or a reviewer.
export const sections = ['Needs you', 'Ready to merge', 'Agents working', 'Checks running', 'Waiting on review'] as const;

export function section(cluster: PullRequestCluster): (typeof sections)[number] {
  const prs = cluster.pullRequests;
  const running = (pr: PullRequest) => (pr.checks?.total ?? 0) > (pr.checks?.done ?? 0);
  const needsYou = (pr: PullRequest) =>
    pr.proposals.length > 0 ||
    pr.rebase?.question ||
    pr.merge?.question ||
    (pr.behind && !pr.rebase) ||
    (pr.checksFailing && !running(pr) && pr.agents.length === 0);
  if (prs.some(needsYou)) return 'Needs you';
  if (prs.some((pr) => pr.agents.length > 0 || pr.rebase || pr.merge)) return 'Agents working';
  if (cluster.mergeable) return 'Ready to merge';
  if (prs.some(running)) return 'Checks running';
  return 'Waiting on review';
}

const SEGMENT_WIDTH = 36;
const SEGMENT_GAP = 4;

// Each PR's checks, review and merge, as three bars: green done, red failed, amber in progress.
function segments(pr: PullRequest): (string | undefined)[] {
  const checks = pr.checksFailing ? t3.deletion : pr.checksPassing ? t3.addition : t3.warning;
  const review = pr.approved ? t3.addition : undefined;
  const merge = pr.merge ? t3.warning : undefined;
  return [checks, review, merge];
}

// Above the first card, naming the bars.
export function Legend() {
  return (
    <View style={styles.legend}>
      {['Checks', 'Review', 'Merge'].map((name) => (
        <Text key={name} style={styles.legendText}>
          {name}
        </Text>
      ))}
    </View>
  );
}

// A cluster sized for a thumb: its name and how long ago, over one line per PR with its repository,
// number and bars. On the board, where columns are narrow, the name wraps to a second line, the bars
// shorten, and a PR's waiting proposals are counted; the open cluster's card is outlined.
export function PhoneClusterRow({
  cluster,
  board = false,
  selected = false,
  onPress,
}: {
  cluster: PullRequestCluster;
  board?: boolean;
  selected?: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable onPress={onPress} style={[styles.card, board && styles.boardCard, selected && styles.cardSelected]}>
      <View style={styles.cardTop}>
        <Text numberOfLines={board ? 2 : 1} style={[styles.cardName, board && styles.boardCardName, cluster.mergeable && styles.ready]}>
          {cluster.name || cluster.pullRequests[0].title}
        </Text>
        {cluster.updatedAt && <Text style={styles.rowTime}>{shortAgo(timestampDate(cluster.updatedAt))}</Text>}
      </View>
      {cluster.pullRequests.map((pr) => (
        <View key={pr.url} style={styles.pipe}>
          <Text numberOfLines={1} style={styles.pipeRef}>
            <Text style={styles.pipeRepo}>{pr.repository?.name}</Text> #{pr.number}
          </Text>
          {board && pr.proposals.length > 0 && <Text style={styles.count}>{pr.proposals.length}</Text>}
          {segments(pr).map((color, i) => (
            <View
              key={i}
              style={[styles.segment, board && styles.boardSegment, { backgroundColor: color ?? 'rgba(255, 255, 255, 0.08)' }]}
            />
          ))}
        </View>
      ))}
    </Pressable>
  );
}

// A spinner while the agent runs; otherwise a blue dot when it proposes something.
function InvestigationMark({ investigation }: { investigation: Investigation }) {
  if (investigation.working) return <ActivityIndicator size={10} color="#8FA8F5" />;
  return <View style={[styles.dot, investigation.proposals.length > 0 && styles.dotProposals]} />;
}

// One line per investigation or follow-up, like a cluster's.
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

// Above the agents and the clusters, when there are agents.
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
  card: {
    gap: 6,
    marginHorizontal: 12,
    marginBottom: 8,
    paddingHorizontal: 14,
    paddingVertical: 12,
    borderRadius: 14,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  boardCard: {
    marginHorizontal: 0,
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: 12,
  },
  cardSelected: {
    borderColor: 'rgba(55, 103, 232, 0.55)',
    backgroundColor: 'rgba(55, 103, 232, 0.08)',
  },
  cardTop: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginBottom: 2,
  },
  cardName: {
    flex: 1,
    color: t3.foreground,
    fontSize: 15,
    fontWeight: '600',
  },
  boardCardName: {
    fontSize: 14,
  },
  pipe: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: SEGMENT_GAP,
  },
  pipeRef: {
    flex: 1,
    color: t3.muted,
    fontSize: 12.5,
  },
  pipeRepo: {
    color: t3.foreground,
    fontWeight: '600',
  },
  segment: {
    width: SEGMENT_WIDTH,
    height: 4,
    borderRadius: 2,
  },
  boardSegment: {
    width: 24,
  },
  legend: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: SEGMENT_GAP,
    paddingHorizontal: 26,
    paddingVertical: 6,
    backgroundColor: t3.background,
  },
  legendText: {
    width: SEGMENT_WIDTH,
    color: t3.faint,
    fontSize: 9,
    textAlign: 'center',
  },
  phoneRowName: {
    color: '#D0CFD4',
    fontSize: 16,
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
});
