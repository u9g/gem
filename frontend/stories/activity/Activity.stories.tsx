import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Activity as ActivityIcon, ChevronLeft } from 'lucide-react';
import { Pressable, ScrollView, Text, View } from 'react-native';

import { Activity } from '../../Activity';
import { t3, usePhone } from '../../t3';
import type { Cluster, Event, EventKind, PullRequest } from '../../view-types';
import { blank, clusterMergeBy, longDiff, mergeBy, stored, twoFileDiff } from '../fixtures';

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
  answering: false,
  trusted: true,
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

const retryUrl = 'https://github.com/livekit/agents/pull/4821';
const retry = pullRequest('agents', 4821, 'feat(voice): retry TTS once on a dropped stream', {
  // The agent's answer to theomonnom's question on a line.
  proposals: [
    stored({
      ...blank.proposal,
      id: 'answer-backoff',
      summary: 'Reply that the retry now backs off 250ms, and resolve',
      summaryHtml: '<p dir="auto">Reply that the retry now backs off 250ms, and resolve</p>',
      pullRequestUrl: retryUrl,
      comment: {
        ...blank.comment,
        id: 'backoff',
        author: 'theomonnom',
        path: 'livekit-agents/livekit/agents/voice/tts_retry.py',
        body: 'Should this back off before the retry? A dropped stream usually means the provider is overloaded.',
        bodyHtml: '<p dir="auto">Should this back off before the retry? A dropped stream usually means the provider is overloaded.</p>',
      },
      threadId: 'thread-backoff',
      reply: 'It does now: f00dcaf backs off 250ms before the retry.\n',
      replyHtml: '<p dir="auto">It does now: f00dcaf backs off 250ms before the retry.</p>',
      resolveThread: true,
    }),
  ],
  checksFailing: true,
  checks: { total: 14, done: 14, failed: 2 },
  createdAt: hoursAgo(52),
  events: [
    event('changesRequested', 'theomonnom', 30, {
      ...said(
        'One question inline, otherwise looks right.',
        '<p dir="auto">One question inline, otherwise looks right.</p>',
      ),
      comments: [
        event('reviewCommented', 'theomonnom', 30, {
          id: 'backoff',
          subject: 'livekit-agents/livekit/agents/voice/tts_retry.py',
          ...said(
            'Should this back off before the retry? A dropped stream usually means the provider is overloaded.',
            '<p dir="auto">Should this back off before the retry? A dropped stream usually means the provider is overloaded.</p>',
          ),
        }),
      ],
    }),
    event('commented', 'u9g', 6, {
      ...said(
        'Added a 250ms backoff. @theomonnom mind taking another look?',
        '<p dir="auto">Added a 250ms backoff. <a href="https://github.com/theomonnom">@theomonnom</a> mind taking another look?</p>',
      ),
    }),
    // An agent was sent it and still works on it.
    event('commented', 'github-actions', 5, {
      answering: true,
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
    event('convertedToDraft', 'u9g', 29, { url: '' }),
    event('readyForReview', 'u9g', 27, { url: '' }),
    event('approved', 'davidzhao', 3, { ...said('LGTM', '<p dir="auto">LGTM</p>') }),
  ],
});

const websocket = pullRequest('python-sdks', 1204, 'fix(stt): close the websocket when the session ends', {
  checksPassing: true,
  createdAt: hoursAgo(80),
  events: [
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
          answering: true,
          ...said('And log the close reason.', '<p dir="auto">And log the close reason.</p>'),
        }),
      ],
    }),
  ],
});

