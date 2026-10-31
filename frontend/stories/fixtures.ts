import type { Node } from '../db';
import { type Input, MemoryDb } from '../memory';
import type { Agent, MergeBy, Proposal, PullRequest } from '../shapes';

// What the server would write as a made-up PR's `mergeBy`, and its cluster's.
export function mergeBy(pr: { approved?: boolean; checksPassing?: boolean; admin?: boolean }): MergeBy {
  if (!pr.checksPassing) return 'MERGE_BY_UNSPECIFIED';
  if (pr.approved) return 'MERGE_BY_APPROVAL';
  return pr.admin ? 'MERGE_BY_ADMIN' : 'MERGE_BY_UNSPECIFIED';
}

export function clusterMergeBy(prs: readonly { mergeBy?: MergeBy }[]): MergeBy {
  if (prs.some((pr) => (pr.mergeBy ?? 'MERGE_BY_UNSPECIFIED') === 'MERGE_BY_UNSPECIFIED')) return 'MERGE_BY_UNSPECIFIED';
  return prs.some((pr) => pr.mergeBy === 'MERGE_BY_ADMIN') ? 'MERGE_BY_ADMIN' : 'MERGE_BY_APPROVAL';
}

// Values written to a database of their own, for stories of a component that takes one value.
const alone = new MemoryDb();
const kept = new WeakMap<object, unknown>();

// `value`, stored as the server stores it, the same node each time it's asked for.
export function stored<S>(value: Input<S>): Node<S> {
  let node = kept.get(value as object) as Node<S> | undefined;
  if (!node) kept.set(value as object, (node = alone.put<S>(value)));
  return node;
}

// A time `seconds` before now.
export const ago = (seconds: number) => new Date(Date.now() - seconds * 1000);

// Made-up conversations shaped like real ones: this repository is public.

export const pr = {
  repository: { owner: 'livekit', name: 'agents' },
  number: 4821,
  title: 'feat(voice): retry TTS once on a dropped stream',
  url: 'https://github.com/livekit/agents/pull/4821',
} satisfies Input<PullRequest>;

const diff = `diff --git a/livekit-agents/livekit/agents/voice/tts_retry.py b/livekit-agents/livekit/agents/voice/tts_retry.py
index 3d723da..c883a61 100644
--- a/livekit-agents/livekit/agents/voice/tts_retry.py
+++ b/livekit-agents/livekit/agents/voice/tts_retry.py
@@ -41,7 +41,8 @@ async def synthesize_with_retry(tts, text, *, conn_options):
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

// The same fix on the agents-js port of the PR.
const jsDiff = `diff --git a/agents/src/voice/tts_retry.ts b/agents/src/voice/tts_retry.ts
index 5be1c2a..7f0e9d4 100644
--- a/agents/src/voice/tts_retry.ts
+++ b/agents/src/voice/tts_retry.ts
@@ -38,6 +38,6 @@ export async function synthesizeWithRetry(
   } catch (error) {
     if (!(error instanceof APIConnectionError) || attempt >= connOptions.maxRetry) throw error;
-    await delay(100);
+    await delay(connOptions.retryIntervalMs);
     return synthesizeWithRetry(tts, text, connOptions, attempt + 1);
   }
 }
`;

// bodyHtml and replyHtml are what GitHub's markdown API renders for body and reply.
const comment = (author: string, body: string, bodyHtml: string, anchor: string, line = 0) => ({
  id: `comment-${anchor}`,
  author,
  body,
  bodyHtml,
  url: `${pr.url}#${anchor}`,
  // Review comments sit on a line of the PR's diff, ending GitHub's hunk.
  ...(line && {
    path: 'livekit-agents/livekit/agents/voice/tts_retry.py',
    line,
    diffHunk: prHunk.split('\n').slice(0, line - 32).join('\n'),
  }),
});

// The PR's own hunk the review comments are on, from line 34 of the new file.
const prHunk = `@@ -30,6 +34,14 @@ from .errors import APIConnectionError
 
 async def synthesize_with_retry(tts, text, *, conn_options, attempt=0):
+    \"\"\"Synthesizes text, retrying once when the stream drops.\"\"\"
+    try:
+        return await tts.synthesize(text, conn_options=conn_options)
+    except APIConnectionError:
+        if attempt > 0:
+            raise
+        await asyncio.sleep(0.1)
+        return await synthesize_with_retry(tts, text, conn_options=conn_options, attempt=attempt + 1)`;

