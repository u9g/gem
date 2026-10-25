import { clone, create, type MessageInitShape } from '@bufbuild/protobuf';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Check, CircleCheck, CircleDashed, CircleX } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { type PullRequest, ProposalSchema, PullRequestSchema, RebaseSchema } from '../../gen/gem_pb';
import { bySubject, ProposalGroup } from '../../ProposalView';
import { PullRequestList } from '../../PullRequestList';
import { Icon, t3 } from '../../t3';
import { proposals } from '../fixtures';

// Made-up cluster: one change across three repositories.
const pullRequest = (name: string, number: number, title: string, status: { approved?: boolean; checksPassing?: boolean; checksFailing?: boolean }) =>
  create(PullRequestSchema, {
    repository: { owner: 'livekit', name },
    number,
    title,
    url: `https://github.com/livekit/${name}/pull/${number}`,
    ...status,
  });

const agents = pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', { checksFailing: true });
const agentsJs = pullRequest('agents-js', 1290, 'feat(voice): retry TTS once on a dropped stream', {
  approved: true,
  checksPassing: true,
});
const protocol = pullRequest('protocol', 1147, 'Add retry_interval to TTS connection options', {
  approved: true,
  checksPassing: true,
});
const pending = pullRequest('agents', 4830, 'test(voice): cover TTS retries in the e2e suite', {});

const moved = (id: string, to: PullRequest) => {
  const proposal = clone(ProposalSchema, proposals.find((p) => p.id === id)!);
  proposal.pullRequestUrl = to.url;
  return proposal;
};
agents.proposals = proposals.filter((p) => !['quote-reply', 'failed'].includes(p.id));
agentsJs.proposals = [moved('quote-reply', agentsJs)];
pending.proposals = [moved('failed', pending)];
const cluster = [agents, agentsJs, protocol, pending];

function ref(pr: PullRequest) {
  return `${pr.repository?.name}#${pr.number}`;
}

// Checks on the head commit: failed, passed, or still running.
function ChecksIcon({ pr, size = 14 }: { pr: PullRequest; size?: number }) {
  if (pr.checksFailing) return <Icon icon={CircleX} size={size} color={t3.deletion} />;
  if (pr.checksPassing) return <Icon icon={CircleCheck} size={size} color={t3.addition} />;
  return <Icon icon={CircleDashed} size={size} color={t3.warning} />;
}

function Count({ n }: { n: number }) {
  if (n === 0) return null;
  return <Text style={styles.count}>{n}</Text>;
}

function Title({ children }: { children?: ReactNode }) {
  return (
    <View style={{ gap: 4 }}>
      <Text style={styles.title}>TTS retry fixes</Text>
      <Text style={styles.meta}>2 hours ago</Text>
      {children}
    </View>
  );
}

function Page({ header, proposalsShown }: { header: ReactNode; proposalsShown: PullRequest[] }) {
  return (
    <View style={styles.page}>
      <View style={styles.column}>
        {header}
        {bySubject(proposalsShown.flatMap((pr) => pr.proposals)).map((group) => (
          <ProposalGroup key={group[0].id} proposals={group} onApprove={() => {}} onDismiss={() => {}} />
        ))}
      </View>
    </View>
  );
}

// ---------- A: chips ----------

// One pill per PR under the title: checks icon, repo#number, a check when approved, and how many
// proposals wait on it. The least space; the title is on GitHub.
function Chips() {
  return (
    <Page
      proposalsShown={cluster}
      header={
        <Title>
          <View style={styles.chips}>
            {cluster.map((pr) => (
              <Pressable key={pr.url} style={styles.chip}>
                <ChecksIcon pr={pr} size={12} />
                <Text style={styles.chipText}>{ref(pr)}</Text>
                {pr.approved && <Check size={12} color={t3.addition} strokeWidth={2.5} />}
                <Count n={pr.proposals.length} />
              </Pressable>
            ))}
          </View>
        </Title>
      }
    />
  );
}

// ---------- B: list ----------

// The app's design: a bordered list with one row per PR.
function List() {
  return (
    <Page
      proposalsShown={cluster}
      header={
        <View style={{ gap: 16 }}>
          <Title />
          <PullRequestList pullRequests={cluster} />
        </View>
      }
    />
  );
}

// ---------- C: tabs ----------

// The PRs are tabs over the conversations: All, then one per PR with its checks and how many
// proposals wait on it. Picking one shows its title and only its conversations.
function Tabs() {
  const [picked, setPicked] = useState<PullRequest>();
  const total = cluster.reduce((n, pr) => n + pr.proposals.length, 0);
  return (
    <Page
      proposalsShown={picked ? [picked] : cluster}
      header={
        <View style={{ gap: 14 }}>
          <Title />
          <View style={styles.tabs}>
            <Pressable onPress={() => setPicked(undefined)} style={[styles.tab, !picked && styles.tabOn]}>
              <Text style={[styles.tabText, !picked && { color: t3.foreground }]}>All</Text>
              <Count n={total} />
            </Pressable>
            {cluster.map((pr) => (
              <Pressable key={pr.url} onPress={() => setPicked(pr)} style={[styles.tab, picked === pr && styles.tabOn]}>
                <ChecksIcon pr={pr} size={12} />
                <Text style={[styles.tabText, picked === pr && { color: t3.foreground }]}>{ref(pr)}</Text>
                <Count n={pr.proposals.length} />
              </Pressable>
            ))}
          </View>
          {picked && (
            <Text style={styles.meta}>
              <Text style={{ color: t3.foreground }}>{picked.title}</Text>
              {'  ·  '}
              <Text style={picked.approved && { color: t3.addition }}>
                {picked.approved ? 'Approved' : 'Review required'}
              </Text>
              {'  ·  '}
              <Text style={{ color: picked.checksFailing ? t3.deletion : picked.checksPassing ? t3.addition : t3.warning }}>
                {picked.checksFailing ? 'Checks failing' : picked.checksPassing ? 'Checks passing' : 'Checks running'}
              </Text>
            </Text>
          )}
        </View>
      }
    />
  );
}

