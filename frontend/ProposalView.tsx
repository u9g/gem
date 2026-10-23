import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import type { Proposal, PullRequest } from './gen/gem_pb';

// GitHub hides HTML comments, which bots use to tag their own comments.
function visibleMarkdown(markdown: string): string {
  return markdown.replace(/<!--[\s\S]*?-->/g, '').trim();
}

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
  return (
    <View style={styles.proposal}>
      <Pressable onPress={() => proposal.comment && Linking.openURL(proposal.comment.url)}>
        <Text style={styles.repo}>
          {pr.repository?.name}#{pr.number} · {proposal.comment?.author}
        </Text>
        <Text style={styles.quote} numberOfLines={4}>
          {visibleMarkdown(proposal.comment?.body ?? '')}
        </Text>
      </Pressable>
      <Text style={styles.summary}>{proposal.summary}</Text>
      {proposal.patch && (
        <>
          <Text style={styles.step}>Commit: {proposal.patch.commitMessage}</Text>
          <ScrollView style={styles.diff}>
            <ScrollView horizontal>
              <Text style={styles.code}>{proposal.patch.diff}</Text>
            </ScrollView>
          </ScrollView>
        </>
      )}
      {proposal.reply !== '' && (
        <>
          <Text style={styles.step}>{proposal.threadId ? 'Reply in thread' : 'Quote reply'}</Text>
          <Text style={styles.text}>{proposal.reply}</Text>
        </>
      )}
      {proposal.resolveThread && <Text style={styles.step}>Resolve thread</Text>}
      {proposal.requestedTool !== '' && (
        <>
          <Text style={styles.step}>Requested tool</Text>
          <Text style={styles.text}>{proposal.requestedTool}</Text>
        </>
      )}
      {proposal.error !== '' && <Text style={styles.error}>{proposal.error}</Text>}
      <View style={styles.buttons}>
        {proposal.requestedTool === '' && (
          <Pressable
            style={[styles.button, styles.approve]}
            disabled={disabled}
            onPress={onApprove}
          >
            <Text style={styles.buttonText}>{busy ? 'Working…' : 'Approve'}</Text>
          </Pressable>
        )}
        <Pressable style={styles.button} disabled={disabled} onPress={onDismiss}>
          <Text style={styles.buttonText}>Dismiss</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  proposal: {
    gap: 6,
    padding: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#2E3946',
  },
  quote: {
    color: '#D0CFD4',
    opacity: 0.6,
    paddingLeft: 8,
    borderLeftWidth: 2,
    borderLeftColor: '#2E3946',
  },
  summary: {
    color: '#D0CFD4',
    fontWeight: '600',
  },
  step: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
    marginTop: 4,
  },
  diff: {
    maxHeight: 240,
    padding: 8,
    borderRadius: 4,
    backgroundColor: '#11171E',
  },
  code: {
    color: '#D0CFD4',
    fontFamily: 'monospace',
    fontSize: 12,
  },
  error: {
    color: '#F87171',
    fontSize: 12,
  },
  buttons: {
    flexDirection: 'row',
    gap: 8,
    marginTop: 4,
  },
  button: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 6,
    backgroundColor: '#2E3946',
  },
  approve: {
    backgroundColor: '#2F6B45',
  },
  buttonText: {
    color: '#D0CFD4',
    fontWeight: '600',
  },
  repo: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
  },
  text: {
    color: '#D0CFD4',
    marginTop: 4,
  },
});
