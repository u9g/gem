import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { useMemo, useState } from 'react';
import { Text, View } from 'react-native';

import { AgentView } from '../../AgentView';
import { ClusterReview } from '../../ClusterFiles';
import { DbContext, useDb } from '../../db';
import { Inbox } from '../../Inbox';
import { MemoryDb } from '../../memory';
import { SearchBox } from '../../Search';
import { t3, usePhone } from '../../t3';
import { shown } from '../../view';
import type { Agent, Cluster, Proposal, PullRequest } from '../../view-types';
import { blank, clusterMergeBy, mergeBy, prFiles } from '../fixtures';

// Made-up clusters and agents, at least one for every reason a row is in the inbox and a few that
// only wait on someone else. Stories render at the window's width: a laptop's shows the sidebar, a
// phone's the row of chips.

const minutesAgo = (minutes: number) => Date.now() - minutes * 60_000;

type Fields = Partial<
  Pick<
    PullRequest,
    'approved' | 'checksPassing' | 'checksFailing' | 'behind' | 'conflicts' | 'checks' | 'proposals' | 'merging' | 'rebasing' | 'files'
  >
> & { admin?: boolean };

const pullRequest = (repo: string, number: number, title: string, { admin, ...fields }: Fields): PullRequest => ({
  ...blank.pullRequest,
  repository: { owner: 'livekit', name: repo },
  number,
  title,
  url: `https://github.com/livekit/${repo}/pull/${number}`,
  base: 'main',
  checks: { total: 14, done: 14, failed: 0 },
  mergeBy: mergeBy({ ...fields, admin }),
  ...fields,
});

const cluster = (name: string, minutes: number, pullRequests: PullRequest[], agents: Agent[] = []): Cluster => ({
  ...blank.cluster,
  name,
  updatedAt: minutesAgo(minutes),
  pullRequests,
  mergeBy: clusterMergeBy(pullRequests),
  agents,
});

const passing = { approved: true, checksPassing: true };

const proposal = (id: string, url: string, summary: string, fields: Partial<Proposal> = {}): Proposal => ({
  ...blank.proposal,
  id,
  pullRequestUrl: url,
  summary,
  summaryHtml: `<p>${summary}</p>`,
  ...fields,
});

const reviewComment = (id: string, url: string, author: string, body: string, summary: string): Proposal =>
  proposal(id, url, summary, {
    comment: { ...blank.comment, id, author, body, bodyHtml: `<p>${body}</p>`, url: `${url}#discussion_${id}`, path: 'src/index.ts', line: 12 },
    reply: 'Good catch, fixed.',
    resolveThread: true,
  });

// Checks failing, with proposals to fix them.
const failing = cluster('Retry TTS once on a dropped stream', 6, [
  pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', {
    checksFailing: true,
    checks: { total: 14, done: 14, failed: 2 },
    files: prFiles,
  }),
  pullRequest('agents-js', 1290, 'feat(voice): retry TTS once on a dropped stream', passing),
]);

const agentsUrl = 'https://github.com/livekit/agents/pull/4833';
const proposals = cluster('Turn detector: drop the ONNX warmup', 14, [
  pullRequest('agents', 4833, 'perf(turn-detector): drop the ONNX warmup', {
    checksPassing: true,
    proposals: [
      proposal('p1', agentsUrl, 'Keep a lazy warmup behind TURN_DETECTOR_WARMUP and document it'),
      proposal('p2', agentsUrl, 'Rerun the flaky e2e job', { rerun: true }),
    ],
  }),
]);

const gatewayUrl = 'https://github.com/livekit/agent-gateway/pull/391';
const comments = cluster('Hedge gateway requests after the p95', 28, [
  pullRequest('agent-gateway', 391, 'Hedge requests after the p95', {
    checksPassing: true,
    proposals: [
      reviewComment('c1', gatewayUrl, 'davidzhao', 'Should the hedge delay come from config? 50 ms is tight for EU.', 'Read the delay from GatewayConfig.hedge_after'),
      reviewComment('c2', gatewayUrl, 'theomonnom', 'nit: cancel the losing request', 'Cancel the slower request once one answers'),
    ],
  }),
  pullRequest('protocol', 1112, 'Add hedge_after to GatewayConfig', passing),
]);

const finishedAgent: Agent = {
  ...blank.agent,
  id: 'investigate-egress',
  title: 'Investigate S3 upload timeouts on egress#905',
  state: 'waiting',
  pullRequestUrls: ['https://github.com/livekit/egress/pull/905'],
  reply: 'The timeouts come from the default 5 s part timeout; raising it to 30 s matches the SDK default.',
  replyHtml: '<p>The timeouts come from the default 5 s part timeout.</p>',
  startedAt: minutesAgo(52),
};
const agentFinished = cluster(
  'Stream large uploads in parts',
  45,
  [pullRequest('egress', 905, 'Stream large uploads to S3 in parts', { checksPassing: true })],
  [finishedAgent],
);

const behind = cluster('Close idle data channels', 70, [
  pullRequest('client-sdk-swift', 611, 'Close data channels idle for 30 s', { checksPassing: true, behind: true }),
  pullRequest('client-sdk-android', 742, 'Close data channels idle for 30 s', { checksPassing: true, conflicts: true }),
]);

const ready = cluster('Respect the configured S3 region', 180, [
  pullRequest('protocol', 1103, 'Add region to S3 upload options', passing),
  pullRequest('egress', 902, 'Upload to the configured S3 region', passing),
]);

