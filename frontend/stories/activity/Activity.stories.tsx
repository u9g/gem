import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Activity } from '../../Activity';
import { Drawer } from '../../Board';
import { Login } from '../../surfaces';
import { usePhone } from '../../t3';
import type { Cluster, Event, EventKind, PullRequest } from '../../view-types';
import { blank } from '../fixtures';

// The activity across every open PR, as the app shows it: in the wide drawer on a laptop, on a
// screen of its own under a back bar on a phone. Made-up PRs with every kind of event, including
// the user's own, which are hidden until asked for.

const hoursAgo = (hours: number) => Date.now() - hours * 3_600_000;

const event = (kind: EventKind, actor: string, hours: number, body = '', comments = 0): Event => ({
  id: `${kind}-${actor}-${hours}`,
  kind,
  actor,
  at: hoursAgo(hours),
  body,
  url: body ? `https://github.com/livekit/agents/pull/1#issuecomment-${hours}` : '',
  comments,
});

const pullRequest = (repo: string, number: number, title: string, hours: number, events: Event[], approved = false): PullRequest => ({
  ...blank.pullRequest,
  repository: { owner: 'livekit', name: repo },
  number,
  title,
  url: `https://github.com/livekit/${repo}/pull/${number}`,
  base: 'main',
  createdAt: hoursAgo(hours),
  approved,
  events,
});

const prs: PullRequest[] = [
  pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', 52, [
    event('reviewRequested', 'u9g', 51, 'theomonnom'),
    event(
      'comment',
      'devin-ai-integration',
      50,
      '<!-- devin-review-comment -->\n🟡 **Retry ignores the configured interval**\n\nThe retry sleeps a fixed 100 ms instead of `conn_options.retry_interval`.',
    ),
    event('reviewed', 'theomonnom', 30, 'Should this retry honor `conn_options.max_retry`? It always retries exactly once.', 2),
    event('comment', 'u9g', 29, 'Good catch, honoring max_retry now.'),
    event('changesRequested', 'theomonnom', 27, 'Looks right, but the STT path still sleeps a fixed interval. Can you align them before this lands?'),
    event('convertedToDraft', 'u9g', 26),
    event('readyForReview', 'u9g', 5),
    event('approved', 'theomonnom', 2, 'LGTM, thanks for aligning the two paths.'),
  ], true),
  pullRequest('agents-js', 1290, 'feat(voice): retry TTS once on a dropped stream', 30, [
    event('reviewRequested', 'u9g', 30, 'lukasIO'),
    event('reviewed', 'lukasIO', 6, '', 3),
    event('comment', 'lukasIO', 6, 'Left a few nits inline; nothing blocking. The `delay` helper already exists in `utils`, so the new one can go.'),
  ]),
  pullRequest('livekit-cli', 612, 'lk agent simulate: show each scenario as a flow', 9, [
    event('comment', 'theomonnom', 8, 'Fwiw I think we can render both, the simulator version and the agent version, side by side.'),
    event('reviewRequestRemoved', 'davidzhao', 7, 'davidzhao'),
    event('reviewRequested', 'davidzhao', 7, 'theomonnom'),
  ]),
  pullRequest('agents', 4840, 'fix(stt): close the websocket when the session ends', 5, [
    event('approved', 'davidzhao', 1),
  ], true),
  pullRequest('agents', 4850, 'chore: bump the turn detector model', 3, []),
];

const clusters: Cluster[] = [
  { ...blank.cluster, name: 'Retry TTS on a dropped stream', updatedAt: hoursAgo(2), pullRequests: [prs[0], prs[1]] },
  { ...blank.cluster, name: 'Scenario flows', updatedAt: hoursAgo(7), pullRequests: [prs[2]] },
  { ...blank.cluster, name: 'Close the STT websocket', updatedAt: hoursAgo(1), pullRequests: [prs[3]] },
  { ...blank.cluster, updatedAt: hoursAgo(3), pullRequests: [prs[4]] },
];

const noop = () => {};

function Frame({ clusters }: { clusters: Cluster[] }) {
  const phone = usePhone();
  const view = (
    <Login value="u9g">
      <Activity clusters={clusters} onOpenCluster={noop} />
    </Login>
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

const meta: Meta = { title: 'Activity/Activity' };
export default meta;

export const Busy: StoryObj = { render: () => <Frame clusters={clusters} /> };
export const Quiet: StoryObj = { render: () => <Frame clusters={[clusters[3]]} /> };