// A third cluster whose events fall inside the others' stretches, so the feed shows a cluster
// broken off by another and picked up again within a day.
const docs = pullRequest('web', 7005, 'docs(simulations): show scenarios as tabbed charts', {
  checksPassing: true,
  createdAt: hoursAgo(28),
  events: [
    event('commented', 'bcherry', 5.5, {
      ...said('Can the tabs keep their place on reload?', '<p dir="auto">Can the tabs keep their place on reload?</p>'),
    }),
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
  stored({ ...blank.cluster, updatedAt: hoursAgo(5.5), pullRequests: [docs], mergeBy: clusterMergeBy([docs]) }),
];

// The feed with two answers that change code: one to a review's question, taller than a laptop's
// screen, and further down one to the first of a review's two comments on lines, shorter than
// one, with the feed going on above, between and below them. Each change is a stage from the
// window's left edge to its right one, across the column the feed is in.
const backoffUrl = 'https://github.com/livekit/agents/pull/4830';
const backoff = pullRequest('agents', 4830, 'fix(voice): back off between TTS retries', {
  proposals: [
    stored({
      ...blank.proposal,
      id: 'stage-backoff',
      summary: 'Honor max_retry and back off between attempts, reply, and resolve',
      summaryHtml: '<p dir="auto">Honor max_retry and back off between attempts, reply, and resolve</p>',
      pullRequestUrl: backoffUrl,
      comment: {
        ...blank.comment,
        id: 'stage-backoff-comment',
        author: 'theomonnom',
        path: 'livekit-agents/livekit/agents/voice/tts_retry.py',
        body: 'This still sleeps a fixed 100 ms and gives up after one attempt. Can it read both from conn_options, like STT does?',
        bodyHtml:
          '<p dir="auto">This still sleeps a fixed 100 ms and gives up after one attempt. Can it read both from <code class="notranslate">conn_options</code>, like STT does?</p>',
      },
      threadId: 'thread-stage-backoff',
      patch: { ...blank.patch, diff: longDiff, commitMessage: 'fix(voice): honor max_retry and back off between TTS retries' },
      reply: 'It does now: the retry count and the wait both come from `conn_options`, and the wait doubles each attempt up to 2 s.\n',
      replyHtml:
        '<p dir="auto">It does now: the retry count and the wait both come from <code class="notranslate">conn_options</code>, and the wait doubles each attempt up to 2 s.</p>',
      resolveThread: true,
    }),
  ],
  checksPassing: true,
  createdAt: hoursAgo(50),
  events: [
    event('commented', 'u9g', 1, {
      ...said('Rebased onto main; the metrics PR can go on top once this lands.', '<p dir="auto">Rebased onto main; the metrics PR can go on top once this lands.</p>'),
    }),
    event('changesRequested', 'theomonnom', 4, {
      ...said('Close. One thing inline before I approve.', '<p dir="auto">Close. One thing inline before I approve.</p>'),
      comments: [
        event('reviewCommented', 'theomonnom', 4, {
          id: 'stage-backoff-comment',
          subject: 'livekit-agents/livekit/agents/voice/tts_retry.py',
          ...said(
            'This still sleeps a fixed 100 ms and gives up after one attempt. Can it read both from conn_options, like STT does?',
            '<p dir="auto">This still sleeps a fixed 100 ms and gives up after one attempt. Can it read both from <code class="notranslate">conn_options</code>, like STT does?</p>',
          ),
        }),
      ],
    }),
    event('readyForReview', 'u9g', 9, { url: '' }),
    event('convertedToDraft', 'u9g', 11, { url: '' }),
  ],
});

const playoutUrl = 'https://github.com/livekit/agents/pull/4835';
const playout = pullRequest('agents', 4835, 'fix(voice): bound the playout wait on close', {
  proposals: [
    stored({
      ...blank.proposal,
      id: 'stage-playout',
      summary: 'Bound the playout wait, cover the retry with tests, reply, and resolve',
      summaryHtml: '<p dir="auto">Bound the playout wait, cover the retry with tests, reply, and resolve</p>',
      pullRequestUrl: playoutUrl,
      comment: {
        ...blank.comment,
        id: 'stage-playout-comment',
        author: 'longcw',
        path: 'livekit-agents/livekit/agents/voice/room_io/room_io.py',
        body: 'This can hang forever if the output never drains. Give it a timeout?',
        bodyHtml: '<p dir="auto">This can hang forever if the output never drains. Give it a timeout?</p>',
      },
      threadId: 'thread-stage-playout',
      patch: { ...blank.patch, diff: twoFileDiff, commitMessage: 'fix(voice): bound the playout wait and cover TTS retries' },
      reply: 'Bounded it with `PLAYOUT_TIMEOUT` and it logs when the wait runs out.\n',
      replyHtml: '<p dir="auto">Bounded it with <code class="notranslate">PLAYOUT_TIMEOUT</code> and it logs when the wait runs out.</p>',
      resolveThread: true,
    }),
  ],
  checksPassing: true,
  createdAt: hoursAgo(70),
  events: [
    event('approved', 'davidzhao', 2, { ...said('LGTM once the timeout is in.', '<p dir="auto">LGTM once the timeout is in.</p>') }),
    event('commented', 'github-actions', 6, {
      answering: true,
      ...said(
        '1 check failed on c883a61: `test (3.12)`.',
        '<p dir="auto">1 check failed on c883a61: <code class="notranslate">test (3.12)</code>.</p>',
      ),
    }),
    event('reviewed', 'longcw', 20, {
      comments: [
        event('reviewCommented', 'longcw', 20, {
          id: 'stage-playout-comment',
          subject: 'livekit-agents/livekit/agents/voice/room_io/room_io.py',
          ...said(
            'This can hang forever if the output never drains. Give it a timeout?',
            '<p dir="auto">This can hang forever if the output never drains. Give it a timeout?</p>',
          ),
        }),
        event('reviewCommented', 'longcw', 20, {
          subject: 'livekit-agents/livekit/agents/voice/room_io/room_io.py',
          ...said('Nit: `disconnect()` can move into a `finally`.', '<p dir="auto">Nit: <code class="notranslate">disconnect()</code> can move into a <code class="notranslate">finally</code>.</p>'),
        }),
      ],
    }),
    event('renamed', 'u9g', 60, { subject: 'fix(voice): bound the playout wait on close', url: '' }),
    event('retargeted', 'u9g', 61, { subject: 'main', url: '' }),
  ],
});

const staged: Cluster[] = [
  stored({
    ...blank.cluster,
    name: 'Back off between TTS retries',
    updatedAt: hoursAgo(1),
    pullRequests: [backoff],
    mergeBy: clusterMergeBy([backoff]),
  }),
  stored({
    ...blank.cluster,
    name: 'Bound the playout wait on close',
    updatedAt: hoursAgo(2),
    pullRequests: [playout],
    mergeBy: clusterMergeBy([playout]),
  }),
  stored({ ...blank.cluster, updatedAt: hoursAgo(5.5), pullRequests: [docs], mergeBy: clusterMergeBy([docs]) }),
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
  const view = <Activity clusters={clusters} onOpenCluster={noop} onApprove={noop} onDismiss={noop} onInvestigate={noop} />;
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
export const Changes: StoryObj = { render: () => <Frame clusters={staged} /> };
export const Empty: StoryObj = {
  render: () => <Frame clusters={[stored({ ...blank.cluster, pullRequests: [pullRequest('agents', 4840, 'chore: bump deps', { events: [] })] })]} />,
};
