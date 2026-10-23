import { ChevronRight, CircleAlert, ExternalLink } from 'lucide-react';
import { useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from './gen/gem_pb';
import { Bubble, DiffSurface, Quote, ResolvesTag, Theirs, ToolRequestBanner } from './surfaces';
import { Button, DiffStat, diffFiles, Icon, Markdown, proposalIcon, stepSentence, t3, visibleMarkdown } from './t3';

// An approval banner in T3 Code's style: what approving does and the proposal's one-line summary,
// with every step behind Details.
export function ProposalView({
  proposal,
  busy,
  disabled,
  onApprove,
  onDismiss,
}: {
  proposal: Proposal;
  // This proposal is being approved or dismissed.
  busy: boolean;
  disabled: boolean;
  onApprove: () => void;
  onDismiss: () => void;
}) {
  const [open, setOpen] = useState(false);
  const files = proposal.patch ? diffFiles(proposal.patch.diff) : [];
  return (
    <View style={[styles.banner, proposal.error !== '' && styles.bannerError]}>
      <View style={styles.top}>
        <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
        <View style={styles.text}>
          <Text style={styles.eyebrow}>
            <Text style={{ color: proposal.error ? t3.deletion : t3.foreground, fontWeight: '500' }}>
              {proposal.error ? 'Approval failed' : stepSentence(proposal)}
            </Text>
            {'  '}
            {proposal.comment?.author}
            {files.length > 0 && (
              <>
                {'  '}
                <DiffStat
                  added={files.reduce((n, f) => n + f.added, 0)}
                  removed={files.reduce((n, f) => n + f.removed, 0)}
                />
              </>
            )}
          </Text>
          <Text numberOfLines={2} style={styles.summary}>
            {proposal.summary}
          </Text>
        </View>
      </View>
      <View style={styles.bottom}>
        <Pressable style={styles.toggle} onPress={() => setOpen(!open)}>
          <Text style={styles.toggleText}>Details</Text>
          <View style={{ transform: [{ rotate: open ? '90deg' : '0deg' }] }}>
            <ChevronRight size={12} color={t3.muted} />
          </View>
        </Pressable>
        <Button label="Dismiss" disabled={disabled} onPress={onDismiss} />
        {!proposal.requestedTool && (
          <Button
            label={busy ? 'Working…' : proposal.error ? 'Retry' : 'Approve'}
            variant="default"
            disabled={disabled}
            onPress={onApprove}
          />
        )}
      </View>
      {open && <Details proposal={proposal} />}
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

// The conversation in order: their comment, the change, then u9g's reply as it will be posted.
function Details({ proposal }: { proposal: Proposal }) {
  const comment = proposal.comment!;
  const failed = failedStep(proposal);
  const quoting = proposal.reply !== '' && !proposal.threadId;
  const error = <Text style={styles.error}>{proposal.error}</Text>;
  return (
    <View style={styles.detailsBody}>
      {!quoting && (
        <Theirs
          author={comment.author}
          body={comment.body}
          trailing={
            <Pressable accessibilityLabel="Open on GitHub" onPress={() => Linking.openURL(comment.url)}>
              <Icon icon={ExternalLink} size={12} />
            </Pressable>
          }
        />
      )}
      {proposal.requestedTool !== '' && <ToolRequestBanner proposal={proposal} />}
      {proposal.patch && (
        <View style={styles.step}>
          <Text style={styles.meta}>{proposal.patch.commitMessage}</Text>
          <DiffSurface
            diff={proposal.patch.diff}
            header={failed === 'patch' ? <Icon icon={CircleAlert} size={12} color={t3.deletion} /> : undefined}
          />
          {failed === 'patch' && error}
        </View>
      )}
      {proposal.reply !== '' && (
        <View style={styles.step}>
          <Bubble>
            {quoting && (
              <Quote lines={3}>{visibleMarkdown(comment.body).split('\n\n')[0].replace(/[#*]/g, '').trim()}</Quote>
            )}
            <Markdown>{proposal.reply}</Markdown>
          </Bubble>
          {proposal.resolveThread && (
            <View style={styles.tagRow}>
              <ResolvesTag />
            </View>
          )}
          {(failed === 'reply' || failed === 'resolve') && error}
        </View>
      )}
      {proposal.reply === '' && proposal.resolveThread && (
        <View style={styles.step}>
          <ResolvesTag />
          {failed === 'resolve' && error}
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  banner: {
    gap: 4,
    padding: 10,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  bannerError: {
    borderColor: 'rgba(242, 109, 109, 0.28)',
    backgroundColor: 'rgba(242, 109, 109, 0.06)',
  },
  top: {
    flexDirection: 'row',
    gap: 6,
  },
  text: {
    flex: 1,
    gap: 2,
  },
  eyebrow: {
    color: t3.muted,
    fontSize: 11,
  },
  summary: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 18,
  },
  bottom: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingLeft: 30,
  },
  toggle: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    height: 24,
  },
  toggleText: {
    color: t3.muted,
    fontSize: 12,
  },
  detailsBody: {
    gap: 10,
    paddingTop: 6,
  },
  step: {
    gap: 6,
  },
  tagRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
  },
  meta: {
    color: t3.muted,
    fontSize: 11,
  },
  error: {
    color: t3.deletion,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
  },
});
