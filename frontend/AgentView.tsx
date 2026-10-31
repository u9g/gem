import { useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, TextInput, View } from 'react-native';

import type { Node } from './db';
import { ProposalGroup } from './ProposalView';
import { type Agent, agentState } from './shapes';
import { Html } from './surfaces';
import { Button, t3 } from './t3';

// An agent's latest reply, what it proposes, and a box to message it, which is how a question in its
// reply is answered. Its proposals are independent, so each shows on its own.
export function AgentView({
  agent,
  ago,
  busy,
  onApprove,
  onDismiss,
  onMessage,
  onClose,
}: {
  agent: Node<Agent>;
  ago: (date: Date) => string;
  busy?: ReadonlySet<string>;
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  // Resolves once the message is sent.
  onMessage: (message: string) => Promise<unknown>;
  onClose: () => void;
}) {
  const state = agentState(agent);
  const startedAt = agent.time('startedAt');
  const on = agent.strings('pullRequestUrls').map((url) => {
    const [, repository, number] = /github\.com\/[^/]+\/([^/]+)\/pull\/(\d+)/.exec(url) ?? [];
    return `${repository}#${number}`;
  });
  return (
    <View style={styles.column}>
      <View style={styles.header}>
        <View style={styles.titleBlock}>
          <Text style={styles.title}>{agent.str('title')}</Text>
          <Text style={styles.meta}>
            {on.length > 0 && `on ${on.join(', ')}`}
            {startedAt && ` · ${ago(startedAt)}`}
          </Text>
        </View>
        <Button label="Done" disabled={busy?.has(agent.str('id'))} onPress={onClose} />
      </View>
      {agent.str('reply') !== '' && <Html html={agent.str('replyHtml')} />}
      {state === 'AGENT_STATE_WORKING' && (
        <View style={styles.working}>
          <ActivityIndicator size={12} color="#8FA8F5" />
          <Text style={styles.meta}>Working…</Text>
        </View>
      )}
      {state === 'AGENT_STATE_QUEUED' && <Text style={styles.meta}>Queued behind other agents…</Text>}
      {agent.list('proposals').map((proposal) => (
        <ProposalGroup key={proposal.str('id')} proposals={[proposal]} busy={busy} onApprove={onApprove} onDismiss={onDismiss} />
      ))}
      {state !== 'AGENT_STATE_QUEUED' && <Composer placeholder="Message the agent…" onSend={onMessage} />}
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
