import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  CircleDashed,
  CircleX,
  Inbox,
  Layers,
  Menu,
  SlidersHorizontal,
} from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from '../../gen/gem_pb';
import { byComment } from '../../ProposalView';
import { Bubble, Clamp, DiffSurface, FollowUpCard, Html, Theirs, ToolRequestBanner } from '../../surfaces';
import { DiffStat, diffFiles, Icon, proposalIcon, stepSentence, t3 } from '../../t3';
import { long } from '../compact/long';
import { proposals } from '../fixtures';

// ---------- made-up clusters ----------

type Cluster = {
  name: string;
  waiting: number;
  ago: string;
  repos: { name: string; checks: 'failing' | 'passing' | 'running' }[];
  // Every PR approved with checks passing.
  ready?: boolean;
};

const clusters: Cluster[] = [
  {
    name: 'TTS retry fixes',
    waiting: 6,
    ago: '2h',
    repos: [
      { name: 'agents', checks: 'failing' },
      { name: 'agents-js', checks: 'passing' },
      { name: 'protocol', checks: 'passing' },
    ],
  },
  { name: 'Room close ordering', waiting: 2, ago: '5h', repos: [{ name: 'agents', checks: 'running' }] },
  {
    name: 'Turn detector warmup',
    waiting: 0,
    ago: '6h',
    ready: true,
    repos: [{ name: 'agents', checks: 'passing' }],
  },
  {
    name: 'Faster plugin imports',
    waiting: 4,
    ago: '1d',
    ready: true,
    repos: [
      { name: 'agents', checks: 'passing' },
      { name: 'agents-js', checks: 'passing' },
    ],
  },
  { name: 'Interrupt on DTMF', waiting: 1, ago: '1d', repos: [{ name: 'agents', checks: 'failing' }] },
  { name: 'Metrics for queued audio', waiting: 0, ago: '2d', repos: [{ name: 'agents-js', checks: 'passing' }] },
  {
    name: 'Shorter endpointing defaults',
    waiting: 3,
    ago: '3d',
    repos: [
      { name: 'agents', checks: 'running' },
      { name: 'docs', checks: 'passing' },
    ],
  },
  { name: 'Retry SIP dispatch', waiting: 1, ago: '1w', repos: [{ name: 'agents', checks: 'passing' }] },
];

// The first cluster's conversations: the long one first, then the shorter fixtures.
const groups = [long, ...byComment(proposals)];
const total = clusters.reduce((n, c) => n + c.waiting, 0);

// ---------- shared ----------

function ChecksIcon({ checks, size = 12 }: { checks: Cluster['repos'][number]['checks']; size?: number }) {
  if (checks === 'failing') return <Icon icon={CircleX} size={size} color={t3.deletion} />;
  if (checks === 'passing') return <Icon icon={CircleCheck} size={size} color={t3.addition} />;
  return <Icon icon={CircleDashed} size={size} color={t3.warning} />;
}

function Count({ n }: { n: number }) {
  if (n === 0) return null;
  return <Text style={styles.count}>{n}</Text>;
}

// A phone-sized screen: a 390×844 viewport with its own scrolling, like an iPhone 15.
function Phone({ label, children }: { label: string; children: ReactNode }) {
  return (
    <View style={{ gap: 8 }}>
      <Text style={styles.phoneLabel}>{label}</Text>
      <View style={styles.phone}>{children}</View>
    </View>
  );
}

// One cluster row sized for a thumb: 64px tall, name over its repos, waiting count and age.
function ClusterRow({ cluster, selected }: { cluster: Cluster; selected?: boolean }) {
  return (
    <Pressable style={[styles.clusterRow, selected && styles.clusterRowOn]}>
      <View style={{ flex: 1, gap: 3 }}>
        <Text numberOfLines={1} style={[styles.clusterName, cluster.ready && { color: '#4ADE80' }]}>
          {cluster.name}
        </Text>
        <View style={styles.repoLine}>
          {cluster.repos.map((repo) => (
            <View key={repo.name} style={styles.repo}>
              <ChecksIcon checks={repo.checks} size={11} />
              <Text style={styles.repoText}>{repo.name}</Text>
            </View>
          ))}
        </View>
      </View>
      <Count n={cluster.waiting} />
      <Text style={styles.ago}>{cluster.ago}</Text>
    </Pressable>
  );
}