// The PR's diff of the file the review comments are on.
export const prFiles = [{ path: 'livekit-agents/livekit/agents/voice/tts_retry.py', patch: `@@ -1,4 +1,5 @@
 import asyncio
+import logging
 
 from .errors import APIConnectionError
 
${prHunk}
 
 
 async def stream_with_retry(tts, text, *, conn_options):` }];

const reviewQuestion = comment(
  'theo',
  "Should this retry honor `conn_options.max_retry`? It always retries exactly once.\n\nAsking because the STT path uses the options, so these two would behave differently.",
  '<p dir="auto">Should this retry honor <code class="notranslate">conn_options.max_retry</code>? It always retries exactly once.</p>\n<p dir="auto">Asking because the STT path uses the options, so these two would behave differently.</p>',
  'discussion_r1',
  40,
);

// As the server writes them, and as the components read them.
export const proposalInputs: Input<Proposal>[] = [
  {
    id: 'patch-reply-resolve',
    summary: 'Use conn_options for the retry count and interval, reply, and resolve',
    summaryHtml: '<p dir="auto">Use conn_options for the retry count and interval, reply, and resolve</p>',
    pullRequestUrl: pr.url,
    comment: comment(
      'devin-ai-integration',
      '<!-- devin-review-comment {"id": "BUG_0001"} -->\n🟡 **Retry ignores the configured interval**\n\nThe retry in [`synthesize_with_retry`](https://github.com/livekit/agents/blob/main/livekit-agents/livekit/agents/voice/tts_retry.py) sleeps a fixed 100 ms instead of `conn_options.retry_interval`.\n\n<details>\n<summary>Learn more</summary>\n\n`conn_options.retry_interval` defaults to 2 s, so a dropped stream retries 20× faster than configured.\n</details>',
      '\n<p dir="auto">🟡 <strong>Retry ignores the configured interval</strong></p>\n<p dir="auto">The retry in <a href="https://github.com/livekit/agents/blob/main/livekit-agents/livekit/agents/voice/tts_retry.py"><code class="notranslate">synthesize_with_retry</code></a> sleeps a fixed 100 ms instead of <code class="notranslate">conn_options.retry_interval</code>.</p>\n<details>\n<summary>Learn more</summary>\n<p dir="auto"><code class="notranslate">conn_options.retry_interval</code> defaults to 2 s, so a dropped stream retries 20× faster than configured.</p>\n</details>',
      'discussion_r0',
      42,
    ),
    threadId: 'thread-0',
    patch: { diff, commitMessage: 'fix(voice): retry TTS with the configured interval' },
    reply: 'Good catch. The retry now sleeps for `conn_options.retry_interval`.\n',
    replyHtml: "<p dir=\"auto\">Good catch. The retry now sleeps for <code class=\"notranslate\">conn_options.retry_interval</code>.</p>",
    resolveThread: true,
  },
  {
    id: 'alternative-patch',
    summary: 'Honor max_retry like the STT path, and say so',
    summaryHtml: '<p dir="auto">Honor max_retry like the STT path, and say so</p>',
    pullRequestUrl: pr.url,
    comment: reviewQuestion,
    threadId: 'thread-1',
    patch: { diff, commitMessage: 'fix(voice): honor max_retry when retrying TTS' },
    reply: "Yes, it should. It now retries up to `conn_options.max_retry` times, the same as STT.\n",
    replyHtml: "<p dir=\"auto\">Yes, it should. It now retries up to <code class=\"notranslate\">conn_options.max_retry</code> times, the same as STT.</p>",
  },
  {
    id: 'alternative-reply',
    summary: 'Explain why TTS retries once, and keep it',
    summaryHtml: '<p dir="auto">Explain why TTS retries once, and keep it</p>',
    pullRequestUrl: pr.url,
    comment: reviewQuestion,
    threadId: 'thread-1',
    reply:
      "On purpose: a TTS retry replays audio the user may have already heard part of, so more than one retry sounds broken. STT retries are silent. I'll add a comment saying so.\n",
    replyHtml: "<p dir=\"auto\">On purpose: a TTS retry replays audio the user may have already heard part of, so more than one retry sounds broken. STT retries are silent. I'll add a comment saying so.</p>",
  },
  {
    id: 'quote-reply',
    summary: 'Quote reply that the changeset is not needed',
    summaryHtml: '<p dir="auto">Quote reply that the changeset is not needed</p>',
    pullRequestUrl: pr.url,
    comment: comment(
      'changeset-bot',
      "### ⚠️ No Changeset found\n\nLatest commit: c883a61\n\nMerging this PR will not cause a version bump for any packages.",
      '<h3 dir="auto"><g-emoji class="g-emoji" alias="warning">⚠️</g-emoji> No Changeset found</h3>\n<p dir="auto">Latest commit: c883a61</p>\n<p dir="auto">Merging this PR will not cause a version bump for any packages.</p>',
      'issuecomment-1',
    ),
    reply:
      '> ### ⚠️ No Changeset found\n>\n> Latest commit: c883a61\n>\n> Merging this PR will not cause a version bump for any packages.\n\nNo release needed: this only changes an internal retry.\n',
    replyHtml:
      '<blockquote>\n<h3 dir="auto"><g-emoji class="g-emoji" alias="warning">⚠️</g-emoji> No Changeset found</h3>\n<p dir="auto">Latest commit: c883a61</p>\n<p dir="auto">Merging this PR will not cause a version bump for any packages.</p>\n</blockquote>\n<p dir="auto">No release needed: this only changes an internal retry.</p>',
  },
  {
    id: 'close',
    summary: 'Close this PR in favor of #2650',
    summaryHtml: '<p dir="auto">Close this PR in favor of <a class="issue-link js-issue-link" data-error-text="Failed to load title" data-id="3160627175" data-permission-text="Title is private" data-url="https://github.com/livekit/agents/issues/2650" data-hovercard-type="issue" data-hovercard-url="/livekit/agents/issues/2650/hovercard" href="https://github.com/livekit/agents/issues/2650">#2650</a></p>',
    pullRequestUrl: pr.url,
    comment: comment(
      'davidzhao',
      '#2650 fixes this in the base class, so every TTS gets it. Can we close this one?',
      '<p dir="auto">#2650 fixes this in the base class, so every TTS gets it. Can we close this one?</p>',
      'issuecomment-3',
    ),
    reply: 'Agreed, #2650 covers it. Closing.\n',
    replyHtml: '<p dir="auto">Agreed, #2650 covers it. Closing.</p>',
    closePullRequest: true,
  },
  {
    id: 'tool-request',
    summary: 'slack_message: ask a channel a question',
    summaryHtml: '<p dir="auto">slack_message: ask a channel a question</p>',
    pullRequestUrl: pr.url,
    comment: comment(
      'adrian',
      'Is the voice team OK with retries changing the latency budget?',
      '<p dir="auto">Is the voice team OK with retries changing the latency budget?</p>',
      'issuecomment-2',
    ),
    requestedTool:
      "Post a question to a Slack channel and wait for the answer. The voice team owns the latency budget, so they should answer this, not the PR author.\n",
  },
  {
    id: 'failed',
    summary: 'Log the retry attempt',
    summaryHtml: '<p dir="auto">Log the retry attempt</p>',
    pullRequestUrl: pr.url,
    comment: comment(
      'copilot-pull-request-reviewer',
      'Consider logging retries so they show up in traces.',
      '<p dir="auto">Consider logging retries so they show up in traces.</p>',
      'discussion_r2',
      43,
    ),
    threadId: 'thread-2',
    patch: { diff, commitMessage: 'chore(voice): log TTS retries' },
    reply: 'Added a debug log for each retry.\n',
    replyHtml: "<p dir=\"auto\">Added a debug log for each retry.</p>",
    resolveThread: true,
    error: 'git: error: patch failed: livekit-agents/livekit/agents/voice/tts_retry.py:41\nerror: patch does not apply',
  },
  {
    id: 'other-patches',
    summary: 'Retry TTS with the configured interval here and in the agents-js port, reply, and resolve',
    summaryHtml:
      '<p dir="auto">Retry TTS with the configured interval here and in the agents-js port, reply, and resolve</p>',
    pullRequestUrl: pr.url,
    comment: comment(
      'devin-ai-integration',
      'The agents-js port of this retry has the same fixed 100 ms sleep.',
      '<p dir="auto">The agents-js port of this retry has the same fixed 100 ms sleep.</p>',
      'discussion_r3',
      43,
    ),
    threadId: 'thread-3',
    patch: { diff, commitMessage: 'fix(voice): retry TTS with the configured interval' },
    otherPatches: [
      {
        pullRequestUrl: 'https://github.com/livekit/agents-js/pull/1907',
        diff: jsDiff,
        commitMessage: 'fix(voice): retry TTS with the configured interval',
      },
    ],
    reply: 'Fixed here, and the same fix goes into the agents-js port (livekit/agents-js#1907).\n',
    replyHtml: '<p dir="auto">Fixed here, and the same fix goes into the agents-js port (livekit/agents-js#1907).</p>',
    resolveThread: true,
  },
];

