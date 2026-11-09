import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Drawer } from '../../Board';
import { ClusterTimeline } from '../../ClusterTimeline';
import { usePhone } from '../../t3';
import type { Cluster, PullRequest } from '../../view-types';
import { agents, blank, checkProposals, clusterMergeBy, mergeBy, prFiles, proposals, stored } from '../fixtures';

// A cluster opened from the inbox, as the app shows it: in the wide drawer on a laptop, on a screen
// of its own under a back bar on a phone.

const hoursAgo = (hours: number) => Date.now() - hours * 3_600_000;

const pullRequest = (number: number, title: string, base: string, fields: Partial<PullRequest> & { admin?: boolean }): PullRequest => {
  const { admin, ...rest } = fields;
  return {
    ...blank.pullRequest,
    repository: { owner: 'livekit', name: 'agents' },
    number,
    title,
    url: `https://github.com/livekit/agents/pull/${number}`,
    base,
    headOid: 'c883a61f0b2a4e6d9c1e8f7a5b3d2c1e0f9a8b7c',
    checks: { total: 14, done: 14, failed: 0 },
    mergeBy: mergeBy({ ...rest, admin }),
    ...rest,
  };
};

// Three stacked PRs: the base one fails two checks and has review threads with proposed answers,
// the middle one is approved, and the top one is behind its base. Two agents finished, one with a
// reply, and one still works.
const stackPrs = [
  pullRequest(4821, 'feat(voice): retry TTS once on a dropped stream', 'main', {
    checksFailing: true,
    checks: { total: 14, done: 14, failed: 2 },
    files: prFiles,
    createdAt: hoursAgo(52),
    proposals: [proposals[0], proposals[1], proposals[2], checkProposals[0], checkProposals[1], checkProposals[2]],
  }),
  pullRequest(4822, 'feat(voice): log each TTS retry attempt', 'u9g/tts-retry', {
    checksPassing: true,
    approved: true,
    createdAt: hoursAgo(30),
  }),
  pullRequest(4823, 'feat(voice): surface TTS retries in session metrics', 'u9g/tts-retry-log', {
    checksPassing: true,
    behind: true,
    createdAt: hoursAgo(7),
  }),
];

const rich: Cluster = stored({
  ...blank.cluster,
  name: 'Retry TTS on a dropped stream',
  updatedAt: Date.now() - 6 * 60_000,
  pullRequests: stackPrs,
  mergeBy: clusterMergeBy(stackPrs),
  agents: [agents[0], agents[1], agents[2]],
});

const readyPrs = [
  pullRequest(4840, 'fix(stt): close the websocket when the session ends', 'main', {
    checksPassing: true,
    approved: true,
    createdAt: hoursAgo(5),
  }),
];

const ready: Cluster = stored({
  ...blank.cluster,
  name: 'Close the STT websocket on session end',
  updatedAt: Date.now() - 20 * 60_000,
  pullRequests: readyPrs,
  mergeBy: clusterMergeBy(readyPrs),
  agents: [],
});

const noop = () => {};

function Frame({ cluster }: { cluster: Cluster }) {
  const phone = usePhone();
  const view = (
    <ClusterTimeline
      cluster={cluster}
      onOpenAgent={noop}
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
  );
  if (phone) {
    return (
      <View style={{ height: '100vh' as unknown as number, backgroundColor: '#19212A' }}>
        <Pressable
          style={{ flexDirection: 'row', alignItems: 'center', height: 44, paddingHorizontal: 8, borderBottomWidth: 1, borderBottomColor: 'rgba(255, 255, 255, 0.06)' }}
        >
          <ChevronLeft size={22} color="#8FA8F5" />
          <Text style={{ color: '#8FA8F5', fontSize: 16 }}>Clusters</Text>
        </Pressable>
        <ScrollView contentContainerStyle={{ padding: 12, paddingBottom: 24 }}>{view}</ScrollView>
      </View>
    );
  }
  return (
    <View style={{ height: '100vh' as unknown as number, backgroundColor: '#19212A' }}>
      <Drawer wide onClose={noop}>
        {view}
      </Drawer>
    </View>
  );
}

const meta: Meta = { title: 'Detail/Timeline' };
export default meta;

export const Stack: StoryObj = { render: () => <Frame cluster={rich} /> };
export const ReadyToMerge: StoryObj = { render: () => <Frame cluster={ready} /> };