const readyAdmin = cluster('Bump pion to v4.1', 300, [
  pullRequest('livekit', 3402, 'Bump pion/webrtc to v4.1.2', { checksPassing: true, admin: true }),
]);

const review = cluster('Pin agent version on dispatch', 22, [
  pullRequest('agents', 4810, 'Pin agent version on dispatch', { checksPassing: true }),
  pullRequest('livekit', 3398, 'Send the pinned agent version with dispatch', { checksPassing: true }),
]);

const review2 = cluster('', 60 * 26, [
  pullRequest('livekit-cli', 702, 'lk agent simulate: print the run URL first', { checksPassing: true }),
]);

// Only waiting on someone else: checks running, an agent working, a merge.
const running = cluster('Type room metadata updates', 3, [
  pullRequest('client-sdk-js', 1612, 'Type room metadata updates', { checks: { total: 22, done: 15, failed: 0 } }),
]);
const merging = cluster('Drop the legacy room service client', 2, [
  pullRequest('server-sdk-go', 540, 'Remove RoomServiceClientV1', { ...passing, merging: true }),
]);
const working = cluster(
  'Fix flaky ICE restart test',
  9,
  [pullRequest('livekit', 3405, 'Fix flaky ICE restart test', { checksFailing: true, checks: { total: 14, done: 14, failed: 1 } })],
  [{ ...blank.agent, id: 'fix-ice', title: 'Fix checks on livekit#3405', state: 'working', pullRequestUrls: ['https://github.com/livekit/livekit/pull/3405'] }],
);

const clusters = [failing, proposals, comments, agentFinished, behind, ready, readyAdmin, review, review2, running, merging, working];

// Agents acting on no open pull request.
const loneAgents: Agent[] = [
  {
    ...blank.agent,
    id: 'investigation-merged',
    title: 'Investigate e2e on agents#4790',
    pullRequestUrls: ['https://github.com/livekit/agents/pull/4790'],
    state: 'waiting',
    reply: 'The e2e failure was a flaky TURN server; it passed on rerun before the merge.',
    replyHtml: '<p>The e2e failure was a flaky TURN server; it passed on rerun before the merge.</p>',
    startedAt: minutesAgo(95),
  },
  {
    ...blank.agent,
    id: 'open-ruff-pin',
    title: 'Open a PR on main pinning ruff to 0.5',
    pullRequestUrls: ['https://github.com/livekit/agents/pull/4799'],
    state: 'waiting',
    proposals: [proposal('np1', 'https://github.com/livekit/agents/pull/4799', 'Open livekit/agents “Pin ruff to 0.5” against main')],
    startedAt: minutesAgo(33),
  },
];

const noop = () => {};

function Toolbar({ phone }: { phone: boolean }) {
  const [query, setQuery] = useState('');
  return (
    <View style={{ flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: 6, padding: 10 }}>
      <Text
        style={{
          color: t3.foreground,
          fontSize: 11,
          paddingHorizontal: 8,
          paddingVertical: 3,
          borderRadius: 999,
          backgroundColor: 'rgba(255, 255, 255, 0.06)',
        }}
      >
        server: connected
      </Text>
      {!phone && (
        <Text style={{ color: t3.muted, fontSize: 11, paddingHorizontal: 8 }}>u9g · gem.local</Text>
      )}
      <SearchBox value={query} onChange={setQuery} status="idle" matched={0} />
    </View>
  );
}

// The inbox as the app shows it, read from a database holding the clusters and agents.
function Home({ initial }: { initial?: Cluster }) {
  const db = useMemo(() => new MemoryDb({ clusters, agents: loneAgents }), []);
  return (
    <DbContext value={db}>
      <Shown initial={initial?.pullRequests[0].url} />
    </DbContext>
  );
}

function Shown({ initial }: { initial?: string }) {
  const { clusters, agents } = useDb(shown);
  const phone = usePhone();
  const [openUrl, setOpenUrl] = useState(initial);
  const [openAgent, setOpenAgent] = useState<string>();
  const opened = clusters.find((c) => c.pullRequests[0].url === openUrl);
  const agent = [...agents, ...clusters.flatMap((c) => c.agents)].find((a) => a.id === openAgent);
  return (
    <View style={{ height: '100vh' as unknown as number }}>
      <Inbox
        phone={phone}
        clusters={clusters}
        agents={agents}
        toolbar={<Toolbar phone={phone} />}
        openKey={agent ? `agent:${agent.id}` : openUrl}
        onOpenCluster={(key) => {
          setOpenAgent(undefined);
          setOpenUrl(key);
        }}
        onOpenAgent={setOpenAgent}
        onClose={() => {
          setOpenUrl(undefined);
          setOpenAgent(undefined);
        }}
        wide={!agent && opened !== undefined}
      >
        {agent ? (
          <AgentView
            agent={agent}
            ago={() => '4 minutes ago'}
            onApprove={noop}
            onDismiss={noop}
            onMessage={() => Promise.resolve()}
            onClose={noop}
          />
        ) : (
          opened && (
            <ClusterReview
              cluster={opened}
              onOpenAgent={setOpenAgent}
              onApprove={noop}
              onDismiss={noop}
              onRestart={noop}
              onRebase={noop}
              onRebaseCluster={noop}
              onMerge={noop}
              onCloseCluster={noop}
              onInvestigate={noop}
              onMessage={() => Promise.resolve()}
            />
          )
        )}
      </Inbox>
    </View>
  );
}

const meta: Meta = { title: 'Home/Inbox' };
export default meta;

export const Notifications: StoryObj = { render: () => <Home /> };
export const ClusterOpen: StoryObj = { render: () => <Home initial={failing} /> };