export type Item = { pr: Node<PullRequest>; proposal: Node<Proposal> };
// A reply quoting code by permalink, which GitHub renders as the lines it points at.
const permalinkInputs: Input<Proposal>[] = [
  {
    ...proposalInputs[0],
    id: 'permalink-reply',
    reply: "This isn't new with this PR. The parent is re-watched in the awaiter's `finally` on main too:\nhttps://github.com/livekit/agents/blob/74e614109616ef78627dcecf5a8b0c33d4db5dfd/livekit-agents/livekit/agents/voice/agent.py#L1084-L1088\n\nSo a parent that parks holds the run open:\n\n```py\nawait session.run(user_input=\"hi\")\n```\n",
    replyHtml: "<p dir=\"auto\">This isn't new with this PR. The parent is re-watched in the awaiter's <code class=\"notranslate\">finally</code> on main too:<br>\n</p><div class=\"Box Box--condensed my-2\">\n  <div class=\"Box-header f6\">\n    <p class=\"mb-0 text-bold\">\n      <a href=\"https://github.com/livekit/agents/blob/74e614109616ef78627dcecf5a8b0c33d4db5dfd/livekit-agents/livekit/agents/voice/agent.py#L1084-L1088\">agents/livekit-agents/livekit/agents/voice/agent.py</a>\n    </p>\n    <p class=\"mb-0 color-fg-muted\">\n        Lines 1084 to 1088\n      in\n      <a data-pjax=\"true\" class=\"commit-tease-sha Link--inTextBlock\" href=\"/livekit/agents/commit/74e614109616ef78627dcecf5a8b0c33d4db5dfd\">74e6141</a>\n    </p>\n  </div>\n  <div itemprop=\"text\" class=\"Box-body p-0 blob-wrapper blob-wrapper-embedded data\">\n    <table class=\"highlight tab-size mb-0 js-file-line-container\" data-tab-size=\"8\" data-paste-markdown-skip=\"\">\n\n        <tbody><tr class=\"border-0\">\n          <td id=\"L1084\" class=\"blob-num border-0 tmp-px-3 py-0 color-bg-default\" data-line-number=\"1084\"></td>\n          <td id=\"LC1084\" class=\"blob-code border-0 tmp-px-3 py-0 color-bg-default blob-code-inner js-file-line\"> <span class=\"pl-c\"># re-watch the suspended handles so the resumed parent activity</span> </td>\n        </tr>\n\n        <tr class=\"border-0\">\n          <td id=\"L1085\" class=\"blob-num border-0 tmp-px-3 py-0 color-bg-default\" data-line-number=\"1085\"></td>\n          <td id=\"LC1085\" class=\"blob-code border-0 tmp-px-3 py-0 color-bg-default blob-code-inner js-file-line\"> <span class=\"pl-c\"># is tracked by the current RunResult again</span> </td>\n        </tr>\n\n        <tr class=\"border-0\">\n          <td id=\"L1086\" class=\"blob-num border-0 tmp-px-3 py-0 color-bg-default\" data-line-number=\"1086\"></td>\n          <td id=\"LC1086\" class=\"blob-code border-0 tmp-px-3 py-0 color-bg-default blob-code-inner js-file-line\"> <span class=\"pl-k\">if</span> <span class=\"pl-s1\">run_state</span> <span class=\"pl-c1\">and</span> <span class=\"pl-c1\">not</span> <span class=\"pl-s1\">run_state</span>.<span class=\"pl-c1\">done</span>(): </td>\n        </tr>\n\n        <tr class=\"border-0\">\n          <td id=\"L1087\" class=\"blob-num border-0 tmp-px-3 py-0 color-bg-default\" data-line-number=\"1087\"></td>\n          <td id=\"LC1087\" class=\"blob-code border-0 tmp-px-3 py-0 color-bg-default blob-code-inner js-file-line\">     <span class=\"pl-k\">for</span> <span class=\"pl-s1\">handle</span> <span class=\"pl-c1\">in</span> [<span class=\"pl-c1\">*</span><span class=\"pl-s1\">suspended_handles</span>, <span class=\"pl-s1\">current_task</span>]: </td>\n        </tr>\n\n        <tr class=\"border-0\">\n          <td id=\"L1088\" class=\"blob-num border-0 tmp-px-3 py-0 color-bg-default\" data-line-number=\"1088\"></td>\n          <td id=\"LC1088\" class=\"blob-code border-0 tmp-px-3 py-0 color-bg-default blob-code-inner js-file-line\">         <span class=\"pl-s1\">run_state</span>.<span class=\"pl-c1\">_watch_handle</span>(<span class=\"pl-s1\">handle</span>) </td>\n        </tr>\n    </tbody></table>\n  </div>\n</div>\n<p></p>\n<p dir=\"auto\">So a parent that parks holds the run open:</p>\n<div class=\"highlight highlight-source-python\" dir=\"auto\"><pre class=\"notranslate\"><span class=\"pl-k\">await</span> <span class=\"pl-s1\">session</span>.<span class=\"pl-c1\">run</span>(<span class=\"pl-s1\">user_input</span><span class=\"pl-c1\">=</span><span class=\"pl-s\">\"hi\"</span>)</pre></div>",
  },
];

