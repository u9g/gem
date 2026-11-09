import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Activity } from '../../Activity';
import { Drawer } from '../../Board';
import { usePhone } from '../../t3';
import type { Cluster, Event, EventKind, PullRequest } from '../../view-types';
import { blank, clusterMergeBy, mergeBy, stored } from '../fixtures';

// The activity of every open PR, as the app shows it: in the wide drawer on a laptop, on a screen
// of its own under a back bar on a phone.

const hoursAgo = (hours: number) => Date.now() - hours * 3_600_000;

let ids = 0;
const event = (kind: EventKind, actor: string, hours: number, fields: Partial<Event> = {}): Event => ({
  id: `event-${ids++}`,
  kind,
  actor,
  at: hoursAgo(hours),
  body: '',
  bodyHtml: '',
  url: '',
  detail: '',
  ...fields,
});

const commit = (actor: string, hours: number, sha: string, message: string) =>
  event('commit', actor, hours, { body: message, detail: sha, url: `https://github.com/livekit/agents/commit/${sha}` });

const pullRequest = (
  number: number,
  title: string,
  base: string,
  fields: Partial<PullRequest> & { admin?: boolean },
): PullRequest => {
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

const prs = [
  pullRequest(4821, 'feat(voice): retry TTS once on a dropped stream', 'main', {
    checksFailing: true,
    checks: { total: 14, done: 14, failed: 2 },
    createdAt: hoursAgo(52),
    events: [
      commit('u9g', 52, '3d723da', 'feat(voice): retry TTS once on a dropped stream'),
      commit('u9g', 52, '9f1c0e2', 'test(voice): cover the retry'),
      event('reviewRequested', 'u9g', 51, { detail: 'theomonnom' }),
      event('comment', 'changeset-bot', 51, {
        body: '### ⚠️ No Changeset found\n\nLatest commit: 9f1c0e2\n\nMerging this PR will not cause a version bump for any packages.',
        bodyHtml:
          '<h3 dir="auto">⚠️ No Changeset found</h3>\n<p dir="auto">Latest commit: 9f1c0e2</p>\n<p dir="auto">Merging this PR will not cause a version bump for any packages.</p>',
        url: 'https://github.com/livekit/agents/pull/4821#issuecomment-1',
      }),
      event('review', 'devin-ai-integration', 50, {
        detail: 'COMMENTED',
        body: '**Devin Review** found 1 potential issue.',
        bodyHtml: '<p dir="auto"><strong>Devin Review</strong> found 1 potential issue.</p>',
        url: 'https://github.com/livekit/agents/pull/4821#pullrequestreview-1',
      }),
      event('reviewComment', 'devin-ai-integration', 50, {
        detail: 'livekit-agents/livekit/agents/voice/tts_retry.py',
        body: '🟡 **Retry ignores the configured interval**\n\nThe retry sleeps a fixed 100 ms instead of `conn_options.retry_interval`.',
        bodyHtml:
          '<p dir="auto">🟡 <strong>Retry ignores the configured interval</strong></p>\n<p dir="auto">The retry sleeps a fixed 100 ms instead of <code class="notranslate">conn_options.retry_interval</code>.</p>',
        url: 'https://github.com/livekit/agents/pull/4821#discussion_r0',
      }),
      event('review', 'theomonnom', 30, {
        detail: 'CHANGES_REQUESTED',
        body: 'Should this retry honor `conn_options.max_retry`? It always retries exactly once.',
        bodyHtml:
          '<p dir="auto">Should this retry honor <code class="notranslate">conn_options.max_retry</code>? It always retries exactly once.</p>',
        url: 'https://github.com/livekit/agents/pull/4821#pullrequestreview-2',
      }),
      commit('u9g', 26, 'c883a61', 'fix(voice): honor max_retry when retrying TTS'),
      event('forcePush', 'u9g', 25, { detail: 'c883a61' }),
      event('renamed', 'u9g', 25, { detail: 'feat(voice): retry TTS once on a dropped stream' }),
      event('review', 'theomonnom', 6, {
        detail: 'APPROVED',
        url: 'https://github.com/livekit/agents/pull/4821#pullrequestreview-3',
      }),
      event('comment', 'davidzhao', 2, {
        body: '#2650 fixes this in the base class, so every TTS gets it. Can we close this one?',
        bodyHtml: '<p dir="auto">#2650 fixes this in the base class, so every TTS gets it. Can we close this one?</p>',
        url: 'https://github.com/livekit/agents/pull/4821#issuecomment-3',
      }),
    ],
  }),
  pullRequest(4822, 'feat(voice): log each TTS retry attempt', 'u9g/tts-retry', {
    checksPassing: true,
    approved: true,
    createdAt: hoursAgo(30),
    events: [
      commit('u9g', 30, 'b6a4cae', 'feat(voice): log each TTS retry attempt'),
      event('readyForReview', 'u9g', 29),
      event('review', 'longcw', 20, { detail: 'APPROVED', url: 'https://github.com/livekit/agents/pull/4822#pullrequestreview-4' }),
    ],
  }),
  pullRequest(4823, 'feat(voice): surface TTS retries in session metrics', 'u9g/tts-retry-log', {
    checksPassing: true,
    behind: true,
    createdAt: hoursAgo(7),
    events: [
      commit('u9g', 7, 'e0f55a9', 'feat(voice): surface TTS retries in session metrics'),
      event('draft', 'u9g', 7),
      event('retargeted', 'u9g', 5, { detail: 'u9g/tts-retry-log' }),
    ],
  }),
];

const cluster: Cluster = stored({
  ...blank.cluster,
  name: 'Retry TTS on a dropped stream',
  updatedAt: hoursAgo(2),
  pullRequests: prs,
  mergeBy: clusterMergeBy(prs),
  agents: [],
});

const noop = () => {};

function Frame() {
  const phone = usePhone();
  const view = <Activity clusters={[cluster]} />;
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

const meta: Meta = { title: 'Detail/Activity' };
export default meta;

export const EveryPullRequest: StoryObj = { render: () => <Frame /> };
