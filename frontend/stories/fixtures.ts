import { create } from '@bufbuild/protobuf';

import { type Proposal, ProposalSchema, type PullRequest, PullRequestSchema } from '../gen/gem_pb';

// Made-up conversations shaped like real ones: this repository is public.

export const pr = create(PullRequestSchema, {
  repository: { owner: 'livekit', name: 'agents' },
  number: 4821,
  title: 'feat(voice): retry TTS once on a dropped stream',
  url: 'https://github.com/livekit/agents/pull/4821',
});

const diff = `diff --git a/livekit-agents/livekit/agents/voice/tts_retry.py b/livekit-agents/livekit/agents/voice/tts_retry.py
index 3d723da..c883a61 100644
--- a/livekit-agents/livekit/agents/voice/tts_retry.py
+++ b/livekit-agents/livekit/agents/voice/tts_retry.py
@@ -41,10 +41,11 @@ async def synthesize_with_retry(tts, text, *, conn_options):
     try:
         return await tts.synthesize(text, conn_options=conn_options)
     except APIConnectionError:
-        if attempt > 0:
+        if attempt >= conn_options.max_retry:
             raise
-        await asyncio.sleep(0.1)
+        await asyncio.sleep(conn_options.retry_interval)
+        logger.debug("retrying TTS after a dropped stream", extra={"attempt": attempt})
         return await synthesize_with_retry(tts, text, conn_options=conn_options, attempt=attempt + 1)
`;

// bodyHtml is what GitHub's markdown API renders for body.
const comment = (author: string, body: string, bodyHtml: string, anchor: string) => ({
  id: `comment-${anchor}`,
  author,
  body,
  bodyHtml,
  url: `${pr.url}#${anchor}`,
});

const reviewQuestion = comment(
  'theo',
  "Should this retry honor `conn_options.max_retry`? It always retries exactly once.\n\nAsking because the STT path uses the options, so these two would behave differently.",
  '<p dir="auto">Should this retry honor <code class="notranslate">conn_options.max_retry</code>? It always retries exactly once.</p>\n<p dir="auto">Asking because the STT path uses the options, so these two would behave differently.</p>',
  'discussion_r1',
);

export const proposals: Proposal[] = [
  create(ProposalSchema, {
    id: 'patch-reply-resolve',
    summary: 'Use conn_options for the retry count and interval, reply, and resolve',
    pullRequestUrl: pr.url,
    comment: comment(
      'devin-ai-integration',
      '<!-- devin-review-comment {"id": "BUG_0001"} -->\n🟡 **Retry ignores the configured interval**\n\nThe retry in [`synthesize_with_retry`](https://github.com/livekit/agents/blob/main/livekit-agents/livekit/agents/voice/tts_retry.py) sleeps a fixed 100 ms instead of `conn_options.retry_interval`.\n\n<details>\n<summary>Learn more</summary>\n\n`conn_options.retry_interval` defaults to 2 s, so a dropped stream retries 20× faster than configured.\n</details>',
      '\n<p dir="auto">🟡 <strong>Retry ignores the configured interval</strong></p>\n<p dir="auto">The retry in <a href="https://github.com/livekit/agents/blob/main/livekit-agents/livekit/agents/voice/tts_retry.py"><code class="notranslate">synthesize_with_retry</code></a> sleeps a fixed 100 ms instead of <code class="notranslate">conn_options.retry_interval</code>.</p>\n<details>\n<summary>Learn more</summary>\n<p dir="auto"><code class="notranslate">conn_options.retry_interval</code> defaults to 2 s, so a dropped stream retries 20× faster than configured.</p>\n</details>',
      'discussion_r0',
    ),
    threadId: 'thread-0',
    patch: { diff, commitMessage: 'fix(voice): retry TTS with the configured interval' },
    reply: 'Good catch. The retry now sleeps for `conn_options.retry_interval`.\n',
    resolveThread: true,
  }),
  create(ProposalSchema, {
    id: 'alternative-patch',
    summary: 'Honor max_retry like the STT path, and say so',
    pullRequestUrl: pr.url,
    comment: reviewQuestion,
    threadId: 'thread-1',
    patch: { diff, commitMessage: 'fix(voice): honor max_retry when retrying TTS' },
    reply: "Yes, it should. It now retries up to `conn_options.max_retry` times, the same as STT.\n",
  }),
  create(ProposalSchema, {
    id: 'alternative-reply',
    summary: 'Explain why TTS retries once, and keep it',
    pullRequestUrl: pr.url,
    comment: reviewQuestion,
    threadId: 'thread-1',
    reply:
      "On purpose: a TTS retry replays audio the user may have already heard part of, so more than one retry sounds broken. STT retries are silent. I'll add a comment saying so.\n",
  }),
  create(ProposalSchema, {
    id: 'quote-reply',
    summary: 'Quote reply that the changeset is not needed',
    pullRequestUrl: pr.url,
    comment: comment(
      'changeset-bot',
      "### ⚠️ No Changeset found\n\nLatest commit: c883a61\n\nMerging this PR will not cause a version bump for any packages.",
      '<h3 dir="auto"><g-emoji class="g-emoji" alias="warning">⚠️</g-emoji> No Changeset found</h3>\n<p dir="auto">Latest commit: c883a61</p>\n<p dir="auto">Merging this PR will not cause a version bump for any packages.</p>',
      'issuecomment-1',
    ),
    reply: 'No release needed: this only changes an internal retry.\n',
  }),
  create(ProposalSchema, {
    id: 'tool-request',
    summary: 'slack_message: ask a channel a question',
    pullRequestUrl: pr.url,
    comment: comment(
      'adrian',
      'Is the voice team OK with retries changing the latency budget?',
      '<p dir="auto">Is the voice team OK with retries changing the latency budget?</p>',
      'issuecomment-2',
    ),
    requestedTool:
      "Post a question to a Slack channel and wait for the answer. The voice team owns the latency budget, so they should answer this, not the PR author.\n",
  }),
  create(ProposalSchema, {
    id: 'failed',
    summary: 'Log the retry attempt',
    pullRequestUrl: pr.url,
    comment: comment(
      'copilot-pull-request-reviewer',
      'Consider logging retries so they show up in traces.',
      '<p dir="auto">Consider logging retries so they show up in traces.</p>',
      'discussion_r2',
    ),
    threadId: 'thread-2',
    patch: { diff, commitMessage: 'chore(voice): log TTS retries' },
    reply: 'Added a debug log for each retry.\n',
    resolveThread: true,
    error: 'git: error: patch failed: livekit-agents/livekit/agents/voice/tts_retry.py:41\nerror: patch does not apply',
  }),
];

export type Item = { pr: PullRequest; proposal: Proposal };
export const items: Item[] = proposals.map((proposal) => ({ pr, proposal }));