export const proposals = proposalInputs.map((proposal) => stored<Proposal>(proposal));
export const permalinkProposals = permalinkInputs.map((proposal) => stored<Proposal>(proposal));
export const items: Item[] = proposals.map((proposal) => ({ pr: stored<PullRequest>(pr), proposal }));

const failure = {
  id: 'c883a61,check-ruff,check-tests',
  headOid: 'c883a61f0b2a4e6d9c1e8f7a5b3d2c1e0f9a8b7c',
};

const ruff = {
  id: 'check-ruff',
  name: 'ruff',
  conclusion: 'FAILURE',
  url: 'https://github.com/livekit/agents/actions/runs/1/job/1',
  workflowRunId: 1n,
  excerpt:
    'livekit/agents/voice/tts_retry.py:2:8: F401 [*] `time` imported but unused\nFound 1 error.\n[*] 1 fixable with the `--fix` option.\n',
};

const tests = {
  id: 'check-tests',
  name: 'tests (3.12)',
  conclusion: 'TIMED_OUT',
  url: 'https://github.com/livekit/agents/actions/runs/2/job/2',
  workflowRunId: 2n,
  excerpt:
    'tests/test_room_io.py::test_close_flushes_audio\nError: The job running on runner GitHub Actions 14 has exceeded the maximum execution time of 30 minutes.\n',
};