const meta: Meta = { title: 'Pull requests at the top', parameters: { layout: 'fullscreen' } };
export default meta;

export const AChips: StoryObj = { name: 'A · Chips', render: () => <Chips /> };
export const BList: StoryObj = { name: 'B · List', render: () => <List /> };
export const CTabs: StoryObj = { name: 'C · Tabs', render: () => <Tabs /> };

// Behind its base, being rebased, and being rebased by an agent with a question.
const threadUrl = 'https://app.t3.codes/environment/thread';
const rebasing = [
  { ...agents, behind: true },
  { ...agentsJs, behind: true, rebase: create(RebaseSchema, { threadUrl }) },
  { ...protocol, behind: true, rebase: create(RebaseSchema, { threadUrl, question: true }) },
  pending,
];
export const DRebase: StoryObj = {
  name: 'D · Rebase',
  render: () => (
    <Page
      proposalsShown={[]}
      header={
        <View style={{ gap: 16 }}>
          <Title />
          <PullRequestList pullRequests={rebasing} onRebase={() => {}} />
        </View>
      }
    />
  ),
};

// One PR through a rebase, its checks, and the agent answering the checks that failed.
const step = (title: string, status: Exclude<MessageInitShape<typeof PullRequestSchema>, PullRequest>) =>
  create(PullRequestSchema, {
    repository: { owner: 'livekit', name: 'agents' },
    number: 4821,
    title,
    url: 'https://github.com/livekit/agents/pull/4821',
    approved: true,
    ...status,
  });
const passed = { total: 8, done: 8, failed: 0 };
const failed = { total: 8, done: 8, failed: 2 };
const steps = [
  step('Behind its base', { checksPassing: true, checks: passed, behind: true }),
  step('Rebasing', { checksPassing: true, checks: passed, behind: true, rebase: { threadUrl } }),
  step('Rebased, not listed yet', { checksPassing: true, checks: passed, rebase: { threadUrl, pushed: true } }),
  step('Checks running', { checks: { total: 8, done: 3, failed: 0 } }),
  step('A check failed, others run', { checksFailing: true, checks: { total: 8, done: 5, failed: 1 } }),
  step('Agent queued', { checksFailing: true, checks: failed, agents: [{ queued: true, checks: true }] }),
  step('Agent looking', { checksFailing: true, checks: failed, agents: [{ threadUrl, checks: true }] }),
  step('Proposal ready', { checksFailing: true, checks: failed, proposals: [moved('failed', agents)] }),
  step('No proposal', { checksFailing: true, checks: failed }),
  step('Green', { checksPassing: true, checks: passed }),
];
// Right after Restart investigation: each PR's waiting conversations have new agents at once.
const restarted = [
  step('Queued behind other agents', { checksPassing: true, checks: passed, agents: [{ queued: true }, { queued: true }] }),
  step('Answering comments', { checksPassing: true, checks: passed, agents: [{ threadUrl }, { threadUrl }, { queued: true }] }),
  step('Answering a comment and failed checks', {
    checksFailing: true,
    checks: failed,
    agents: [{ threadUrl }, { threadUrl, checks: true }],
  }),
];
export const FRestarted: StoryObj = {
  name: 'F · Restarted',
  render: () => (
    <Page
      proposalsShown={[]}
      header={
        <View style={{ gap: 16 }}>
          <Title />
          <PullRequestList pullRequests={restarted} />
        </View>
      }
    />
  ),
};

export const EStatus: StoryObj = {
  name: 'E · Status',
  render: () => (
    <Page
      proposalsShown={[]}
      header={
        <View style={{ gap: 16 }}>
          <Title />
          <PullRequestList pullRequests={steps} onRebase={() => {}} />
        </View>
      }
    />
  ),
};

const styles = StyleSheet.create({
  page: {
    minHeight: '100%',
    alignItems: 'center',
    paddingHorizontal: 24,
    paddingVertical: 32,
    backgroundColor: t3.background,
  },
  column: {
    width: '100%',
    maxWidth: 1240,
    gap: 28,
  },
  title: {
    color: t3.foreground,
    fontSize: 20,
    fontWeight: '600',
  },
  meta: {
    color: t3.muted,
    fontSize: 12,
  },
  count: {
    minWidth: 18,
    paddingHorizontal: 5,
    borderRadius: 6,
    color: '#8FA8F5',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
    fontSize: 11,
    lineHeight: 18,
    textAlign: 'center',
    fontVariant: ['tabular-nums'],
  },
  chips: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 6,
    marginTop: 8,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    height: 26,
    paddingRight: 6,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.input,
  },
  chipText: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
    marginRight: 2,
  },
  tabs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignSelf: 'flex-start',
    gap: 2,
    padding: 3,
    borderRadius: 10,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  tab: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    height: 28,
    paddingHorizontal: 8,
    borderRadius: 8,
  },
  tabOn: {
    backgroundColor: 'rgba(255, 255, 255, 0.08)',
  },
  tabText: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
});
