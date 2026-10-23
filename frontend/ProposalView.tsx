import { ChevronRight, ExternalLink } from 'lucide-react';
import { useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal, PullRequest } from './gen/gem_pb';
import {
  Button,
  CommentRow,
  DiffStat,
  diffFiles,
  ErrorLine,
  Icon,
  proposalIcon,
  Steps,
  stepSentence,
  t3,
} from './t3';

// An approval banner in T3 Code's style: what approving does and the proposal's one-line summary,
// with every step behind Details.
export function ProposalView({
  pr,
  proposal,
  busy,
  disabled,
  onApprove,
  onDismiss,
}: {
  pr: PullRequest;
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
          <Button label={busy ? 'Working…' : 'Approve'} variant="default" disabled={disabled} onPress={onApprove} />
        )}
      </View>
      {open && (
        <View>
          <CommentRow
            item={{ pr, proposal }}
            trailing={
              <Pressable
                accessibilityLabel="Open on GitHub"
                onPress={() => proposal.comment && Linking.openURL(proposal.comment.url)}
              >
                <Icon icon={ExternalLink} size={12} />
              </Pressable>
            }
          />
          <Steps proposal={proposal} />
          <ErrorLine proposal={proposal} />
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
});