const lintDiff = `diff --git a/livekit-agents/livekit/agents/voice/tts_retry.py b/livekit-agents/livekit/agents/voice/tts_retry.py
index c883a61..d41f0e2 100644
--- a/livekit-agents/livekit/agents/voice/tts_retry.py
+++ b/livekit-agents/livekit/agents/voice/tts_retry.py
@@ -1,6 +1,5 @@
 import asyncio
-import time
 
 from ..log import logger
 from ..types import APIConnectOptions
`;

const flushDiff = `diff --git a/livekit-agents/livekit/agents/voice/room_io/room_io.py b/livekit-agents/livekit/agents/voice/room_io/room_io.py
index 5a1c2e0..9b7d4f3 100644
--- a/livekit-agents/livekit/agents/voice/room_io/room_io.py
+++ b/livekit-agents/livekit/agents/voice/room_io/room_io.py
@@ -212,7 +212,7 @@ class RoomIO:
     async def _on_session_close(self, ev: CloseEvent) -> None:
-        await self._audio_output.wait_for_playout()
+        await asyncio.wait_for(self._audio_output.wait_for_playout(), timeout=5)
         await self._room.disconnect()
`;

// Answers to checks that failed on the head commit, rather than to a comment. Each answers only
// some of the failed checks: the lint failure gets a fix, the timed-out tests get two alternatives.
export const checkProposalInputs: Input<Proposal>[] = [
  {
    id: 'checks-patch',
    summary: 'Drop the unused import ruff flags',
    summaryHtml: '<p dir="auto">Drop the unused import ruff flags</p>',
    pullRequestUrl: pr.url,
    checkFailure: { ...failure, checks: [ruff] },
    patch: { diff: lintDiff, commitMessage: 'chore(voice): drop unused import' },
  },
  {
    id: 'checks-flush',
    summary: 'Stop waiting forever for playout on close, which hangs the test',
    summaryHtml: '<p dir="auto">Stop waiting forever for playout on close, which hangs the test</p>',
    pullRequestUrl: pr.url,
    checkFailure: { ...failure, checks: [tests] },
    patch: { diff: flushDiff, commitMessage: 'fix(voice): bound the playout wait on close' },
  },
  {
    id: 'checks-rerun',
    summary: 'Rerun the tests: the runner lost its network',
    summaryHtml: '<p dir="auto">Rerun the tests: the runner lost its network</p>',
    pullRequestUrl: pr.url,
    checkFailure: { ...failure, checks: [tests] },
    rerun: true,
  },
];


