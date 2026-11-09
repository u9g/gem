import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Activity as ActivityIcon, ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Activity } from '../../Activity';
import { t3, usePhone } from '../../t3';
import type { Cluster, Event, EventKind, PullRequest } from '../../view-types';
import { blank, clusterMergeBy, mergeBy, stored } from '../fixtures';

// Every open PR's activity, as the app shows it: under the toolbar on a laptop, under a back bar on
// a phone.

const hoursAgo = (hours: number) => Date.now() - hours * 3_600_000;

let ids = 0;
const event = (kind: EventKind, actor: string, hours: number, fields: Partial<Event> = {}): Event => ({
  id: `E${++ids}`,
  kind,
  actor,
  at: hoursAgo(hours),
  url: `https://github.com/livekit/agents/pull/4821#event-${ids}`,
  body: '',
  bodyHtml: '',
  subject: '',
  comments: [],
  ...fields,
});

const said = (markdown: string, html: string) => ({ body: markdown, bodyHtml: html });

const pullRequest = (
  repo: string,
  number: number,
  title: string,
  fields: Partial<PullRequest> & { admin?: boolean },
): PullRequest => {
  const { admin, ...rest } = fields;
  return {
    ...blank.pullRequest,
    repository: { owner: 'livekit', name: repo },
    number,
    title,
    url: `https://github.com/livekit/${repo}/pull/${number}`,
    base: 'main',
    headOid: 'c883a61f0b2a4e6d9c1e8f7a5b3d2c1e0f9a8b7c',
    checks: { total: 14, done: 14, failed: 0 },
    mergeBy: mergeBy({ ...rest, admin }),
    ...rest,
  };
};

const retry = pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', {
  checksFailing: true,
  checks: { total: 14, done: 14, failed: 2 },
  createdAt: hoursAgo(52),
  events: [
    event('committed', 'u9g', 52, { id: 'a1b2c3d4e5f6', body: 'feat(voice): retry TTS once on a dropped stream', url: 'https://github.com/livekit/agents/commit/a1b2c3d' }),
    event('reviewRequested', 'u9g', 51, { subject: 'theomonnom', url: '' }),
    event('changesRequested', 'theomonnom', 30, {
      ...said(
        'One question inline, otherwise looks right.',
        '<p dir="auto">One question inline, otherwise looks right.</p>',
      ),
      comments: [
        event('reviewCommented', 'theomonnom', 30, {
          subject: 'livekit-agents/livekit/agents/voice/tts_retry.py',
          ...said(
            'Should this back off before the retry? A dropped stream usually means the provider is overloaded.',
            '<p dir="auto">Should this back off before the retry? A dropped stream usually means the provider is overloaded.</p>',
          ),
        }),
      ],
    }),
    event('committed', 'u9g', 6, { id: 'f00dcafe1234', body: 'Back off 250ms before the retry', url: 'https://github.com/livekit/agents/commit/f00dcaf' }),
    event('commented', 'u9g', 6, {
      ...said(
        'Added a 250ms backoff. @theomonnom mind taking another look?',
        '<p dir="auto">Added a 250ms backoff. <a href="https://github.com/theomonnom">@theomonnom</a> mind taking another look?</p>',
      ),
    }),
    event('commented', 'github-actions', 5, {
      ...said(
        '2 checks failed on f00dcaf: `lint` and `test (3.12)`.',
        '<p dir="auto">2 checks failed on f00dcaf: <code class="notranslate">lint</code> and <code class="notranslate">test (3.12)</code>.</p>',
      ),
    }),
  ],
});

const log = pullRequest('agents', 4822, 'feat(voice): log each TTS retry attempt', {
  base: 'u9g/tts-retry',
  checksPassing: true,
  approved: true,
  createdAt: hoursAgo(30),
  events: [
    event('committed', 'u9g', 30, { id: '9988776655aa', body: 'feat(voice): log each TTS retry attempt', url: 'https://github.com/livekit/agents/commit/9988776' }),
    event('convertedToDraft', 'u9g', 29, { url: '' }),
    event('readyForReview', 'u9g', 27, { url: '' }),
    event('approved', 'davidzhao', 3, { ...said('LGTM', '<p dir="auto">LGTM</p>') }),
  ],
});

