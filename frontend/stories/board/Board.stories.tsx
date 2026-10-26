import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { useState } from 'react';
import { View } from 'react-native';

import { Board } from '../../Board';
import { ClusterReview } from '../../ClusterFiles';
import { type Proposal, type PullRequest, type PullRequestCluster, PullRequestClusterSchema, PullRequestSchema } from '../../gen/gem_pb';
import { InvestigationView } from '../../InvestigationView';
import { clusterMergeBy, investigations, mergeBy, prFiles, proposals } from '../fixtures';

// Made-up clusters, at least one in each column. Stories render at the window's width, so view
// them at a laptop's.

const minutesAgo = (minutes: number) => timestampFromDate(new Date(Date.now() - minutes * 60_000));

type Fields = {
  approved?: boolean;
  checksPassing?: boolean;
  checksFailing?: boolean;
  behind?: boolean;
  checks?: { total: number; done: number; failed: number };
  proposals?: Proposal[];
  agents?: { threadUrl: string }[];
  merge?: { threadUrl: string };
  files?: { path: string; patch: string }[];
};

const pullRequest = (name: string, number: number, title: string, fields: Fields) =>
  create(PullRequestSchema, {
    repository: { owner: 'livekit', name },
    number,
    title,
    url: `https://github.com/livekit/${name}/pull/${number}`,
    base: 'main',
    checks: { total: 14, done: 14, failed: 0 },
    mergeBy: mergeBy(fields),
    ...fields,
  });

const cluster = (name: string, minutes: number, pullRequests: PullRequest[]) =>
  create(PullRequestClusterSchema, {
    name,
    updatedAt: minutesAgo(minutes),
    pullRequests,
    mergeBy: clusterMergeBy(pullRequests),
  });

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
  pullRequest('server-sdk-go', 540, 'Remove RoomServiceClientV1', { ...passing, merge: { threadUrl: 'https://t3.example/m' } }),
]);
const working = cluster('Hedge gateway requests', 120, [
  pullRequest('agent-gateway', 388, 'Hedge requests after the p95', {
    checks: { total: 14, done: 9, failed: 0 },
    agents: [{ threadUrl: 'https://t3.example/thread/answer' }],
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

const noop = () => {};

function Desktop({ initial, agents = false }: { initial?: PullRequestCluster; agents?: boolean }) {
  const [openUrl, setOpenUrl] = useState(initial?.pullRequests[0].url);
  const [openInvestigation, setOpenInvestigation] = useState<string>();
  const opened = clusters.find((c) => c.pullRequests[0].url === openUrl);
  const investigation = investigations.find((i) => i.id === openInvestigation);
  return (
    <View style={{ height: '100vh' as unknown as number }}>
      <Board
        clusters={clusters}
        investigations={agents ? investigations : []}
        openUrl={investigation ? undefined : openUrl}
        openInvestigation={openInvestigation}
        onOpenCluster={(c) => {
          setOpenInvestigation(undefined);
          setOpenUrl(c.pullRequests[0].url);
        }}
        onOpenInvestigation={setOpenInvestigation}
        onClose={() => {
          setOpenUrl(undefined);
          setOpenInvestigation(undefined);
        }}
        wide={!investigation && opened !== undefined}
      >
        {investigation ? (
          <InvestigationView
            investigation={investigation}
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
export const BehindOpen: StoryObj = { render: () => <Desktop initial={behind} /> };
