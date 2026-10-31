import { ChevronLeft, ChevronRight, CircleAlert, ExternalLink } from 'lucide-react';
import { type ReactNode, useContext, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import './ProposalView.css';

import type { Node } from './db';
import type { Proposal } from './shapes';
import {
  Clamp,
  ClosesTag,
  DiffSurface,
  EditableReply,
  FailedChecks,
  FollowUpCard,
  Html,
  Login,
  NewPullRequestCard,
  type ReplyDraft,
  RerunCard,
  ResolvesTag,
  Theirs,
  ToolRequestBanner,
} from './surfaces';
import { Button, DiffStat, diffFiles, Icon, proposalIcon, stepSentence, t3, usePhone } from './t3';

// Proposals answering the same conversation, or the same failed checks, are alternatives: approving
// one clears the rest.
export function bySubject(list: Node<Proposal>[]): Node<Proposal>[][] {
  const groups = new Map<string, Node<Proposal>[]>();
  for (const proposal of list) {
    const checks = proposal.node('checkFailure')?.list('checks').map((c) => c.str('id')).sort().join(',');
    const id = proposal.node('comment')?.str('id') ?? checks ?? proposal.str('id');
    groups.set(id, [...(groups.get(id) ?? []), proposal]);
  }
  return [...groups.values()];
}

// The last lines of a review comment's diff hunk, ending at the line it is on, as a one-file diff.
function anchorDiff(path: string, hunk: string, lines = 6): string {
  const [header, ...body] = hunk.split('\n');
  const kept = body.slice(-lines);
  // The new side's start moves past every dropped line the new file has: context and additions.
  // An empty line is context whose leading space was stripped.
  const start = Number(/\+(\d+)/.exec(header)?.[1] ?? 1) + body.slice(0, -lines).filter((l) => /^([ +]|$)/.test(l)).length;
  const count = (side: RegExp) => kept.filter((l) => side.test(l)).length;
  return `--- a/${path}\n+++ b/${path}\n@@ -0,${count(/^([ -]|$)/)} +${start},${count(/^([ +]|$)/)} @@\n${kept.join('\n')}`;
}

// The repository name and number of the pull request at `url`.
function pullRequestName(url: string): [string?, string?] {
  const [, repository, number] = /github\.com\/[^/]+\/([^/]+)\/pull\/(\d+)/.exec(url) ?? [];
  return [repository, number];
}

// One conversation waiting on the user as a band: the proposal's summary and actions on top, then
// three columns cut to a few lines each: their thread, the user's reply, and the change. Arrows step
// through alternatives. `anchored` stacks them in full instead, under the code the comment is on;
// `inline` stacks them in full as a thread placed inside that code.
export function ProposalGroup({
  proposals,
  anchored = false,
  inline = false,
  busy,
  onApprove,
  onDismiss,
  onInvestigate,
}: {
  proposals: Node<Proposal>[];
  anchored?: boolean;
  inline?: boolean;
  // Proposals being approved or dismissed.
  busy?: ReadonlySet<string>;
  // Called with the user's rewrites of the reply and follow-up prompt where that version is showing,
  // and of the patch's diff where it differs, and '' otherwise.
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  // Approves a rerun and starts an investigation of the checks it reruns.
  onInvestigate?: (id: string) => void;
}) {
  const [picked, setPicked] = useState(0);
  // Approving or dismissing an alternative removes it from the list.
  const index = Math.min(picked, proposals.length - 1);
  const proposal = proposals[index];
  const id = proposal.str('id');
  return (
    <ProposalView
      key={id}
      proposal={proposal}
      anchored={anchored || inline}
      inline={inline}
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
      busy={busy?.has(id) === true}
      // Approving one alternative clears the others.
      disabled={proposals.some((p) => busy?.has(p.str('id')))}
      onApprove={(reply, followUp, diff) => onApprove(id, reply, followUp, diff)}
      onDismiss={() => onDismiss(id)}
      onInvestigate={onInvestigate && (() => onInvestigate(id))}
    />
  );
}

// What the user is writing in each proposal, by proposal ID and field, outliving the view showing
// it: a thread that moves, e.g. inline once its file's diff arrives, keeps it.
const drafts = new Map<string, unknown>();

function useDraft<T>(key: string, initial: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => (drafts.has(key) ? (drafts.get(key) as T) : initial));
  return [
    value,
    (next) => {
      drafts.set(key, next);
      setValue(next);
    },
  ];
}

