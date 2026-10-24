import { CircleAlert, ExternalLink, MessageCircle, MessageSquareQuote } from 'lucide-react';
import { Fragment, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from './gen/gem_pb';
import { DiffSurface, EditableReply, type ReplyDraft, ResolvesTag, Theirs, ToolRequestBanner } from './surfaces';
import { Button, Icon, proposalIcon, t3, visibleMarkdown } from './t3';

// Proposals answering the same comment are alternatives: approving one clears the rest.
export function byComment(list: Proposal[]): Proposal[][] {
  const groups = new Map<string, Proposal[]>();
  for (const proposal of list) {
    const id = proposal.comment?.id ?? proposal.id;
    groups.set(id, [...(groups.get(id) ?? []), proposal]);
  }
  return [...groups.values()];
}

// One conversation waiting on u9g, laid out like a code review: the thread and each proposed
// answer on the left, its change on the right. Alternatives stack, split by "or".
export function ProposalGroup({
  proposals,
  busy,
  onApprove,
  onDismiss,
}: {
  proposals: Proposal[];
  // The proposal being approved or dismissed, if any.
  busy?: string;
  // Called with u9g's rewrite of the reply when that version is showing, and '' otherwise.
  onApprove: (id: string, reply: string) => void;
  onDismiss: (id: string) => void;
}) {
  const first = proposals[0];
  const [, repo, number] = /github\.com\/[^/]+\/([^/]+)\/pull\/(\d+)/.exec(first.pullRequestUrl) ?? [];
  return (
    <View style={styles.conversation}>
      <Text style={styles.where}>
        {repo}#{number} · {first.threadId ? 'review thread' : 'conversation'}
      </Text>
      {proposals.map((proposal, i) => (
        <Fragment key={proposal.id}>
          {i > 0 && (
            <View style={styles.orRow}>
              <View style={styles.orLine} />
              <Text style={styles.orText}>or</Text>
              <View style={styles.orLine} />
            </View>
          )}
          <ProposalView
            proposal={proposal}
            thread={i === 0}
            busy={busy === proposal.id}
            disabled={busy !== undefined}
            onApprove={(reply) => onApprove(proposal.id, reply)}
            onDismiss={() => onDismiss(proposal.id)}
          />
        </Fragment>
      ))}
    </View>
  );
}

function ProposalView({
  proposal,
  thread,
  busy,
  disabled,
  onApprove,
  onDismiss,
}: {
  proposal: Proposal;
  // Shows the conversation above the answer; alternatives after the first share it.
  thread: boolean;
  busy: boolean;
  disabled: boolean;
  onApprove: (reply: string) => void;
  onDismiss: () => void;
}) {
  const [draft, setDraft] = useState<ReplyDraft>({ yours: proposal.reply, showing: 'suggested' });
  const comment = proposal.comment!;
  const failed = failedStep(proposal);
  const error = <Text style={styles.error}>{proposal.error}</Text>;
  return (
    <View style={styles.split}>
      <View style={styles.left}>
        {thread && (
          <View style={{ gap: 10 }}>
            {proposal.earlierComments.map((earlier) => (
              <Theirs key={earlier.id} comment={earlier} />
            ))}
            <Theirs
              comment={comment}
              trailing={
                <Pressable accessibilityLabel="Open on GitHub" onPress={() => Linking.openURL(comment.url)}>
                  <Icon icon={ExternalLink} size={12} />
                </Pressable>
              }
            />
          </View>
        )}
        <View style={styles.summaryRow}>
          <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
          <Text style={styles.summary}>{proposal.summary}</Text>
        </View>
        {proposal.reply !== '' && (
          <View style={{ gap: 6 }}>
            <EditableReply
              suggested={proposal.reply}
              draft={draft}
              onChange={setDraft}
              quote={
                proposal.threadId ? undefined : visibleMarkdown(comment.body).split('\n\n')[0].replace(/[#*]/g, '').trim()
              }
            />
            {proposal.resolveThread && (
              <View style={styles.tagRow}>
                <ResolvesTag />
              </View>
            )}
          </View>
        )}
        {proposal.reply === '' && proposal.resolveThread && <ResolvesTag />}
        {(failed === 'reply' || failed === 'resolve') && error}
        <View style={styles.buttons}>
          <Button label="Dismiss" disabled={disabled} onPress={onDismiss} />
          {!proposal.requestedTool && (
            <Button
              label={busy ? 'Working…' : proposal.error ? 'Retry' : 'Approve'}
              variant="default"
              disabled={disabled}
              onPress={() => onApprove(draft.showing === 'yours' ? draft.yours : '')}
            />
          )}
        </View>
      </View>
      <View style={styles.right}>
        {proposal.patch ? (
          <>
            <Text style={styles.meta}>{proposal.patch.commitMessage}</Text>
            <DiffSurface
              diff={proposal.patch.diff}
              header={failed === 'patch' ? <Icon icon={CircleAlert} size={12} color={t3.deletion} /> : undefined}
            />
            {failed === 'patch' && error}
          </>
        ) : proposal.requestedTool ? (
          <ToolRequestBanner proposal={proposal} />
        ) : (
          <View style={styles.noChange}>
            <Icon icon={proposal.threadId ? MessageCircle : MessageSquareQuote} />
            <Text style={styles.meta}>No code change: approving only replies</Text>
          </View>
        )}
      </View>
    </View>
  );
}

// The step approving runs first is the one that failed: steps that succeed are cleared.
function failedStep(proposal: Proposal): 'patch' | 'reply' | 'resolve' | undefined {
  if (!proposal.error) return undefined;
  if (proposal.patch) return 'patch';
  if (proposal.reply) return 'reply';
  if (proposal.resolveThread) return 'resolve';
}

const styles = StyleSheet.create({
  conversation: {
    gap: 14,
    paddingBottom: 28,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  where: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  orRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  orLine: {
    flex: 1,
    height: 1,
    backgroundColor: t3.border,
  },
  orText: {
    color: t3.faint,
    fontSize: 11,
  },
  split: {
    flexDirection: 'row',
    gap: 24,
    alignItems: 'flex-start',
  },
  left: {
    flex: 4,
    gap: 12,
  },
  right: {
    flex: 5,
    gap: 8,
  },
  summaryRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 6,
  },
  summary: {
    flex: 1,
    color: t3.foreground,
    fontSize: 14,
    lineHeight: 22,
  },
  tagRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
  },
  buttons: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 6,
  },
  noChange: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t3.input,
  },
  meta: {
    color: t3.muted,
    fontSize: 12,
  },
  error: {
    color: t3.deletion,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
  },
});
