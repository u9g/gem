import { ChevronLeft, ChevronRight, CircleAlert, ExternalLink } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from './gen/gem_pb';
import {
  Bubble,
  Clamp,
  DiffSurface,
  EditableReply,
  FollowUpCard,
  Html,
  Quote,
  type ReplyDraft,
  ResolvesTag,
  Theirs,
  ToolRequestBanner,
} from './surfaces';
import { Button, DiffStat, diffFiles, Icon, proposalIcon, stepSentence, t3, visibleMarkdown } from './t3';

// Proposals answering the same comment are alternatives: approving one clears the rest.
export function byComment(list: Proposal[]): Proposal[][] {
  const groups = new Map<string, Proposal[]>();
  for (const proposal of list) {
    const id = proposal.comment?.id ?? proposal.id;
    groups.set(id, [...(groups.get(id) ?? []), proposal]);
  }
  return [...groups.values()];
}

// One conversation waiting on u9g as a band: the proposal's summary and actions on top, then
// three columns cut to a few lines each: their thread, u9g's reply, and the change. Arrows step
// through alternatives.
export function ProposalGroup({
  proposals,
  busy,
  onApprove,
  onDismiss,
}: {
  proposals: Proposal[];
  // The proposal being approved or dismissed, if any.
  busy?: string;
  // Called with u9g's rewrites of the reply and follow-up prompt where that version is showing,
  // and '' otherwise.
  onApprove: (id: string, reply: string, followUp: string) => void;
  onDismiss: (id: string) => void;
}) {
  const [picked, setPicked] = useState(0);
  // Approving or dismissing an alternative removes it from the list.
  const index = Math.min(picked, proposals.length - 1);
  const proposal = proposals[index];
  return (
    <ProposalView
      key={proposal.id}
      proposal={proposal}
      stepper={
        proposals.length > 1 && (
          <View style={styles.stepper}>
            <Pressable
              accessibilityLabel="Previous alternative"
              onPress={() => setPicked((index + proposals.length - 1) % proposals.length)}
            >
              <Icon icon={ChevronLeft} size={14} />
            </Pressable>
            <Text style={styles.meta}>
              {index + 1} of {proposals.length}
            </Text>
            <Pressable accessibilityLabel="Next alternative" onPress={() => setPicked((index + 1) % proposals.length)}>
              <Icon icon={ChevronRight} size={14} />
            </Pressable>
          </View>
        )
      }
      busy={busy === proposal.id}
      disabled={busy !== undefined}
      onApprove={(reply, followUp) => onApprove(proposal.id, reply, followUp)}
      onDismiss={() => onDismiss(proposal.id)}
    />
  );
}

function ProposalView({
  proposal,
  stepper,
  busy,
  disabled,
  onApprove,
  onDismiss,
}: {
  proposal: Proposal;
  stepper: ReactNode;
  busy: boolean;
  disabled: boolean;
  onApprove: (reply: string, followUp: string) => void;
  onDismiss: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<ReplyDraft>({ yours: proposal.reply, showing: 'suggested' });
  const [followUp, setFollowUp] = useState<ReplyDraft>({ yours: proposal.followUp, showing: 'suggested' });
  const comment = proposal.comment!;
  const files = proposal.patch ? diffFiles(proposal.patch.diff) : [];
  const [, repository, number] = /github\.com\/[^/]+\/([^/]+)\/pull\/(\d+)/.exec(proposal.pullRequestUrl) ?? [];
  const quote = proposal.threadId
    ? undefined
    : visibleMarkdown(comment.body).split('\n\n')[0].replace(/[#*]/g, '').trim();
  return (
    <View style={[styles.band, proposal.error !== '' && styles.bandError]}>
      <View style={styles.header}>
        <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
        <Text numberOfLines={1} style={styles.summary}>
          {proposal.summary}
        </Text>
        {stepper}
        <Button label="Dismiss" disabled={disabled} onPress={onDismiss} />
        {!proposal.requestedTool && (
          <Button
            label={busy ? 'Working…' : proposal.error ? 'Retry' : 'Approve'}
            variant="default"
            disabled={disabled}
            onPress={() =>
              onApprove(
                draft.showing === 'yours' ? draft.yours : '',
                followUp.showing === 'yours' ? followUp.yours : '',
              )
            }
          />
        )}
      </View>
      {proposal.error !== '' && (
        <View style={styles.errorRow}>
          <Icon icon={CircleAlert} size={12} color={t3.deletion} />
          <Text style={styles.error}>{proposal.error}</Text>
        </View>
      )}
      <View style={styles.columns}>
        <View style={styles.column}>
          <Text style={styles.where}>
            {repository}#{number} · {proposal.threadId ? 'review thread' : 'conversation'}
          </Text>
          {proposal.earlierComments.map((earlier) => (
            <Theirs key={earlier.id} comment={earlier} max={110} />
          ))}
          <Theirs
            comment={comment}
            max={110}
            trailing={
              <Pressable accessibilityLabel="Open on GitHub" onPress={() => Linking.openURL(comment.url)}>
                <Icon icon={ExternalLink} size={12} />
              </Pressable>
            }
          />
        </View>
        <View style={styles.column}>
          {proposal.reply ? (
            <>
              <Text style={[styles.meta, styles.author]}>u9g</Text>
              {editing ? (
                <EditableReply suggested={proposal.reply} draft={draft} onChange={setDraft} quote={quote} />
              ) : (
                <Bubble side="right">
                  {quote !== undefined && <Quote lines={3}>{quote}</Quote>}
                  <Clamp max={110}>
                    <Html html={proposal.replyHtml} onPress={() => setEditing(true)} />
                  </Clamp>
                </Bubble>
              )}
            </>
          ) : (
            <Text style={styles.meta}>No reply</Text>
          )}
          {proposal.resolveThread && (
            <View style={styles.rightRow}>
              <ResolvesTag />
            </View>
          )}
        </View>
        <View style={styles.column}>
          {proposal.followUp !== '' && (
            <FollowUpCard
              suggested={proposal.followUp}
              html={proposal.followUpHtml}
              draft={followUp}
              onChange={setFollowUp}
            />
          )}
          {proposal.patch ? (
            <>
              <Text style={styles.meta}>
                {proposal.patch.commitMessage}
                {'  '}
                <DiffStat
                  added={files.reduce((n, f) => n + f.added, 0)}
                  removed={files.reduce((n, f) => n + f.removed, 0)}
                />
              </Text>
              <Clamp max={130}>
                <DiffSurface diff={proposal.patch.diff} />
              </Clamp>
            </>
          ) : proposal.requestedTool ? (
            <ToolRequestBanner proposal={proposal} />
          ) : (
            !proposal.followUp && (
              <Text style={styles.meta}>No code change: {stepSentence(proposal).toLowerCase()}</Text>
            )
          )}
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  band: {
    gap: 10,
    padding: 12,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  bandError: {
    borderColor: 'rgba(242, 109, 109, 0.28)',
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  summary: {
    flex: 1,
    color: t3.foreground,
    fontSize: 14,
    lineHeight: 22,
  },
  stepper: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
  },
  errorRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 4,
  },
  error: {
    flex: 1,
    color: t3.deletion,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
  },
  columns: {
    flexDirection: 'row',
    gap: 16,
    alignItems: 'flex-start',
  },
  column: {
    flex: 1,
    gap: 6,
  },
  where: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  meta: {
    color: t3.muted,
    fontSize: 12,
  },
  author: {
    textAlign: 'right',
    paddingRight: 14,
  },
  rightRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
  },
});
