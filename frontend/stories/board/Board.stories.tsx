import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { useMemo, useState } from 'react';
import { View } from 'react-native';

import { Board } from '../../Board';
import { ClusterReview } from '../../ClusterFiles';
import { AgentView } from '../../AgentView';
import { DbContext, useDb, type Write } from '../../db';
import { MemoryDb } from '../../memory';
import { Agent, Cluster, clusterKey, Proposal, PullRequest, shown } from '../../shapes';
import { agentInputs as followed, clusterMergeBy, mergeBy, prFiles, proposalInputs as proposals } from '../fixtures';

// Made-up clusters, at least one in each column. Stories render at the window's width, so view
// them at a laptop's.

const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000);

type Fields = {
  approved?: boolean;
  checksPassing?: boolean;
  checksFailing?: boolean;
  behind?: boolean;
  checks?: { total: number; done: number; failed: number };
  proposals?: Write<typeof Proposal>[];
  merging?: boolean;
  files?: { path: string; patch: string }[];
};

const pullRequest = (name: string, number: number, title: string, fields: Fields): Write<typeof PullRequest> => ({
    repository: { owner: 'livekit', name },
    number,
    title,
    url: `https://github.com/livekit/${name}/pull/${number}`,
    base: 'main',
    checks: { total: 14, done: 14, failed: 0 },
    mergeBy: mergeBy(fields),
    ...fields,
  });

const cluster = (name: string, minutes: number, pullRequests: Write<typeof PullRequest>[]): Write<typeof Cluster> => ({
  name,
  updatedAt: minutesAgo(minutes),
  pullRequests,
  mergeBy: clusterMergeBy(pullRequests),
});

const urlOf = (c: Write<typeof Cluster>) => c.pullRequests?.[0]?.url ?? '';

const passing = { approved: true, checksPassing: true };

const needsYou = cluster('Retry TTS once on a dropped stream', 5, [
  pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', {
    checksFailing: true,
    checks: { total: 14, done: 14, failed: 2 },
    proposals,
    files: prFiles,
  }),
  pullRequest('agents-js', 1290, 'feat(voice): retry TTS once on a dropped stream', passing),
]);
const behind = cluster('Close idle data channels', 12, [
  pullRequest('client-sdk-swift', 611, 'Close data channels idle for 30 s', { checksPassing: true, behind: true }),
  pullRequest('client-sdk-android', 742, 'Close data channels idle for 30 s', { checksPassing: true, behind: true }),
]);
const ready1 = cluster('Respect the configured S3 region', 180, [
  pullRequest('protocol', 1103, 'Add region to S3 upload options', passing),
  pullRequest('egress', 902, 'Upload to the configured S3 region', passing),
]);
const merging = cluster('Drop the legacy room service client', 3, [
  pullRequest('server-sdk-go', 540, 'Remove RoomServiceClientV1', { ...passing, merging: true }),
]);
const working = cluster('Hedge gateway requests', 120, [
  pullRequest('agent-gateway', 388, 'Hedge requests after the p95', {
    checks: { total: 14, done: 9, failed: 0 },
  }),
]);
const running = cluster('Type room metadata updates', 40, [
  pullRequest('client-sdk-js', 1612, 'Type room metadata updates', { checks: { total: 22, done: 15, failed: 0 } }),
  pullRequest('protocol', 1108, 'Add a typed metadata field to Room', { checks: { total: 6, done: 6, failed: 0 }, checksPassing: true }),
]);
const reviewing = cluster('Pin agent version on dispatch', 22, [
  pullRequest('agents', 4810, 'Pin agent version on dispatch', { checksPassing: true }),
]);

const clusters = [needsYou, behind, ready1, merging, working, running, reviewing];

// The agents merging and answering a comment, which their clusters list.
const clusterAgents: Write<typeof Agent>[] = [
  { id: 'merge:m', title: 'Merge Drop the legacy room service client', pullRequestUrls: [urlOf(merging)] },
  { id: 'answer:c', title: 'Answer davidzhao on livekit/agent-gateway#388', pullRequestUrls: [urlOf(working)] },
];

// An investigation whose pull request merged, so it acts on no open pull request.
const orphan: Write<typeof Agent> = {
  id: 'investigation-merged',
  title: 'Investigate e2e on agents#4790',
  pullRequestUrls: ['https://github.com/livekit/agents/pull/4790'],
  state: 'AGENT_STATE_WAITING',
  reply: 'The e2e failure was a flaky TURN server; it passed on rerun before the merge.',
  replyHtml: '<p dir="auto">The e2e failure was a flaky TURN server; it passed on rerun before the merge.</p>',
};

const noop = () => {};

// Each cluster holding the agents acting on its pull requests, as the server writes it.
const holding = (agents: readonly Write<typeof Agent>[]): Write<typeof Cluster>[] =>
  clusters.map((c) => ({
    ...c,
    agents: agents.filter((a) => c.pullRequests?.some((pr) => a.pullRequestUrls?.includes(pr.url ?? ''))),
  }));

// The board as the app shows it, read from a database holding the clusters and agents.
function Desktop({ initial, agents = false }: { initial?: Write<typeof Cluster>; agents?: boolean }) {
  const db = useMemo(() => {
    const all = agents ? [...followed, ...clusterAgents] : clusterAgents;
    return new MemoryDb({ clusters: holding(all), agents: agents ? [orphan] : [] });
  }, [agents]);
  return (
    <DbContext value={db}>
      <Shown initial={initial && urlOf(initial)} />
    </DbContext>
  );
}

function Shown({ initial }: { initial?: string }) {
  const { clusters, agents } = useDb(shown);
  const [openUrl, setOpenUrl] = useState(initial);
  const [openAgent, setOpenAgent] = useState<string>();
  const opened = clusters.find((c) => clusterKey(c) === openUrl);
  const agent = [...agents, ...clusters.flatMap((c) => c.agents)].find((a) => a.id === openAgent);
  return (
    <View style={{ height: '100vh' as unknown as number }}>
      <Board
        clusters={clusters}
        agents={agents}
        openUrl={agent ? undefined : openUrl}
        openAgent={openAgent}
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
              onInvestigate={noop}
              onMessage={() => Promise.resolve()}
            />
          )
        )}
      </Board>
    </View>
  );
}

const meta: Meta = { title: 'Board' };
export default meta;

export const Clusters: StoryObj = { render: () => <Desktop /> };
export const WithAgents: StoryObj = { render: () => <Desktop agents /> };
export const ReadyToMergeOpen: StoryObj = { render: () => <Desktop initial={ready1} /> };
export const NeedsYouOpen: StoryObj = { render: () => <Desktop initial={needsYou} /> };
// Agents waiting on the user, listed in the drawer of the cluster they wait in.
export const NeedsYouWithAgentsOpen: StoryObj = { render: () => <Desktop initial={needsYou} agents /> };
export const BehindOpen: StoryObj = { render: () => <Desktop initial={behind} /> };