// An investigation still on its first prompt, one that found the root cause and proposes a pull
// request on the base branch, a follow-up working, and a rebase asking the user something.
export const agentInputs: Input<Agent>[] = [
  {
    id: 'investigation-working',
    title: 'Investigate tests (3.12) on agents#4821',
    pullRequestUrls: [pr.url],
    startedAt: ago(240),
  },
  {
    id: 'investigation-answered',
    title: 'Investigate ruff on agents#4821',
    pullRequestUrls: [pr.url],
    state: 'AGENT_STATE_WAITING',
    startedAt: ago(7200),
    reply:
      'Not flaky. `ruff` on the runner resolves to 0.6, which `uv.lock` does not pin, and 0.6 added F401 for re-exports. It fails the same way on every run since Oct 2.',
    replyHtml:
      '<p dir="auto">Not flaky. <code class="notranslate">ruff</code> on the runner resolves to 0.6, which <code class="notranslate">uv.lock</code> does not pin, and 0.6 added F401 for re-exports. It fails the same way on <a href="https://github.com/livekit/agents/actions">every run since Oct 2</a>.</p>',
    proposals: [
      {
        id: 'investigation-follow-up',
        summary: 'Open a PR on main pinning ruff to 0.5',
        summaryHtml: '<p dir="auto">Open a PR on main pinning ruff to 0.5</p>',
        pullRequestUrl: pr.url,
        investigationId: 'investigation-answered',
        followUp:
          'In ~/code/agents, branch off origin/main, pin `ruff==0.5.7` in the dev dependencies, run `uv lock` and `uv run ruff check .`, then open a PR.',
        followUpHtml:
          '<p dir="auto">In ~/code/agents, branch off origin/main, pin <code class="notranslate">ruff==0.5.7</code> in the dev dependencies, run <code class="notranslate">uv lock</code> and <code class="notranslate">uv run ruff check .</code>, then open a PR.</p>',
      },
      {
        id: 'investigation-open-pull-request',
        summary: 'Open a PR on main pinning ruff to 0.5',
        summaryHtml: '<p dir="auto">Open a PR on main pinning ruff to 0.5</p>',
        pullRequestUrl: pr.url,
        investigationId: 'investigation-answered',
        openPullRequest: {
          repository: 'livekit/agents',
          base: 'main',
          branch: 'ci/pin-ruff',
          title: 'Pin ruff to 0.5',
          body: `ruff 0.6 added F401 for re-exports, which fails lint on every run since Oct 2. Found on ${pr.url}.`,
          bodyHtml: `<p dir="auto">ruff 0.6 added F401 for re-exports, which fails lint on every run since Oct 2. Found on <a href="${pr.url}">${pr.url}</a>.</p>`,
          patch: {
            diff: 'diff --git a/pyproject.toml b/pyproject.toml\n--- a/pyproject.toml\n+++ b/pyproject.toml\n@@ -12,3 +12,3 @@ dev = [\n     "pytest>=8",\n-    "ruff",\n+    "ruff==0.5.7",\n ]\n',
            commitMessage: 'Pin ruff to 0.5',
          },
        },
      },
    ],
  },
  {
    id: 'follow-up-working',
    title: 'Open a PR on main pinning ruff to 0.5',
    pullRequestUrls: [pr.url],
    startedAt: ago(60),
  },
  {
    id: 'rebase:thread-rebase',
    title: 'Rebase agents#4821',
    pullRequestUrls: [pr.url],
    state: 'AGENT_STATE_WAITING',
    startedAt: ago(900),
    reply: 'Rebasing onto main conflicts in `tts_retry.py`: main renamed `synthesize` to `synthesize_stream`. Keep the retry around the new name?',
    replyHtml:
      '<p dir="auto">Rebasing onto main conflicts in <code class="notranslate">tts_retry.py</code>: main renamed <code class="notranslate">synthesize</code> to <code class="notranslate">synthesize_stream</code>. Keep the retry around the new name?</p>',
  },
];

export const checkProposals = checkProposalInputs.map((proposal) => stored<Proposal>(proposal));
export const agents = agentInputs.map((agent) => stored<Agent>(agent));
