import { Handle, Position } from '@xyflow/react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import { gemProfiles, type Login, type Status, useBackendContext } from './backend';
import { hasFields, type Message, messages, template } from './messages';

export const statusColor: Record<Status, string> = {
  'logged-out': '#9AA4AE',
  connecting: '#D9A400',
  unreachable: '#D1453B',
  offline: '#D9A400',
  online: '#2E9E5B',
};

type CardProps = { title: string; status: Status; label: string; handle: ReactNode; children: ReactNode };

function Card({ title, status, label, handle, children }: CardProps) {
  return (
    <View style={styles.card}>
      {handle}
      <View style={styles.header}>
        <View style={[styles.dot, { backgroundColor: statusColor[status] }]} />
        <Text style={styles.title}>{title}</Text>
      </View>
      <Text style={styles.label}>{label}</Text>
      {/* The body takes clicks and typing; the header still drags the node. */}
      <div className="nodrag nowheel" style={{ cursor: 'default' }}>
        <View style={styles.body}>{children}</View>
      </div>
    </View>
  );
}

function Button({ label, onPress, disabled, primary }: { label: string; onPress: () => void; disabled?: boolean; primary?: boolean }) {
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      style={[styles.button, primary && styles.primary, disabled && styles.disabled]}
    >
      <Text style={[styles.buttonText, primary && styles.primaryText]}>{label}</Text>
    </Pressable>
  );
}

const frontendLabel: Record<Status, (identity: string, room?: string) => string> = {
  'logged-out': () => 'Log in to the backend to register as its client',
  connecting: (_, room) => `Joining room ${room}…`,
  unreachable: () => 'LiveKit refused the login',
  offline: (identity, room) => `In room ${room} as ${identity}, waiting for the backend to join`,
  online: (identity, room) => `Registered with the backend as ${identity} in room ${room}`,
};

// This page as the backend's client: a button for each request the backend answers. One with
// fields opens its body to fill in first.
export function FrontendNode() {
  const { status, identity, login, send } = useBackendContext();
  const [open, setOpen] = useState<Message>();
  const [body, setBody] = useState('');
  const [invalid, setInvalid] = useState<string>();
  const online = status === 'online';

  function choose(message: Message) {
    if (!hasFields(message)) {
      send(message.method, message.path, template(message));
      return;
    }
    setOpen(message);
    setBody(JSON.stringify(template(message), null, 2));
    setInvalid(undefined);
  }

  function submit() {
    if (!open) return;
    try {
      send(open.method, open.path, JSON.parse(body));
      setOpen(undefined);
    } catch (error) {
      setInvalid((error as Error).message);
    }
  }

  return (
    <Card
      title="Frontend"
      status={status}
      label={frontendLabel[status](identity, login?.room)}
      handle={<Handle type="source" position={Position.Right} />}
    >
      <View style={styles.buttons}>
        {messages.map((message) => (
          <Button
            key={message.path}
            label={hasFields(message) ? `${message.label}…` : message.label}
            onPress={() => choose(message)}
            disabled={!online}
            primary={open === message}
          />
        ))}
      </View>
      {open && (
        <View style={styles.composer}>
          <Text style={styles.mono}>
            {open.method} {open.path}
          </Text>
          <TextInput style={[styles.input, styles.mono, styles.bodyInput]} value={body} onChangeText={setBody} multiline />
          {invalid && <Text style={styles.invalid}>{invalid}</Text>}
          <View style={styles.row}>
            <Button label="Send" onPress={submit} disabled={!online} primary />
            <Button label="Cancel" onPress={() => setOpen(undefined)} />
          </View>
        </View>
      )}
    </Card>
  );
}

const backendLabel: Record<Status, string> = {
  'logged-out': 'Not logged in',
  connecting: 'Connecting…',
  unreachable: 'Unreachable: check the URL, API key and secret',
  offline: 'Not in the room yet: it joins once a frontend has',
  online: 'Online',
};

const fields: { key: keyof Login; label: string; placeholder: string; secret?: boolean }[] = [
  { key: 'url', label: 'LiveKit URL', placeholder: 'wss://project.livekit.cloud' },
  { key: 'apiKey', label: 'API key', placeholder: 'LIVEKIT_API_KEY' },
  { key: 'apiSecret', label: 'API secret', placeholder: 'LIVEKIT_API_SECRET', secret: true },
  { key: 'room', label: 'Room', placeholder: 'GEM_ROOM' },
];

