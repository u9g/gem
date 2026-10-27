import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ScrollView, StyleSheet, View } from 'react-native';

import { ClusterView } from '../../ClusterView';
import { Legend, PhoneClusterRow, section, sections, SectionTitle } from '../../ClusterRow';
import { agentsOf } from '../../agents';
import {
  AgentSchema,
  type Proposal,
  type PullRequest,
  type PullRequestCluster,
  PullRequestClusterSchema,
  PullRequestSchema,
} from '../../gen/gem_pb';
import { clusterMergeBy, mergeBy, proposals } from '../fixtures';

// Made-up clusters, one in each place a phone lists them. Stories render at the window's width,
// so view them at a phone's.

const minutesAgo = (minutes: number) => timestampFromDate(new Date(Date.now() - minutes * 60_000));

type Fields = {
  approved?: boolean;
  // The user is an admin of the repository.
  admin?: boolean;
  checksPassing?: boolean;
  checksFailing?: boolean;
  checks?: { total: number; done: number; failed: number };
  proposals?: Proposal[];
  merging?: boolean;
};

const pullRequest = (name: string, number: number, title: string, { admin, ...fields }: Fields) =>
  create(PullRequestSchema, {
    repository: { owner: 'livekit', name },
    number,
    title,
    url: `https://github.com/livekit/${name}/pull/${number}`,
    checks: { total: 14, done: 14, failed: 0 },
    mergeBy: mergeBy({ admin, ...fields }),
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
const merging = cluster('Respect the configured S3 region', 1, [
  pullRequest('protocol', 1103, 'Add region to S3 upload options', { ...passing, merging: true }),
  pullRequest('egress', 902, 'Upload to the configured S3 region', { ...passing, merging: true }),
]);
const working = cluster('Hedge gateway requests', 120, [
  pullRequest('agent-gateway', 388, 'Hedge requests after the p95', {
    checks: { total: 14, done: 9, failed: 0 },
  }),
]);
const running = cluster('Type room metadata updates', 40, [
  pullRequest('client-sdk-js', 1612, 'Type room metadata updates', { checks: { total: 22, done: 15, failed: 0 } }),
]);
const reviewing = cluster('Pin agent version on dispatch', 22, [
  pullRequest('agents', 4810, 'Pin agent version on dispatch', { checksPassing: true }),
]);

// The agents merging and answering a comment, which their clusters list.
const agents = [
  create(AgentSchema, { id: 'merge:m', title: 'Merge Respect the configured S3 region', pullRequestUrls: merging.pullRequests.map((pr) => pr.url) }),
  create(AgentSchema, { id: 'answer:c', title: 'Answer davidzhao on livekit/agent-gateway#388', pullRequestUrls: [working.pullRequests[0].url] }),
];

function Home({ clusters }: { clusters: PullRequestCluster[] }) {
  const listed = sections
    .map((name) => ({ name, clusters: clusters.filter((c) => section(c, agents) === name) }))
    .filter((s) => s.clusters.length > 0);
  return (
    <ScrollView style={styles.page} stickyHeaderIndices={[1]}>
      {listed.flatMap(({ name, clusters }, i) => [
        <View key={name} style={styles.section}>
          <SectionTitle>{name}</SectionTitle>
        </View>,
        ...(i === 0 ? [<Legend key="legend" />] : []),
        ...clusters.map((c) => <PhoneClusterRow key={c.pullRequests[0].url} cluster={c} onPress={() => {}} />),
      ])}
    </ScrollView>
  );
}

function Cluster({ cluster }: { cluster: PullRequestCluster }) {
  return (
    <ScrollView style={styles.page} contentContainerStyle={styles.content}>
      <ClusterView
        cluster={cluster}
        agents={agentsOf(cluster, agents)}
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