function BigButton({ label, primary, flex }: { label: string; primary?: boolean; flex?: boolean }) {
  return (
    <Pressable style={[styles.bigButton, primary && styles.bigButtonPrimary, flex && { flex: 1 }]}>
      <Text style={[styles.bigButtonText, primary && { color: '#fff' }]}>{label}</Text>
    </Pressable>
  );
}

// A conversation stacked for a narrow screen: what approving does, their comment, the reply, the
// change, each cut to a few lines, then thumb-sized actions. `actions` false leaves them to a bar.
function MobileCard({ group, actions = true }: { group: Proposal[]; actions?: boolean }) {
  const [picked, setPicked] = useState(0);
  const proposal = group[picked];
  const files = proposal.patch ? diffFiles(proposal.patch.diff) : [];
  return (
    <View style={styles.card}>
      <View style={styles.cardHeader}>
        <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
        <Text style={styles.summary}>{proposal.summary}</Text>
      </View>
      {group.length > 1 && (
        <View style={styles.stepper}>
          <Pressable onPress={() => setPicked((picked + group.length - 1) % group.length)}>
            <Icon icon={ChevronLeft} size={14} />
          </Pressable>
          <Text style={styles.meta}>
            Option {picked + 1} of {group.length}
          </Text>
          <Pressable onPress={() => setPicked((picked + 1) % group.length)}>
            <Icon icon={ChevronRight} size={14} />
          </Pressable>
        </View>
      )}
      <Text style={styles.where}>agents#4821 · {proposal.threadId ? 'review thread' : 'conversation'}</Text>
      <Theirs comment={proposal.comment!} max={100} />
      {proposal.reply !== '' && (
        <Bubble side="right">
          <Clamp max={100}>
            <Html html={proposal.replyHtml} />
          </Clamp>
        </Bubble>
      )}
      {proposal.followUp !== '' && (
        <FollowUpCard
          suggested={proposal.followUp}
          html={proposal.followUpHtml}
          draft={{ yours: proposal.followUp, showing: 'suggested' }}
          onChange={() => {}}
        />
      )}
      {proposal.patch && (
        <View style={{ gap: 6 }}>
          <Text style={styles.meta}>
            {stepSentence(proposal)}
            {'  '}
            <DiffStat added={files.reduce((n, f) => n + f.added, 0)} removed={files.reduce((n, f) => n + f.removed, 0)} />
          </Text>
          <Clamp max={110}>
            <DiffSurface diff={proposal.patch.diff} />
          </Clamp>
        </View>
      )}
      {proposal.requestedTool !== '' && <ToolRequestBanner proposal={proposal} />}
      {actions && (
        <View style={styles.actions}>
          <BigButton label="Dismiss" flex />
          {!proposal.requestedTool && <BigButton label={proposal.error ? 'Retry' : 'Approve'} primary flex />}
        </View>
      )}
    </View>
  );
}

function Chips({ chips, on }: { chips: string[]; on: string }) {
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      style={{ flexGrow: 0, flexShrink: 0 }}
      contentContainerStyle={styles.chips}
    >
      {chips.map((chip) => (
        <Text key={chip} style={[styles.chip, chip === on && styles.chipOn]}>
          {chip}
        </Text>
      ))}
    </ScrollView>
  );
}

// ---------- A: list, then cluster ----------

// Like iOS Mail: the top level is the cluster list, full width, with thumb-sized rows. Tapping a
// cluster pushes its screen: PR statuses as a strip, then each conversation as a stacked card.
function ListScreen() {
  return (
    <View style={{ flex: 1 }}>
      <View style={styles.topBar}>
        <Text style={styles.largeTitle}>Clusters</Text>
        <Icon icon={SlidersHorizontal} size={18} />
      </View>
      <Chips chips={['All', 'Waiting on you', 'Ready', 'Failing']} on="All" />
      <ScrollView>
        {clusters.map((cluster, i) => (
          <ClusterRow key={cluster.name} cluster={cluster} selected={i === 0} />
        ))}
      </ScrollView>
    </View>
  );
}