// The gem server, reached through a room in the user's LiveKit project: the login to reach it by.
export function BackendNode() {
  const { status, login, saveLogin } = useBackendContext();
  const [editing, setEditing] = useState(!login);
  const [draft, setDraft] = useState<Login>(login ?? { url: '', apiKey: '', apiSecret: '', room: 'gem' });
  const profiles = gemProfiles();
  const complete = fields.every(({ key }) => draft[key].trim());

  return (
    <Card
      title="Backend"
      status={status}
      label={`gem server · ${backendLabel[status]}`}
      handle={<Handle type="target" position={Position.Left} />}
    >
      {editing ? (
        <>
          {profiles.length > 0 && (
            <View style={styles.row}>
              <Text style={styles.label}>From gem:</Text>
              {profiles.map((p) => (
                <Button
                  key={p.name}
                  label={p.name}
                  onPress={() => setDraft({ url: p.url, apiKey: p.apiKey, apiSecret: p.apiSecret, room: p.room })}
                />
              ))}
            </View>
          )}
          {fields.map(({ key, label, placeholder, secret }) => (
            <View key={key} style={styles.field}>
              <Text style={styles.fieldLabel}>{label}</Text>
              <TextInput
                style={styles.input}
                value={draft[key]}
                placeholder={placeholder}
                placeholderTextColor="#9AA4AE"
                secureTextEntry={secret}
                autoCapitalize="none"
                autoCorrect={false}
                onChangeText={(value) => setDraft({ ...draft, [key]: value })}
              />
            </View>
          ))}
          <View style={styles.row}>
            <Button
              label="Log in"
              primary
              disabled={!complete}
              onPress={() => {
                saveLogin({
                  url: draft.url.trim(),
                  apiKey: draft.apiKey.trim(),
                  apiSecret: draft.apiSecret.trim(),
                  room: draft.room.trim(),
                });
                setEditing(false);
              }}
            />
            {login && <Button label="Cancel" onPress={() => setEditing(false)} />}
          </View>
        </>
      ) : (
        <>
          <Text style={styles.mono}>{login?.url}</Text>
          <Text style={styles.mono}>room {login?.room}</Text>
          <View style={styles.row}>
            <Button
              label="Edit login"
              onPress={() => {
                if (login) setDraft(login);
                setEditing(true);
              }}
            />
            <Button
              label="Log out"
              onPress={() => {
                saveLogin(undefined);
                setEditing(true);
              }}
            />
          </View>
        </>
      )}
    </Card>
  );
}

export const nodeTypes = { frontend: FrontendNode, backend: BackendNode };

export const styles = StyleSheet.create({
  card: {
    width: 340,
    backgroundColor: '#FFFFFF',
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#D5DBE1',
    padding: 14,
    gap: 6,
    shadowColor: '#000',
    shadowOpacity: 0.08,
    shadowRadius: 12,
    shadowOffset: { width: 0, height: 4 },
  },
  header: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  dot: { width: 10, height: 10, borderRadius: 5 },
  title: { fontSize: 17, fontWeight: '600', color: '#19212A' },
  label: { fontSize: 12, color: '#5B6670' },
  body: { gap: 8, marginTop: 6 },
  buttons: { flexDirection: 'row', flexWrap: 'wrap', gap: 6 },
  row: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: 6 },
  button: {
    paddingHorizontal: 10,
    paddingVertical: 6,
    borderRadius: 6,
    borderWidth: 1,
    borderColor: '#C9D1D9',
    backgroundColor: '#F6F8FA',
  },
  primary: { backgroundColor: '#19212A', borderColor: '#19212A' },
  disabled: { opacity: 0.4 },
  buttonText: { fontSize: 12, color: '#19212A' },
  primaryText: { color: '#FFFFFF' },
  composer: { gap: 6, borderTopWidth: 1, borderTopColor: '#E3E7EB', paddingTop: 8 },
  field: { gap: 2 },
  fieldLabel: { fontSize: 11, fontWeight: '600', color: '#5B6670' },
  input: {
    borderWidth: 1,
    borderColor: '#C9D1D9',
    borderRadius: 6,
    paddingHorizontal: 8,
    paddingVertical: 6,
    fontSize: 13,
    color: '#19212A',
  },
  bodyInput: { minHeight: 120 },
  mono: { fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace', fontSize: 12, color: '#19212A' },
  invalid: { fontSize: 12, color: '#D1453B' },
});
