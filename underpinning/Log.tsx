import { memo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { type Entry, useBackendContext } from './backend';
import { styles as nodeStyles } from './nodes';

const kinds: Record<Entry['kind'], { mark: string; label: string; color: string }> = {
  sent: { mark: '→', label: 'Sent', color: '#2F6FDE' },
  response: { mark: '←', label: 'Response', color: '#2E9E5B' },
  error: { mark: '✕', label: 'Error', color: '#D1453B' },
  pushed: { mark: '⇠', label: 'Received', color: '#8A4FD8' },
  event: { mark: '•', label: 'Event', color: '#5B6670' },
};

function time(at: Date) {
  return at.toLocaleTimeString(undefined, { hour12: false });
}

// One message; pressing it shows its whole body.
const Row = memo(function Row({ entry }: { entry: Entry }) {
  const [expanded, setExpanded] = useState(false);
  const kind = kinds[entry.kind];
  const hasBody = entry.body !== undefined;
  const text = hasBody ? (typeof entry.body === 'string' ? entry.body : JSON.stringify(entry.body, null, expanded ? 2 : 0)) : '';
  return (
    <Pressable style={styles.row} onPress={() => hasBody && setExpanded(!expanded)}>
      <View style={styles.rowHeader}>
        <Text style={[styles.kind, { color: kind.color }]}>
          {kind.mark} {kind.label}
        </Text>
        <Text style={styles.title} numberOfLines={1}>
          {entry.title}
        </Text>
        <Text style={styles.meta}>
          {entry.replyTo !== undefined && `to #${entry.replyTo} · ${entry.ms} ms · `}#{entry.id} · {time(entry.at)}
        </Text>
      </View>
      {hasBody && (
        <Text style={[nodeStyles.mono, styles.body]} numberOfLines={expanded ? undefined : 2} selectable>
          {text}
        </Text>
      )}
    </Pressable>
  );
});

// Every message between this page and the backend, newest first.
export function Log() {
  const { entries, clear } = useBackendContext();
  return (
    <View style={styles.panel}>
      <View style={styles.header}>
        <Text style={styles.heading}>Messages</Text>
        <Text style={styles.meta}>{entries.length}</Text>
        <View style={{ flex: 1 }} />
        <Pressable onPress={clear}>
          <Text style={styles.clear}>Clear</Text>
        </Pressable>
      </View>
      <ScrollView contentContainerStyle={styles.list}>
        {entries.length === 0 && <Text style={styles.meta}>No messages yet. Log in on the Backend node.</Text>}
        {entries.map((entry) => (
          <Row key={entry.id} entry={entry} />
        ))}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  panel: { width: 440, borderLeftWidth: 1, borderLeftColor: '#D5DBE1', backgroundColor: '#FFFFFF' },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    padding: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#E3E7EB',
  },
  heading: { fontSize: 15, fontWeight: '600', color: '#19212A' },
  clear: { fontSize: 12, color: '#2F6FDE' },
  list: { padding: 8, gap: 6 },
  row: { borderWidth: 1, borderColor: '#E3E7EB', borderRadius: 8, padding: 8, gap: 4 },
  rowHeader: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  kind: { fontSize: 12, fontWeight: '600' },
  title: { flex: 1, fontSize: 12, color: '#19212A' },
  meta: { fontSize: 11, color: '#7A858F' },
  body: { fontSize: 11, color: '#3A444E' },
});
