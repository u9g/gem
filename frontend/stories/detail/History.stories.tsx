import { create } from '@bufbuild/protobuf';
import { timestampFromMs } from '@bufbuild/protobuf/wkt';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Drawer } from '../../Board';
import { PullRequestHistorySchema } from '../../gen/gem_pb';
import { History } from '../../History';
import { usePhone } from '../../t3';
import type { Cluster, PullRequest } from '../../view-types';
import { blank, clusterMergeBy, mergeBy, stored } from '../fixtures';

// Every open PR's history, opened from the toolbar: in the wide drawer on a laptop, on a screen of
// its own under a back bar on a phone.

const hoursAgo = (hours: number) => Date.now() - hours * 3_600_000;

const pullRequest = (number: number, title: string, base: string, fields: Partial<PullRequest>): PullRequest => ({
  ...blank.pullRequest,
  repository: { owner: 'livekit', name: 'agents' },
  number,
  title,
  url: `https://github.com/livekit/agents/pull/${number}`,
  base,
  checks: { total: 14, done: 14, failed: 0 },
  mergeBy: mergeBy(fields),
  ...fields,
});

const prs = [
  pullRequest(4821, 'feat(voice): retry TTS once on a dropped stream', 'main', { checksFailing: true }),
  pullRequest(4822, 'feat(voice): log each TTS retry attempt', 'u9g/tts-retry', { checksPassing: true, approved: true }),
];
const other = [pullRequest(4840, 'fix(stt): close the websocket when the session ends', 'main', { checksPassing: true })];

const clusters: Cluster[] = [
  stored({ ...blank.cluster, name: 'Retry TTS on a dropped stream', updatedAt: hoursAgo(0.1), pullRequests: prs, mergeBy: clusterMergeBy(prs), agents: [] }),
  stored({ ...blank.cluster, name: 'Close the STT websocket', updatedAt: hoursAgo(1), pullRequests: other, mergeBy: clusterMergeBy(other), agents: [] }),
];

type Event = {
  kind: string;
  hours: number;
  actor?: string;
  state?: string;
  bodyHtml?: string;
  text?: string;
  oid?: string;
  comments?: { path: string; bodyHtml: string; url: string }[];
};

const history = (url: string, events: Event[], error = '') =>
  create(PullRequestHistorySchema, {
    url,
    error,
    events: events.map(({ hours, ...event }) => ({ actor: 'u9g', ...event, at: timestampFromMs(hoursAgo(hours)) })),
  });

const histories = [
  history(prs[0].url, [
    { kind: 'PullRequestCommit', hours: 52, oid: '1ff531d', text: 'feat(voice): retry TTS once on a dropped stream', state: 'SUCCESS' },
    { kind: 'ReviewRequestedEvent', hours: 51, text: 'davidzhao' },
    {
      kind: 'PullRequestReview',
      hours: 30,
      actor: 'davidzhao',
      state: 'CHANGES_REQUESTED',
      bodyHtml: '<p>The retry should back off; a dropped stream usually means the provider is overloaded.</p>',
      comments: [{ path: 'livekit-agents/livekit/agents/voice/tts_retry.py', bodyHtml: '<p>Use <code>RETRY_DELAY</code> here.</p>', url: '' }],
    },
    { kind: 'HeadRefForcePushedEvent', hours: 28, oid: '8c1e2f0', state: 'FAILURE' },
    { kind: 'IssueComment', hours: 27, actor: 'github-actions', bodyHtml: '<p>2 checks failed: <code>lint</code>, <code>tests (3.12)</code>.</p>' },
    { kind: 'PullRequestCommit', hours: 3, oid: 'c883a61', text: 'pin ruff in uv.lock', state: 'PENDING' },
  ]),
  history(prs[1].url, [
    { kind: 'PullRequestCommit', hours: 30, oid: '4d2b9e1', text: 'feat(voice): log each TTS retry attempt', state: 'SUCCESS' },
    { kind: 'PullRequestReview', hours: 6, actor: 'theomonnom', state: 'APPROVED' },
    { kind: 'BaseRefChangedEvent', hours: 2, text: 'main → u9g/tts-retry' },
  ]),
  history(other[0].url, [], 'gh: Resource protected by organization SAML enforcement.'),
];

const load = () => Promise.resolve(histories);
const noop = () => {};

function Frame() {
  const phone = usePhone();
  const view = <History clusters={clusters} load={load} />;
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

const meta: Meta = { title: 'Detail/History' };
export default meta;

export const EveryPullRequest: StoryObj = { render: () => <Frame /> };
