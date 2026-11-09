import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Activity } from '../../Activity';
import { usePhone } from '../../t3';
import type { Cluster, Event, PullRequest } from '../../view-types';
import { blank, clusterMergeBy, mergeBy, stored } from '../fixtures';

// Every open PR's activity, as the app shows it under the bar back to the inbox: a laptop's width
// and a phone's.

const hoursAgo = (hours: number) => Date.now() - hours * 3_600_000;

const event = (kind: Event['kind'], actor: string, hours: number, fields: Partial<Event> = {}): Event => ({
  ...blank.event,
  kind,
  actor,
  at: hoursAgo(hours),
  ...fields,
});

const comment = (author: string, body: string, hours: number, url: string) =>
  event('commented', author, hours, { body, bodyHtml: `<p dir="auto">${body}</p>`, url });

const pullRequest = (
  repo: string,
  number: number,
  title: string,
  hours: number,
  fields: Partial<PullRequest> & { admin?: boolean },
  events: Event[],
): PullRequest => {
  const { admin, ...rest } = fields;
  const url = `https://github.com/livekit/${repo}/pull/${number}`;
  return {
    ...blank.pullRequest,
    repository: { owner: 'livekit', name: repo },
    number,
    title,
    url,
    base: 'main',
    headOid: 'c883a61f0b2a4e6d9c1e8f7a5b3d2c1e0f9a8b7c',
    checks: { total: 14, done: 14, failed: 0 },
    createdAt: hoursAgo(hours),
    mergeBy: mergeBy({ ...rest, admin }),
    events: events.map((e) => ({ ...e, url: e.url || `${url}#event-${e.at}` })),
    ...rest,
  };
};

const retry = pullRequest(
  'agents',
  4821,
  'feat(voice): retry TTS once on a dropped stream',
  52,
  { checksFailing: true, checks: { total: 14, done: 14, failed: 2 } },
  [
    event('committed', 'u9g', 52, { detail: '3d723da feat(voice): retry TTS once on a dropped stream' }),
    event('reviewRequested', 'u9g', 51, { detail: 'theomonnom' }),
    comment(
      'changeset-bot',
      '⚠️ No Changeset found. Latest commit: 3d723da. Merging this PR will not cause a version bump for any packages.',
      51,
      'https://github.com/livekit/agents/pull/4821#issuecomment-1',
    ),
    event('reviewed', 'devin-ai-integration', 50, {
      comments: [
        {
          ...blank.comment,
          id: 'r0',
          author: 'devin-ai-integration',
          body: '🟡 Retry ignores the configured interval',
          bodyHtml:
            '<p dir="auto">🟡 <strong>Retry ignores the configured interval</strong></p><p dir="auto">The retry sleeps a fixed 100 ms instead of <code class="notranslate">conn_options.retry_interval</code>.</p>',
          url: 'https://github.com/livekit/agents/pull/4821#discussion_r0',
          path: 'livekit-agents/livekit/agents/voice/tts_retry.py',
          line: 42,
        },
      ],
    }),
    event('changesRequested', 'theomonnom', 30, {
      body: 'Should this honor `conn_options.max_retry`? It always retries exactly once.',
      bodyHtml:
        '<p dir="auto">Should this honor <code class="notranslate">conn_options.max_retry</code>? It always retries exactly once.</p>',
      url: 'https://github.com/livekit/agents/pull/4821#pullrequestreview-1',
      comments: [
        {
          ...blank.comment,
          id: 'r1',
          author: 'theomonnom',
          body: 'The STT path uses the options, so these two would behave differently.',
          bodyHtml: '<p dir="auto">The STT path uses the options, so these two would behave differently.</p>',
          url: 'https://github.com/livekit/agents/pull/4821#discussion_r1',
          path: 'livekit-agents/livekit/agents/voice/tts_retry.py',
          line: 40,
        },
      ],
    }),
    event('committed', 'u9g', 6, { detail: 'c883a61 fix(voice): honor max_retry when retrying TTS' }),
    event('forcePushed', 'u9g', 5, { detail: 'c883a61' }),
    comment('u9g', 'Rebased and fixed the retry count; checks are running again.', 5, 'https://github.com/livekit/agents/pull/4821#issuecomment-2'),
  ],
);

const websocket = pullRequest(
  'agents',
  4840,
  'fix(stt): close the websocket when the session ends',
  9,
  { checksPassing: true, approved: true },
  [
    event('committed', 'u9g', 9, { detail: '7f0e9d4 fix(stt): close the websocket when the session ends' }),
    event('readyForReview', 'u9g', 8),
    event('reviewRequested', 'u9g', 8, { detail: 'davidzhao' }),
    event('approved', 'davidzhao', 2, {
      body: 'LGTM, thanks for the quick fix.',
      bodyHtml: '<p dir="auto">LGTM, thanks for the quick fix.</p>',
      url: 'https://github.com/livekit/agents/pull/4840#pullrequestreview-2',
    }),
  ],
);

const warmup = pullRequest(
  'agents-js',
  1290,
  'perf(turn-detector): drop the ONNX warmup',
  80,
  { checksPassing: true },
  [
    event('committed', 'u9g', 80, { detail: 'a1b2c3d perf(turn-detector): drop the ONNX warmup' }),
    event('convertedToDraft', 'u9g', 79),
    event('baseChanged', 'u9g', 70, { detail: 'main release/1.2' }),
    event('closed', 'u9g', 60),
    event('reopened', 'u9g', 26),
    event('reviewDismissed', 'u9g', 25, { detail: 'Stale after the rebase' }),
    comment('lukasIO', 'Can we land this before the 1.2 release?', 20, 'https://github.com/livekit/agents-js/pull/1290#issuecomment-3'),
  ],
);

const clusters: Cluster[] = [
  stored({ ...blank.cluster, name: 'Retry TTS on a dropped stream', updatedAt: hoursAgo(5), pullRequests: [retry], mergeBy: clusterMergeBy([retry]) }),
  stored({ ...blank.cluster, name: 'Close the STT websocket', updatedAt: hoursAgo(2), pullRequests: [websocket], mergeBy: clusterMergeBy([websocket]) }),
  stored({ ...blank.cluster, name: 'Drop the ONNX warmup', updatedAt: hoursAgo(20), pullRequests: [warmup], mergeBy: clusterMergeBy([warmup]) }),
];

function Frame() {
  const phone = usePhone();
  return (
    <View style={{ height: '100vh' as unknown as number, backgroundColor: '#19212A' }}>
      <Pressable
        style={{ flexDirection: 'row', alignItems: 'center', height: 44, paddingHorizontal: 8, borderBottomWidth: 1, borderBottomColor: 'rgba(255, 255, 255, 0.06)' }}
      >
        <ChevronLeft size={22} color="#8FA8F5" />
        <Text style={{ color: '#8FA8F5', fontSize: 16 }}>Inbox</Text>
      </Pressable>
      <ScrollView contentContainerStyle={phone ? { padding: 12, paddingBottom: 24 } : { padding: 24, paddingBottom: 48 }}>
        <Activity clusters={clusters} onOpenCluster={() => {}} />
      </ScrollView>
    </View>
  );
}

const meta: Meta = { title: 'Activity/Activity' };
export default meta;

export const AllPullRequests: StoryObj = { render: () => <Frame /> };
