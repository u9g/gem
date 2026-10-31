import type { Write } from '../../db';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Check, CircleCheck, CircleDashed, CircleX } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { bySubject, ProposalGroup } from '../../ProposalView';
import { PullRequestList } from '../../PullRequestList';
import { Agent, AgentState, PullRequest } from '../../shapes';
import { Icon, t3 } from '../../t3';
import { mergeBy, proposalInputs as proposals, stored } from '../fixtures';

// A pull request being made up, before it's stored.
type Row = { -readonly [K in keyof Write<typeof PullRequest>]: Write<typeof PullRequest>[K] };

// Made-up cluster: one change across three repositories.
const pullRequest = (
  name: string,
  number: number,
  title: string,
  status: { approved?: boolean; checksPassing?: boolean; checksFailing?: boolean },
): Row => ({
    repository: { owner: 'livekit', name },
    number,
    title,
    url: `https://github.com/livekit/${name}/pull/${number}`,
    base: 'main',
    mergeBy: mergeBy(status),
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

const moved = (id: string, to: Row) => ({ ...proposals.find((p) => p.id === id)!, pullRequestUrl: to.url });
agents.proposals = proposals.filter((p) => !['quote-reply', 'failed'].includes(p.id ?? ''));
agentsJs.proposals = [moved('quote-reply', agentsJs)];
pending.proposals = [moved('failed', pending)];
// The rows as the components read them.
const cluster = [agents, agentsJs, protocol, pending].map((row) => stored(PullRequest, row));

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
          <PullRequestList pullRequests={cluster} agents={[]} />
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

// Behind a base that requires it up to date, conflicting with its base, being rebased, and being
// rebased by an agent with a question.
const rebasing = [
  { ...agents, behind: true },
  { ...pullRequest('client-sdk-swift', 611, 'Close data channels idle for 30 s', {}), behind: true, conflicts: true },
  { ...agentsJs, behind: true, rebasing: true },
  { ...protocol, behind: true, rebasing: true },
  pending,
].map((row) => stored(PullRequest, row));
export const DRebase: StoryObj = {
  name: 'D · Rebase',
  render: () => (
    <Page
      proposalsShown={[]}
      header={
        <View style={{ gap: 16 }}>
          <Title />
          <PullRequestList pullRequests={rebasing} agents={rebasingAgents.map((row) => stored(Agent, row))} onRebase={() => {}} />
        </View>
      }
    />
  ),
};

// One PR through a rebase, its checks, and the agent answering the checks that failed.
const step = (title: string, status: Row): Row => ({
    repository: { owner: 'livekit', name: 'agents' },
    number: 4821,
    title,
    // Each step is its own row, so it has its own URL for the agents on it.
    url: `https://github.com/livekit/agents/pull/4821#${title}`,
    base: 'main',
    approved: true,
    mergeBy: mergeBy({ approved: true, ...status }),
    ...status,
  });
const passed = { total: 8, done: 8, failed: 0 };
const failed = { total: 8, done: 8, failed: 2 };
const steps = [
  step('Behind its base', { checksPassing: true, checks: passed, behind: true }),
  step('Conflicts with its base', { checksPassing: true, checks: passed, behind: true, conflicts: true }),
  step('Rebasing', { checksPassing: true, checks: passed, behind: true, rebasing: true }),
  step('Rebased, not listed yet', { checksPassing: true, checks: passed, rebased: true }),
  step('Checks running', { checks: { total: 8, done: 3, failed: 0 } }),
  step('A check failed, others run', { checksFailing: true, checks: { total: 8, done: 5, failed: 1 } }),
  step('Agent queued', { checksFailing: true, checks: failed }),
  step('Agent looking', { checksFailing: true, checks: failed }),
  step('Proposal ready', { checksFailing: true, checks: failed, proposals: [moved('failed', agents)] }),
  step('No proposal', { checksFailing: true, checks: failed }),
  step('Agent proposed nothing', {
    checksFailing: true,
    checks: failed,
    noProposalReason: 'Proposed nothing: both failures need livekit/protocol#1147 released first.',
  }),
  step('Green', { checksPassing: true, checks: passed }),
];
// Right after Restart investigation: each PR's waiting conversations have new agents at once.
const restarted = [
  step('Queued behind other agents', { checksPassing: true, checks: passed }),
  step('Answering comments', { checksPassing: true, checks: passed }),
  step('Answering a comment and failed checks', { checksFailing: true, checks: failed }),
];

// The agents on the rows above, as the server lists them.
const agent = (id: string, title: string, url: string, state: AgentState = 'AGENT_STATE_WORKING'): Write<typeof Agent> => ({
  id,
  title,
  pullRequestUrls: [url],
  state,
});
const urlOf = (rows: Row[], title: string) => rows.find((pr) => pr.title === title)!.url!;
const rebasingAgents = [agent('rebase:r', 'Rebase agents-js#1290 and protocol#1147', agentsJs.url!), agent('rebase:r2', 'Rebase agents-js#1290 and protocol#1147', protocol.url!)];
const stepAgents = [
  agent('rebase:s', 'Rebase agents#4821', urlOf(steps, 'Rebasing')),
  agent('answer:q', 'Fix checks on livekit/agents#4821', urlOf(steps, 'Agent queued'), 'AGENT_STATE_QUEUED'),
  agent('answer:l', 'Fix checks on livekit/agents#4821', urlOf(steps, 'Agent looking')),
];
const restartedAgents = [
  agent('answer:1', 'Answer davidzhao on livekit/agents#4821', urlOf(restarted, 'Queued behind other agents'), 'AGENT_STATE_QUEUED'),
  agent('answer:2', 'Answer theomonnom on livekit/agents#4821', urlOf(restarted, 'Queued behind other agents'), 'AGENT_STATE_QUEUED'),
  agent('answer:3', 'Answer davidzhao on livekit/agents#4821', urlOf(restarted, 'Answering comments')),
  agent('answer:4', 'Answer theomonnom on livekit/agents#4821', urlOf(restarted, 'Answering comments')),
  agent('answer:5', 'Answer longcw on livekit/agents#4821', urlOf(restarted, 'Answering comments'), 'AGENT_STATE_QUEUED'),
  agent('answer:6', 'Answer davidzhao on livekit/agents#4821', urlOf(restarted, 'Answering a comment and failed checks')),
  agent('answer:7', 'Fix checks on livekit/agents#4821', urlOf(restarted, 'Answering a comment and failed checks')),
];
export const FRestarted: StoryObj = {
  name: 'F · Restarted',
  render: () => (
    <Page
      proposalsShown={[]}
      header={
        <View style={{ gap: 16 }}>
          <Title />
          <PullRequestList pullRequests={restarted.map((row) => stored(PullRequest, row))} agents={restartedAgents.map((row) => stored(Agent, row))} />
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
          <PullRequestList pullRequests={steps.map((row) => stored(PullRequest, row))} agents={stepAgents.map((row) => stored(Agent, row))} onRebase={() => {}} />
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