function ClusterScreen() {
  const cluster = clusters[0];
  return (
    <View style={{ flex: 1 }}>
      <View style={styles.navBar}>
        <View style={styles.back}>
          <Icon icon={ChevronLeft} size={18} color={t3.primary} />
          <Text style={{ color: '#8FA8F5', fontSize: 15 }}>Clusters</Text>
        </View>
      </View>
      <ScrollView contentContainerStyle={{ padding: 16, gap: 14 }}>
        <View style={{ gap: 6 }}>
          <Text style={styles.title}>{cluster.name}</Text>
          <View style={styles.repoLine}>
            {cluster.repos.map((repo) => (
              <View key={repo.name} style={styles.prChip}>
                <ChecksIcon checks={repo.checks} />
                <Text style={styles.prChipText}>{repo.name}#4821</Text>
              </View>
            ))}
          </View>
        </View>
        {groups.slice(0, 2).map((group) => (
          <MobileCard key={group[0].id} group={group} />
        ))}
      </ScrollView>
    </View>
  );
}

// ---------- B: inbox feed ----------

// The top level is one feed of every conversation waiting on u9g, newest cluster first, under
// sticky cluster headers, so triage never needs to pick a cluster. A tab bar switches to the
// cluster list for browsing.
function TabBar({ on }: { on: 'inbox' | 'clusters' }) {
  return (
    <View style={styles.tabBar}>
      {(
        [
          ['inbox', Inbox, `Inbox`],
          ['clusters', Layers, 'Clusters'],
        ] as const
      ).map(([key, icon, label]) => (
        <View key={key} style={styles.tabItem}>
          <Icon icon={icon} size={20} color={on === key ? t3.foreground : t3.faint} />
          <Text style={[styles.tabLabel, on === key && { color: t3.foreground }]}>
            {label}
            {key === 'inbox' && ` ${total}`}
          </Text>
        </View>
      ))}
    </View>
  );
}

function FeedScreen() {
  return (
    <View style={{ flex: 1 }}>
      <View style={styles.topBar}>
        <Text style={styles.largeTitle}>Inbox</Text>
        <Text style={styles.meta}>{total} waiting</Text>
      </View>
      <Chips chips={['All', 'Patches', 'Replies', 'Follow-ups', 'Failed']} on="All" />
      <ScrollView stickyHeaderIndices={[0, 3]}>
        <SectionHeader cluster={clusters[0]} />
        <View style={{ padding: 12, gap: 12 }}>
          <MobileCard group={groups[0]} />
        </View>
        <View style={{ padding: 12, gap: 12 }}>
          <MobileCard group={groups[1]} />
        </View>
        <SectionHeader cluster={clusters[1]} />
        <View style={{ padding: 12, gap: 12 }}>
          <MobileCard group={groups[2]} />
        </View>
      </ScrollView>
      <TabBar on="inbox" />
    </View>
  );
}

function SectionHeader({ cluster }: { cluster: Cluster }) {
  return (
    <View style={styles.sectionHeader}>
      <Text numberOfLines={1} style={[styles.sectionName, cluster.ready && { color: '#4ADE80' }]}>
        {cluster.name}
      </Text>
      <View style={styles.repoLine}>
        {cluster.repos.map((repo) => (
          <ChecksIcon key={repo.name} checks={repo.checks} size={11} />
        ))}
      </View>
      <Count n={cluster.waiting} />
    </View>
  );
}

function FeedClustersScreen() {
  return (
    <View style={{ flex: 1 }}>
      <View style={styles.topBar}>
        <Text style={styles.largeTitle}>Clusters</Text>
      </View>
      <ScrollView>
        {clusters.map((cluster) => (
          <ClusterRow key={cluster.name} cluster={cluster} />
        ))}
      </ScrollView>
      <TabBar on="clusters" />
    </View>
  );
}

// ---------- C: one at a time ----------

// Triage one conversation at a time: the top level is the current cluster's next conversation
// filling the screen, with Dismiss and Approve pinned at the bottom under the thumb and arrows to
// skip. The cluster list is a drawer behind the title.
function TriageBar() {
  return (
    <View style={styles.triageBar}>
      <Pressable style={styles.skip}>
        <Icon icon={ChevronLeft} size={18} />
      </Pressable>
      <BigButton label="Dismiss" flex />
      <BigButton label="Approve" primary flex />
      <Pressable style={styles.skip}>
        <Icon icon={ChevronRight} size={18} />
      </Pressable>
    </View>
  );
}

