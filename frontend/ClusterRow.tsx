import { Check, Copy } from 'lucide-react';
import { memo, useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';

import { active } from './agents';
import type { Agent, Cluster, PullRequest } from './shapes';
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

// An agent waiting on the user, or with proposals for them to review, puts its cluster in front of
// them, as a rebase or merge waiting on them does.
export function section(cluster: Cluster): (typeof sections)[number] {
  const prs = cluster.pullRequests;
  const agents = cluster.agents;
  const working = agents.filter(active);
  const running = (pr: PullRequest) => {
    const checks = pr.checks;
    return (checks?.total ?? 0) > (checks?.done ?? 0);
  };
  const answered = (pr: PullRequest) =>
    working.some((agent) => agent.pullRequestUrls.includes(pr.url));
  const needsYou = (pr: PullRequest) =>
    pr.proposals.length > 0 ||
    (pr.behind && !pr.rebasing && !pr.rebased) ||
    ((pr.rebasing || pr.merging) && !answered(pr)) ||
    (pr.checksFailing && !running(pr) && !answered(pr));
  if (agents.some((agent) => !active(agent) || agent.proposals.length > 0) || prs.some(needsYou)) return 'Needs you';
  if (working.length > 0 || prs.some((pr) => pr.rebased)) return 'Agents working';
  if (cluster.mergeBy !== 'MERGE_BY_UNSPECIFIED') return 'Ready to merge';
  if (prs.some(running)) return 'Checks running';
  return 'Waiting on review';
}

const SEGMENT_WIDTH = 36;
const SEGMENT_GAP = 4;

// Each PR's checks, review and merge, as three bars: green done, red failed, amber in progress.
function segments(pr: PullRequest): (string | undefined)[] {
  const checks = pr.checksFailing ? t3.deletion : pr.checksPassing ? t3.addition : t3.warning;
  const review = pr.approved ? t3.addition : undefined;
  const merge = pr.merging ? t3.warning : undefined;
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

// "<url> + <url> - <name>": every PR of the cluster, then the name its card shows.
function CopyCluster({ cluster }: { cluster: Cluster }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    const urls = cluster.pullRequests.map((pr) => pr.url).join(' + ');
    await navigator.clipboard.writeText(`${urls} - ${cluster.name || cluster.pullRequests[0].title}`);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  const Mark = copied ? Check : Copy;
  return (
    <Pressable onPress={copy} hitSlop={10} accessibilityLabel="Copy pull request links">
      <Mark size={13} color={copied ? t3.addition : 'rgba(208, 207, 212, 0.45)'} strokeWidth={2} />
    </Pressable>
  );
}

// A cluster sized for a thumb: its name, a button that copies its links, and how long ago, over one
// line per PR with its repository, number and bars. On the board, where columns are narrow, the name
// wraps to a second line, the bars shorten, and a PR's waiting proposals are counted; the open
// cluster's card is outlined. A card renders again only when its cluster changed, as `cluster` is
// the same node until then.
export const PhoneClusterRow = memo(function PhoneClusterRow({
  cluster,
  board = false,
  selected = false,
  onPress,
}: {
  cluster: Cluster;
  board?: boolean;
  selected?: boolean;
  // Called with the cluster's key.
  onPress: (key: string) => void;
}) {
  const by = cluster.mergeBy;
  const updatedAt = cluster.updatedAt;
  return (
    <Pressable
      onPress={() => onPress(cluster.pullRequests[0].url)}
      style={[styles.card, board && styles.boardCard, selected && styles.cardSelected]}
    >
      <View style={styles.cardTop}>
        <Text numberOfLines={board ? 2 : 1} style={[styles.cardName, board && styles.boardCardName, by === 'MERGE_BY_APPROVAL' && styles.ready, by === 'MERGE_BY_ADMIN' && styles.forceable]}>
          {cluster.name || cluster.pullRequests[0].title}
        </Text>
        <CopyCluster cluster={cluster} />
        {updatedAt && <Text style={styles.rowTime}>{shortAgo(updatedAt)}</Text>}
      </View>
      {cluster.pullRequests.map((pr) => (
        <PullRequestPipe key={pr.url} pr={pr} board={board} />
      ))}
    </Pressable>
  );
});

// One PR of a card: its repository and number, and its bars.
const PullRequestPipe = memo(function PullRequestPipe({ pr, board }: { pr: PullRequest; board: boolean }) {
  const proposals = pr.proposals.length;
  return (
    <View style={styles.pipe}>
      <Text numberOfLines={1} style={styles.pipeRef}>
        <Text style={styles.pipeRepo}>{pr.repository?.name}</Text> #{pr.number}
      </Text>
      {board && proposals > 0 && <Text style={styles.count}>{proposals}</Text>}
      {segments(pr).map((color, i) => (
        <View
          key={i}
          style={[styles.segment, board && styles.boardSegment, { backgroundColor: color ?? 'rgba(255, 255, 255, 0.08)' }]}
        />
      ))}
    </View>
  );
});

// A spinner while the agent runs, a dot while it is queued, and a slot as wide when it waits on the
// user, so titles line up.
function AgentMark({ agent }: { agent: Agent }) {
  const state = agent.state;
  if (state === 'AGENT_STATE_WORKING') return <ActivityIndicator size={10} color="#8FA8F5" />;
  if (state === 'AGENT_STATE_QUEUED') return <View style={[styles.mark, styles.queued]} />;
  return <View style={styles.mark} />;
}

function ToReview({ agent }: { agent: Agent }) {
  const count = agent.proposals.length;
  if (count === 0) return null;
  return <Text style={styles.toReview}>{count} to review</Text>;
}

// One line per agent, like a cluster's.
export const AgentRow = memo(function AgentRow({
  agent,
  selected,
  onPress,
}: {
  agent: Agent;
  selected: boolean;
  // Called with the agent's ID.
  onPress: (id: string) => void;
}) {
  const [hovered, setHovered] = useState(false);
  const startedAt = agent.startedAt;
  return (
    <Pressable
      onPress={() => onPress(agent.id)}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.row, (selected || hovered) && styles.rowActive]}
    >
      <AgentMark agent={agent} />
      <Text numberOfLines={1} style={styles.rowName}>
        {agent.title}
      </Text>
      <ToReview agent={agent} />
      {startedAt && <Text style={styles.rowTime}>{shortAgo(startedAt)}</Text>}
    </Pressable>
  );
});

export const PhoneAgentRow = memo(function PhoneAgentRow({
  agent,
  onPress,
}: {
  agent: Agent;
  // Called with the agent's ID.
  onPress: (id: string) => void;
}) {
  const startedAt = agent.startedAt;
  return (
    <Pressable onPress={() => onPress(agent.id)} style={styles.phoneRow}>
      <AgentMark agent={agent} />
      <Text numberOfLines={1} style={[styles.phoneRowName, { flex: 1 }]}>
        {agent.title}
      </Text>
      <ToReview agent={agent} />
      {startedAt && <Text style={styles.rowTime}>{shortAgo(startedAt)}</Text>}
    </Pressable>
  );
});

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
  mark: {
    width: 10,
  },
  queued: {
    height: 6,
    width: 6,
    marginHorizontal: 2,
    borderRadius: 3,
    backgroundColor: t3.faint,
  },
  toReview: {
    color: '#FFFFFF',
    fontSize: 10,
    fontWeight: '600',
    paddingHorizontal: 7,
    lineHeight: 18,
    borderRadius: 9,
    backgroundColor: '#3767E8',
    fontVariant: ['tabular-nums'],
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
  forceable: {
    color: t3.warning,
  },
});