const websocket = pullRequest('python-sdks', 1204, 'fix(stt): close the websocket when the session ends', {
  checksPassing: true,
  createdAt: hoursAgo(80),
  events: [
    event('committed', 'u9g', 80, { id: '1234abcd5678', body: 'fix(stt): close the websocket when the session ends', url: 'https://github.com/livekit/python-sdks/commit/1234abc' }),
    event('renamed', 'u9g', 79, { subject: 'fix(stt): close the websocket when the session ends', url: '' }),
    event('retargeted', 'u9g', 78, { subject: 'main', url: '' }),
    event('reviewed', 'longcw', 40, {
      ...said(
        'Does this also cover the reconnect path? Left no blocking comments.',
        '<p dir="auto">Does this also cover the reconnect path? Left no blocking comments.</p>',
      ),
    }),
    // A review that said nothing of its own, only comments on lines, as most reviews are.
    event('reviewed', 'theomonnom', 20, {
      comments: [
        event('reviewCommented', 'theomonnom', 20, {
          subject: 'livekit/rtc/stt.py',
          ...said('Nit: `aclose()` here too.', '<p dir="auto">Nit: <code class="notranslate">aclose()</code> here too.</p>'),
        }),
        event('reviewCommented', 'theomonnom', 20, {
          subject: 'livekit/rtc/stt.py',
          ...said('And log the close reason.', '<p dir="auto">And log the close reason.</p>'),
        }),
      ],
    }),
    event('forcePushed', 'u9g', 26, { url: 'https://github.com/livekit/python-sdks/commit/deadbee' }),
  ],
});

const stackPrs = [retry, log];
const clusters: Cluster[] = [
  stored({
    ...blank.cluster,
    name: 'Retry TTS on a dropped stream',
    updatedAt: hoursAgo(5),
    pullRequests: stackPrs,
    mergeBy: clusterMergeBy(stackPrs),
  }),
  stored({
    ...blank.cluster,
    name: 'Close the STT websocket on session end',
    updatedAt: hoursAgo(26),
    pullRequests: [websocket],
    mergeBy: clusterMergeBy([websocket]),
  }),
];

const noop = () => {};

function Frame({ clusters }: { clusters: Cluster[] }) {
  const phone = usePhone();
  const view = <Activity clusters={clusters} onOpenCluster={noop} />;
  if (phone) {
    return (
      <View style={{ height: '100vh' as unknown as number, backgroundColor: t3.background }}>
        <Pressable
          style={{ flexDirection: 'row', alignItems: 'center', height: 44, paddingHorizontal: 8, borderBottomWidth: 1, borderBottomColor: t3.border }}
        >
          <ChevronLeft size={22} color="#8FA8F5" />
          <Text style={{ color: '#8FA8F5', fontSize: 16 }}>Inbox</Text>
        </Pressable>
        <ScrollView contentContainerStyle={{ padding: 12, paddingBottom: 24 }}>{view}</ScrollView>
      </View>
    );
  }
  return (
    <View style={{ height: '100vh' as unknown as number, backgroundColor: t3.background }}>
      <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6, padding: 10, borderBottomWidth: 1, borderBottomColor: t3.border }}>
        <Text style={{ color: t3.foreground, fontSize: 11, paddingHorizontal: 8, paddingVertical: 3, borderRadius: 999, backgroundColor: t3.border }}>
          server: connected
        </Text>
        <View
          style={{ flexDirection: 'row', alignItems: 'center', gap: 5, height: 26, paddingHorizontal: 9, borderRadius: 999, borderWidth: 1, borderColor: 'rgba(55, 103, 232, 0.6)', backgroundColor: 'rgba(55, 103, 232, 0.16)' }}
        >
          <ActivityIcon size={13} color={t3.foreground} />
          <Text style={{ color: t3.foreground, fontSize: 12, fontWeight: '600' }}>Activity</Text>
        </View>
      </View>
      <ScrollView contentContainerStyle={{ padding: 24, paddingBottom: 48 }}>{view}</ScrollView>
    </View>
  );
}

const meta: Meta = { title: 'Activity/Feed' };
export default meta;

export const Feed: StoryObj = { render: () => <Frame clusters={clusters} /> };
export const Empty: StoryObj = {
  render: () => <Frame clusters={[stored({ ...blank.cluster, pullRequests: [pullRequest('agents', 4840, 'chore: bump deps', { events: [] })] })]} />,
};