function TriageTop() {
  const cluster = clusters[0];
  return (
    <View style={styles.triageTop}>
      <Icon icon={Menu} size={18} />
      <View style={{ flex: 1, flexDirection: 'row', alignItems: 'center', gap: 2 }}>
        <Text numberOfLines={1} style={styles.triageTitle}>
          {cluster.name}
        </Text>
        <Icon icon={ChevronDown} size={14} />
      </View>
      <Text style={styles.meta}>1 of {cluster.waiting}</Text>
    </View>
  );
}

function TriageScreen() {
  const cluster = clusters[0];
  return (
    <View style={{ flex: 1 }}>
      <TriageTop />
      <View style={styles.progress}>
        <View style={[styles.progressFill, { width: `${100 / cluster.waiting}%` }]} />
      </View>
      <View style={[styles.repoLine, { paddingHorizontal: 16, paddingVertical: 8 }]}>
        {cluster.repos.map((repo) => (
          <View key={repo.name} style={styles.prChip}>
            <ChecksIcon checks={repo.checks} />
            <Text style={styles.prChipText}>{repo.name}#4821</Text>
          </View>
        ))}
      </View>
      <ScrollView contentContainerStyle={{ padding: 12 }}>
        <MobileCard group={groups[1]} actions={false} />
      </ScrollView>
      <TriageBar />
    </View>
  );
}

function DrawerScreen() {
  return (
    <View style={{ flex: 1 }}>
      <TriageScreen />
      <View style={styles.scrim} />
      <View style={styles.drawer}>
        <View style={[styles.topBar, { paddingTop: 20 }]}>
          <Text style={styles.largeTitle}>gem</Text>
          <Text style={styles.meta}>{total} waiting</Text>
        </View>
        <ScrollView>
          {clusters.map((cluster, i) => (
            <ClusterRow key={cluster.name} cluster={cluster} selected={i === 0} />
          ))}
        </ScrollView>
      </View>
    </View>
  );
}

// ---------- layout ----------

function Approach({ name, note, children }: { name: string; note: string; children: ReactNode }) {
  return (
    <View style={styles.approach}>
      <Text style={styles.approachName}>{name}</Text>
      <Text style={styles.approachNote}>{note}</Text>
      <View style={styles.phones}>{children}</View>
    </View>
  );
}

function Overview() {
  return (
    <View style={styles.board}>
      <Approach name="A · List, then cluster" note="Clusters fill the first screen; tapping one pushes its conversations.">
        <Phone label="Top level">
          <ListScreen />
        </Phone>
        <Phone label="After tapping a cluster">
          <ClusterScreen />
        </Phone>
      </Approach>
      <Approach name="B · Inbox feed" note="Every waiting conversation in one feed under sticky cluster headers; a tab for clusters.">
        <Phone label="Top level">
          <FeedScreen />
        </Phone>
        <Phone label="Clusters tab">
          <FeedClustersScreen />
        </Phone>
      </Approach>
      <Approach name="C · One at a time" note="The next conversation fills the screen with Approve under the thumb; clusters in a drawer.">
        <Phone label="Top level">
          <TriageScreen />
        </Phone>
        <Phone label="Drawer open">
          <DrawerScreen />
        </Phone>
      </Approach>
    </View>
  );
}

const meta: Meta = { title: 'Mobile', parameters: { layout: 'fullscreen' } };
export default meta;

export const Approaches: StoryObj = { render: () => <Overview /> };
export const AList: StoryObj = { name: 'A · List, then cluster', render: () => <Phone label="Top level"><ListScreen /></Phone> };
export const BFeed: StoryObj = { name: 'B · Inbox feed', render: () => <Phone label="Top level"><FeedScreen /></Phone> };
export const COneAtATime: StoryObj = {
  name: 'C · One at a time',
  render: () => (
    <Phone label="Top level">
      <TriageScreen />
    </Phone>
  ),
};

