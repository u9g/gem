import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { useMemo } from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';

import { ClusterView } from '../../ClusterView';
import { Legend, PhoneClusterRow, section, sections, SectionTitle } from '../../ClusterRow';
import { DbContext, useDb, type Write } from '../../db';
import { MemoryDb } from '../../memory';
import { Agent, Cluster as ClusterShape, clusterKey, Proposal, PullRequest, shown } from '../../shapes';
import { clusterMergeBy, mergeBy, proposalInputs as proposals, stored } from '../fixtures';

// Made-up clusters, one in each place a phone lists them. Stories render at the window's width,
// so view them at a phone's.

const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000);

type Fields = {
  approved?: boolean;
  // The user is an admin of the repository.
  admin?: boolean;
  checksPassing?: boolean;
  checksFailing?: boolean;
  checks?: { total: number; done: number; failed: number };
  proposals?: Write<typeof Proposal>[];
  merging?: boolean;
};

const pullRequest = (name: string, number: number, title: string, { admin, ...fields }: Fields): Write<typeof PullRequest> => ({
    repository: { owner: 'livekit', name },
    number,
    title,
    url: `https://github.com/livekit/${name}/pull/${number}`,
    checks: { total: 14, done: 14, failed: 0 },
    mergeBy: mergeBy({ admin, ...fields }),
    ...fields,
  });

const cluster = (
  name: string,
  minutes: number,
  pullRequests: Write<typeof PullRequest>[],
  agents: Write<typeof Agent>[] = [],
): Write<typeof ClusterShape> => ({
  name,
  updatedAt: minutesAgo(minutes),
  pullRequests,
  mergeBy: clusterMergeBy(pullRequests),
  agents,
});

const passing = { approved: true, checksPassing: true };

const needsYou = cluster('Retry TTS once on a dropped stream', 5, [
  pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', {
    checksFailing: true,
    checks: { total: 14, done: 14, failed: 2 },
    proposals,
  }),
  pullRequest('agents-js', 1290, 'feat(voice): retry TTS once on a dropped stream', passing),
]);
const ready = cluster('Respect the configured S3 region', 180, [
  pullRequest('protocol', 1103, 'Add region to S3 upload options', passing),
  pullRequest('egress', 902, 'Upload to the configured S3 region', passing),
]);
const forceable = cluster('Respect the configured S3 region', 180, [
  pullRequest('protocol', 1103, 'Add region to S3 upload options', passing),
  pullRequest('egress', 902, 'Upload to the configured S3 region', { checksPassing: true, admin: true }),
]);
// The agents merging and answering a comment, which their clusters list.
const merging = cluster(
  'Respect the configured S3 region',
  1,
  [
    pullRequest('protocol', 1103, 'Add region to S3 upload options', { ...passing, merging: true }),
    pullRequest('egress', 902, 'Upload to the configured S3 region', { ...passing, merging: true }),
  ],
  [
    {
      id: 'merge:m',
      title: 'Merge Respect the configured S3 region',
      pullRequestUrls: ['https://github.com/livekit/protocol/pull/1103', 'https://github.com/livekit/egress/pull/902'],
    },
  ],
);
const working = cluster(
  'Hedge gateway requests',
  120,
  [pullRequest('agent-gateway', 388, 'Hedge requests after the p95', { checks: { total: 14, done: 9, failed: 0 } })],
  [
    {
      id: 'answer:c',
      title: 'Answer davidzhao on livekit/agent-gateway#388',
      pullRequestUrls: ['https://github.com/livekit/agent-gateway/pull/388'],
    },
  ],
);
const running = cluster('Type room metadata updates', 40, [
  pullRequest('client-sdk-js', 1612, 'Type room metadata updates', { checks: { total: 22, done: 15, failed: 0 } }),
]);
const reviewing = cluster('Pin agent version on dispatch', 22, [
  pullRequest('agents', 4810, 'Pin agent version on dispatch', { checksPassing: true }),
]);

// The list as the app shows it, read from a database holding `clusters`.
function Home({ clusters }: { clusters: Write<typeof ClusterShape>[] }) {
  const db = useMemo(() => new MemoryDb({ clusters }), [clusters]);
  return (
    <DbContext value={db}>
      <List />
    </DbContext>
  );
}

function List() {
  const { clusters } = useDb(shown);
  const listed = sections
    .map((name) => ({ name, clusters: clusters.filter((c) => section(c) === name) }))
    .filter((s) => s.clusters.length > 0);
  return (
    <ScrollView style={styles.page} stickyHeaderIndices={[1]}>
      {listed.flatMap(({ name, clusters }, i) => [
        <View key={name} style={styles.section}>
          <SectionTitle>{name}</SectionTitle>
        </View>,
        ...(i === 0 ? [<Legend key="legend" />] : []),
        ...clusters.map((c) => <PhoneClusterRow key={clusterKey(c)} cluster={c} onPress={() => {}} />),
      ])}
    </ScrollView>
  );
}

function Cluster({ cluster }: { cluster: Write<typeof ClusterShape> }) {
  return (
    <ScrollView style={styles.page} contentContainerStyle={styles.content}>
      <ClusterView
        cluster={stored(ClusterShape, cluster)}
        onOpenAgent={() => {}}
        onApprove={() => {}}
        onDismiss={() => {}}
        onRestart={() => {}}
        onRebase={() => {}}
        onRebaseCluster={() => {}}
        onMerge={() => {}}
        onInvestigate={() => {}}
      />
    </ScrollView>
  );
}

const meta: Meta = { title: 'Phone' };
export default meta;

export const ClusterList: StoryObj = {
  render: () => <Home clusters={[needsYou, ready, working, running, reviewing]} />,
};

export const ReadyToMerge: StoryObj = { render: () => <Cluster cluster={ready} /> };

// One pull request is not approved, but the user administers its repository.
export const ForceMergeAsAdmin: StoryObj = { render: () => <Cluster cluster={forceable} /> };

export const Merging: StoryObj = { render: () => <Cluster cluster={merging} /> };

export const MergingInTheList: StoryObj = {
  render: () => <Home clusters={[needsYou, merging, working, running, reviewing]} />,
};

const styles = StyleSheet.create({
  page: {
    flex: 1,
    backgroundColor: '#19212A',
  },
  section: {
    paddingHorizontal: 8,
    paddingTop: 8,
  },
  content: {
    padding: 12,
  },
});
