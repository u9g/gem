import { timestampDate } from '@bufbuild/protobuf/wkt';
import { useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, TextInput, View } from 'react-native';

import type { Investigation } from './gen/gem_pb';
import { ProposalGroup } from './ProposalView';
import { FailedChecks, Html } from './surfaces';
import { Button, t3 } from './t3';

// An investigation's or follow-up's latest reply, what it proposes, and a box to message its agent. Its
// proposals are independent, so each shows on its own.
export function InvestigationView({
  investigation,
  ago,
  busy,
  onApprove,
  onDismiss,
  onMessage,
  onClose,
}: {
  investigation: Investigation;
  ago: (date: Date) => string;
  busy?: string;
  onApprove: (id: string, reply: string, followUp: string) => void;
  onDismiss: (id: string) => void;
  // Resolves once the message is sent.
  onMessage: (message: string) => Promise<unknown>;
  onClose: () => void;
}) {
  const [, repository, number] = /github\.com\/[^/]+\/([^/]+)\/pull\/(\d+)/.exec(investigation.pullRequestUrl) ?? [];
  return (
    <View style={styles.column}>
      <View style={styles.header}>
        <View style={styles.titleBlock}>
          <Text style={styles.title}>{investigation.title}</Text>
          <Text style={styles.meta}>
            from {repository}#{number}
            {investigation.startedAt && ` · ${ago(timestampDate(investigation.startedAt))}`}
          </Text>
        </View>
        <Button label="Done" disabled={busy !== undefined} onPress={onClose} />
      </View>
      {investigation.checkFailure && <FailedChecks failure={investigation.checkFailure} />}
      {investigation.reply !== '' && <Html html={investigation.replyHtml} />}
      {investigation.working && (
        <View style={styles.working}>
          <ActivityIndicator size={12} color="#8FA8F5" />
          <Text style={styles.meta}>Working…</Text>
        </View>
      )}
      {investigation.proposals.map((proposal) => (
        <ProposalGroup key={proposal.id} proposals={[proposal]} busy={busy} onApprove={onApprove} onDismiss={onDismiss} />
      ))}
      <Composer placeholder="Message the agent…" onSend={onMessage} />
    </View>
  );
}

// A box to write a message in and send it. It empties once the message is sent.
export function Composer({
  placeholder,
  onSend,
}: {
  placeholder: string;
  // Resolves once the message is sent.
  onSend: (message: string) => Promise<unknown>;
}) {
  const [message, setMessage] = useState('');
  const [sending, setSending] = useState(false);
  const send = () => {
    if (!message.trim()) return;
    setSending(true);
    onSend(message)
      .then(() => setMessage(''))
      .finally(() => setSending(false));
  };
  return (
    <View style={styles.composer}>
      <TextInput
        multiline
        value={message}
        onChangeText={setMessage}
        placeholder={placeholder}
        placeholderTextColor={t3.faint}
        style={styles.input}
      />
      <Button label={sending ? 'Sending…' : 'Send'} variant="default" disabled={sending || !message.trim()} onPress={send} />
    </View>
  );
}

const styles = StyleSheet.create({
  column: {
    width: '100%',
    maxWidth: 1240,
    gap: 12,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 12,
    marginBottom: 8,
  },
  titleBlock: {
    flex: 1,
    gap: 4,
  },
  title: {
    color: t3.foreground,
    fontSize: 20,
    fontWeight: '600',
  },
  meta: {
    color: t3.muted,
    fontSize: 12,
  },
  working: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  composer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: 8,
    marginTop: 8,
    padding: 8,
    borderRadius: 12,
    backgroundColor: t3.input,
  },
  input: {
    flex: 1,
    minHeight: 24,
    color: t3.foreground,
    fontSize: 14,
    lineHeight: 20,
    outlineStyle: 'none',
  } as object,
});