const styles = StyleSheet.create({
  board: {
    flexDirection: 'row',
    gap: 40,
    padding: 32,
    minHeight: '100%',
    backgroundColor: '#11161C',
  },
  approach: {
    gap: 6,
  },
  approachName: {
    color: t3.foreground,
    fontSize: 18,
    fontWeight: '600',
  },
  approachNote: {
    width: 800,
    color: t3.muted,
    fontSize: 13,
    marginBottom: 10,
  },
  phones: {
    flexDirection: 'row',
    gap: 20,
  },
  phoneLabel: {
    color: t3.faint,
    fontSize: 11,
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  phone: {
    width: 390,
    height: 844,
    borderRadius: 36,
    borderWidth: 1,
    borderColor: 'rgba(255, 255, 255, 0.12)',
    backgroundColor: t3.background,
    overflow: 'hidden',
  },
  topBar: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 16,
    paddingTop: 52,
    paddingBottom: 8,
  },
  largeTitle: {
    color: t3.foreground,
    fontSize: 28,
    fontWeight: '700',
  },
  navBar: {
    paddingTop: 52,
    paddingHorizontal: 8,
    paddingBottom: 4,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  back: {
    flexDirection: 'row',
    alignItems: 'center',
    height: 36,
  },
  title: {
    color: t3.foreground,
    fontSize: 22,
    fontWeight: '600',
  },
  meta: {
    color: t3.muted,
    fontSize: 12,
  },
  chips: {
    gap: 6,
    paddingHorizontal: 16,
    paddingBottom: 10,
  },
  chip: {
    paddingHorizontal: 12,
    height: 30,
    lineHeight: 30,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.input,
    color: t3.muted,
    fontSize: 13,
  },
  chipOn: {
    backgroundColor: t3.foreground,
    borderColor: t3.foreground,
    color: t3.background,
  },
  clusterRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    minHeight: 64,
    paddingHorizontal: 16,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  clusterRowOn: {
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  clusterName: {
    color: t3.foreground,
    fontSize: 16,
  },
  repoLine: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
  },
  repo: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  repoText: {
    color: t3.muted,
    fontSize: 12,
  },
  ago: {
    width: 24,
    textAlign: 'right',
    color: t3.faint,
    fontSize: 12,
  },
  count: {
    minWidth: 22,
    paddingHorizontal: 6,
    borderRadius: 8,
    color: '#8FA8F5',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
    fontSize: 12,
    lineHeight: 22,
    textAlign: 'center',
    fontVariant: ['tabular-nums'],
  },
  prChip: {
    flexDirection: 'row',
    alignItems: 'center',
    height: 26,
    paddingRight: 8,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.input,
  },
  prChipText: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 11,
  },
  card: {
    gap: 10,
    padding: 12,
    borderRadius: 14,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  cardHeader: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 4,
  },
  summary: {
    flex: 1,
    color: t3.foreground,
    fontSize: 15,
    lineHeight: 21,
    fontWeight: '500',
  },
  stepper: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
  },
  where: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  actions: {
    flexDirection: 'row',
    gap: 8,
    marginTop: 2,
  },
  bigButton: {
    height: 44,
    paddingHorizontal: 16,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.input,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  bigButtonPrimary: {
    borderColor: t3.primary,
    backgroundColor: t3.primary,
  },
  bigButtonText: {
    color: t3.foreground,
    fontSize: 15,
    fontWeight: '500',
  },
  sectionHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 40,
    paddingHorizontal: 16,
    backgroundColor: t3.background,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  sectionName: {
    flex: 1,
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '600',
  },
  tabBar: {
    flexDirection: 'row',
    paddingTop: 8,
    paddingBottom: 28,
    borderTopWidth: 1,
    borderTopColor: t3.border,
    backgroundColor: t3.background,
  },
  tabItem: {
    flex: 1,
    alignItems: 'center',
    gap: 2,
  },
  tabLabel: {
    color: t3.faint,
    fontSize: 11,
  },
  triageTop: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingTop: 52,
    paddingHorizontal: 16,
    paddingBottom: 10,
  },
  triageTitle: {
    color: t3.foreground,
    fontSize: 17,
    fontWeight: '600',
    flexShrink: 1,
  },
  progress: {
    height: 2,
    backgroundColor: t3.border,
  },
  progressFill: {
    height: 2,
    backgroundColor: t3.primary,
  },
  triageBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 12,
    paddingTop: 10,
    paddingBottom: 30,
    borderTopWidth: 1,
    borderTopColor: t3.border,
    backgroundColor: t3.background,
  },
  skip: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 12,
  },
  scrim: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.55)',
  },
  drawer: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    left: 0,
    width: 320,
    backgroundColor: '#1D2631',
    borderRightWidth: 1,
    borderRightColor: t3.border,
  },
});
