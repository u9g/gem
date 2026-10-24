import { ChevronLeft, ChevronRight, CircleAlert, ExternalLink } from 'lucide-react';
import { type ReactNode, useContext, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import './ProposalView.css';

import type { Proposal } from './gen/gem_pb';
import {
  Bubble,
  Clamp,
  ClosesTag,
  DiffSurface,
  EditableReply,
  FailedChecks,
  FollowUpCard,
  Html,
  Login,
  Quote,
  type ReplyDraft,
  RerunCard,
  ResolvesTag,
  Theirs,
  ToolRequestBanner,
} from './surfaces';
import { Button, DiffStat, diffFiles, Icon, proposalIcon, stepSentence, t3, usePhone, visibleMarkdown } from './t3';

// Proposals answering the same conversation, or the same failed checks, are alternatives: approving
// one clears the rest.
export function bySubject(list: Proposal[]): Proposal[][] {
  const groups = new Map<string, Proposal[]>();
  for (const proposal of list) {
    const checks = proposal.checkFailure?.checks.map((c) => c.id).sort().join(',');
    const id = proposal.comment?.id ?? checks ?? proposal.id;
    groups.set(id, [...(groups.get(id) ?? []), proposal]);
  }
  return [...groups.values()];
}

// One conversation waiting on the user as a band: the proposal's summary and actions on top, then
// three columns cut to a few lines each: their thread, the user's reply, and the change. Arrows step
// through alternatives.
export function ProposalGroup({
  proposals,
  busy,
  onApprove,
  onDismiss,
  onInvestigate,
}: {
  proposals: Proposal[];
  // The proposal being approved or dismissed, if any.
  busy?: string;
  // Called with the user's rewrites of the reply and follow-up prompt where that version is showing,
  // and '' otherwise.
  onApprove: (id: string, reply: string, followUp: string) => void;
  onDismiss: (id: string) => void;
  // Approves a rerun and starts an investigation of the checks it reruns.
  onInvestigate?: (id: string) => void;
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
      onInvestigate={onInvestigate && (() => onInvestigate(proposal.id))}
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
  onInvestigate,
}: {
  proposal: Proposal;
  stepper: ReactNode;
  busy: boolean;
  disabled: boolean;
  onApprove: (reply: string, followUp: string) => void;
  onDismiss: () => void;
  onInvestigate?: () => void;
}) {
  const login = useContext(Login);
  const phone = usePhone();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<ReplyDraft>({ yours: proposal.reply, showing: 'suggested' });
  const [followUp, setFollowUp] = useState<ReplyDraft>({ yours: proposal.followUp, showing: 'suggested' });
  const files = proposal.patch ? diffFiles(proposal.patch.diff) : [];
  const [, repository, number] = /github\.com\/[^/]+\/([^/]+)\/pull\/(\d+)/.exec(proposal.pullRequestUrl) ?? [];
  const comment = proposal.comment;
  const quote =
    proposal.threadId || !comment
      ? undefined
      : visibleMarkdown(comment.body).split('\n\n')[0].replace(/[#*]/g, '').trim();
  const investigate = onInvestigate && proposal.rerun && !proposal.patch && !proposal.followUp && (
    <Button label="Rerun & investigate" large={phone} disabled={disabled} onPress={onInvestigate} />
  );
  const actions = (
    <>
      <Button label="Dismiss" large={phone} disabled={disabled} onPress={onDismiss} />
      {!phone && investigate}
      {!proposal.requestedTool && (
        <Button
          label={busy ? 'Working…' : proposal.error ? 'Retry' : 'Approve'}
          variant="default"
          large={phone}
          disabled={disabled}
          onPress={() =>
            onApprove(
              draft.showing === 'yours' ? draft.yours : '',
              followUp.showing === 'yours' ? followUp.yours : '',
            )
          }
        />
      )}
    </>
  );
  return (
    <View style={[styles.band, proposal.error !== '' && styles.bandError]}>
      <View style={styles.header}>
        <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
        <View style={styles.summary}>
          <Html html={proposal.summaryHtml} className={phone ? undefined : 'one-line'} style={summaryHtml} />
        </View>
        {stepper}
        {!phone && actions}
      </View>
      {proposal.error !== '' && (
        <View style={styles.errorRow}>
          <Icon icon={CircleAlert} size={12} color={t3.deletion} />
          <Text style={styles.error}>{proposal.error}</Text>
        </View>
      )}
      <View style={[styles.columns, phone && styles.stacked]}>
        <View style={[styles.column, phone && styles.columnStacked]}>
          <Text style={styles.where}>
            {repository}#{number} ·{' '}
            {comment
              ? proposal.threadId
                ? 'review thread'
                : 'conversation'
              : proposal.checkFailure
                ? 'checks'
                : 'investigation'}
          </Text>
          {proposal.earlierComments.map((earlier) => (
            <Theirs key={earlier.id} comment={earlier} max={110} />
          ))}
          {comment && (
            <Theirs
              comment={comment}
              max={110}
              trailing={
                <Pressable accessibilityLabel="Open on GitHub" onPress={() => Linking.openURL(comment.url)}>
                  <Icon icon={ExternalLink} size={12} />
                </Pressable>
              }
            />
          )}
          {proposal.checkFailure && <FailedChecks failure={proposal.checkFailure} />}
        </View>
        {comment && (
          <View style={[styles.column, phone && styles.columnStacked]}>
            {proposal.reply ? (
              <>
                <Text style={[styles.meta, styles.author]}>{login}</Text>
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
            {(proposal.resolveThread || proposal.closePullRequest) && (
              <View style={styles.rightRow}>
                {proposal.resolveThread && <ResolvesTag />}
                {proposal.closePullRequest && <ClosesTag />}
              </View>
            )}
          </View>
        )}
        <View style={[styles.column, phone && styles.columnStacked]}>
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
          ) : proposal.rerun && proposal.checkFailure ? (
            <RerunCard failure={proposal.checkFailure} />
          ) : (
            !proposal.followUp && (
              <Text style={styles.meta}>No code change: {stepSentence(proposal).toLowerCase()}</Text>
            )
          )}
        </View>
      </View>
      {phone && <View style={styles.phoneActions}>{actions}</View>}
      {phone && investigate && <View style={styles.phoneActions}>{investigate}</View>}
    </View>
  );
}

const summaryHtml = { fontSize: 14, lineHeight: '22px' };

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
    minWidth: 0,
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
  stacked: {
    flexDirection: 'column',
    alignItems: 'stretch',
    gap: 12,
  },
  // Stacked columns are as tall as their content.
  columnStacked: {
    flexBasis: 'auto',
  },
  phoneActions: {
    flexDirection: 'row',
    gap: 8,
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
    gap: 6,
  },
});