function ProposalView({
  proposal,
  anchored,
  inline,
  stepper,
  busy,
  disabled,
  onApprove,
  onDismiss,
  onInvestigate,
}: {
  proposal: Node<Proposal>;
  anchored: boolean;
  inline: boolean;
  stepper: ReactNode;
  busy: boolean;
  disabled: boolean;
  onApprove: (reply: string, followUp: string, diff: string) => void;
  onDismiss: () => void;
  onInvestigate?: () => void;
}) {
  const login = useContext(Login);
  const phone = usePhone();
  const id = proposal.str('id');
  const reply = proposal.str('reply');
  const suggestedFollowUp = proposal.str('followUp');
  const patch = proposal.node('patch');
  const otherPatches = proposal.list('otherPatches');
  const openPullRequest = proposal.node('openPullRequest');
  const checkFailure = proposal.node('checkFailure');
  const error = proposal.str('error');
  const requestedTool = proposal.str('requestedTool');
  const pullRequestUrl = proposal.str('pullRequestUrl');
  const [draft, setDraft] = useDraft<ReplyDraft>(`${id} reply`, { yours: reply, showing: 'suggested' });
  const [followUp, setFollowUp] = useDraft<ReplyDraft>(`${id} follow-up`, {
    yours: suggestedFollowUp,
    showing: 'suggested',
  });
  const [diff, setDiff] = useDraft(`${id} diff`, patch?.str('diff') ?? '');
  const files = diffFiles(diff);
  const [repository, number] = pullRequestName(pullRequestUrl);
  const comment = proposal.node('comment');
  const stacked = phone || anchored;
  const clamp = (max: number, children: ReactNode) => (anchored ? children : <Clamp max={max}>{children}</Clamp>);
  const investigate = onInvestigate &&
    proposal.bool('rerun') &&
    !patch &&
    !otherPatches.length &&
    !openPullRequest &&
    !suggestedFollowUp && (
      <Button label="Rerun & investigate" large={phone} disabled={disabled} onPress={onInvestigate} />
    );
  const actions = (
    <>
      <Button label="Dismiss" large={phone} disabled={disabled} onPress={onDismiss} />
      {!phone && investigate}
      {!requestedTool && (
        <Button
          label={busy ? 'Working…' : error ? 'Retry' : 'Approve'}
          variant="default"
          large={phone}
          disabled={disabled}
          onPress={() =>
            onApprove(
              draft.showing === 'yours' ? draft.yours : '',
              followUp.showing === 'yours' ? followUp.yours : '',
              diff === patch?.str('diff') ? '' : diff,
            )
          }
        />
      )}
    </>
  );
  return (
    <View style={[styles.band, inline && styles.bandInline, error !== '' && styles.bandError]}>
      <View style={styles.header}>
        <Icon icon={proposalIcon(proposal)} color={error ? t3.deletion : t3.muted} />
        <View style={styles.summary}>
          <Html html={proposal.str('summaryHtml')} className={phone ? undefined : 'one-line'} style={summaryHtml} />
        </View>
        {stepper}
        {!phone && actions}
      </View>
      {error !== '' && (
        <View style={styles.errorRow}>
          <Icon icon={CircleAlert} size={12} color={t3.deletion} />
          <Text style={styles.error}>{error}</Text>
        </View>
      )}
      <View style={[styles.columns, stacked && styles.stacked]}>
        <View style={[styles.column, stacked && styles.columnStacked]}>
          <Text style={styles.where}>
            {repository}#{number} ·{' '}
            {comment
              ? proposal.str('threadId')
                ? 'review thread'
                : 'conversation'
              : checkFailure
                ? 'checks'
                : 'investigation'}
          </Text>
          {anchored && !inline && comment?.str('diffHunk') && (
            <DiffSurface
              diff={anchorDiff(comment.str('path'), comment.str('diffHunk'))}
              header={<Text style={styles.meta}>{comment.num('line') ? `line ${comment.num('line')}` : 'outdated'}</Text>}
            />
          )}
          {proposal.list('earlierComments').map((earlier) => (
            <Theirs key={earlier.str('id')} comment={earlier} max={anchored ? undefined : 110} />
          ))}
          {comment && (
            <Theirs
              comment={comment}
              max={anchored ? undefined : 110}
              trailing={
                <Pressable accessibilityLabel="Open on GitHub" onPress={() => Linking.openURL(comment.str('url'))}>
                  <Icon icon={ExternalLink} size={12} />
                </Pressable>
              }
            />
          )}
          {checkFailure && <FailedChecks failure={checkFailure} />}
        </View>
        {comment && (
          <View style={[styles.column, stacked && styles.columnStacked]}>
            {reply ? (
              <>
                <Text style={[styles.meta, styles.author]}>{login}</Text>
                <EditableReply
                  pr={pullRequestUrl}
                  suggested={reply}
                  html={proposal.str('replyHtml')}
                  draft={draft}
                  onChange={setDraft}
                  max={anchored ? undefined : 110}
                />
              </>
            ) : (
              <Text style={styles.meta}>No reply</Text>
            )}
            {(proposal.bool('resolveThread') || proposal.bool('closePullRequest')) && (
              <View style={styles.rightRow}>
                {proposal.bool('resolveThread') && <ResolvesTag />}
                {proposal.bool('closePullRequest') && <ClosesTag />}
              </View>
            )}
          </View>
        )}
        <View style={[styles.column, stacked && styles.columnStacked]}>
          {suggestedFollowUp !== '' && (
            <FollowUpCard
              pr={pullRequestUrl}
              suggested={suggestedFollowUp}
              html={proposal.str('followUpHtml')}
              draft={followUp}
              onChange={setFollowUp}
            />
          )}
          {patch ? (
            <>
              <Text style={styles.meta}>
                {patch.str('commitMessage')}
                {'  '}
                <DiffStat
                  added={files.reduce((n, f) => n + f.added, 0)}
                  removed={files.reduce((n, f) => n + f.removed, 0)}
                />
              </Text>
              {clamp(130, <DiffSurface diff={diff} suggested={patch.str('diff')} onChange={setDiff} />)}
            </>
          ) : requestedTool ? (
            <ToolRequestBanner proposal={proposal} />
          ) : proposal.bool('rerun') && checkFailure ? (
            <RerunCard failure={checkFailure} />
          ) : (
            !suggestedFollowUp &&
            !otherPatches.length &&
            !openPullRequest && (
              <Text style={styles.meta}>No code change: {stepSentence(proposal).toLowerCase()}</Text>
            )
          )}
          {otherPatches.map((other) => {
            const [repository, number] = pullRequestName(other.str('pullRequestUrl'));
            const files = diffFiles(other.str('diff'));
            return (
              <View key={other.str('pullRequestUrl')} style={styles.column}>
                <Text style={styles.where}>
                  {repository}#{number}
                </Text>
                <Text style={styles.meta}>
                  {other.str('commitMessage')}
                  {'  '}
                  <DiffStat
                    added={files.reduce((n, f) => n + f.added, 0)}
                    removed={files.reduce((n, f) => n + f.removed, 0)}
                  />
                </Text>
                {clamp(130, <DiffSurface diff={other.str('diff')} />)}
              </View>
            );
          })}
          {openPullRequest && <NewPullRequestCard pullRequest={openPullRequest} />}
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
  bandInline: {
    borderColor: 'rgba(245, 158, 11, 0.35)',
    backgroundColor: '#1A2129',
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
