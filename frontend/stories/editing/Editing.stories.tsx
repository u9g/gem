import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Copy, Pencil, Undo2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import { EditableReply, Quote, type ReplyDraft, Theirs } from '../../surfaces';
import { Button, Icon, Markdown, t3, visibleMarkdown } from '../../t3';
import { proposals } from '../fixtures';

const threadReply = proposals[1];
const quoteReply = proposals[3];

// A reply the agent suggested and u9g may rewrite before approving.
function useDraft(suggested: string, edited?: string) {
  const [text, setText] = useState((edited ?? suggested).trim());
  return { text, setText, edited: text.trim() !== suggested.trim(), revert: () => setText(suggested.trim()) };
}

// A multiline input styled as message text; field-sizing makes it exactly as tall as its text.
function GrowingInput({ value, onChange, autoFocus }: { value: string; onChange: (text: string) => void; autoFocus?: boolean }) {
  return <TextInput multiline autoFocus={autoFocus} value={value} onChangeText={onChange} style={styles.input} />;
}

// T3 Code's message actions: ghost icon buttons under the bubble, revealed on hover.
function MessageActions({ shown, children }: { shown: boolean; children: ReactNode }) {
  return <View style={[styles.actions, { opacity: shown ? 1 : 0 }]}>{children}</View>;
}

function GhostIcon({ icon, label, onPress }: { icon: typeof Copy; label: string; onPress?: () => void }) {
  return (
    <Pressable accessibilityLabel={label} onPress={onPress} style={styles.ghostIcon}>
      <Icon icon={icon} size={12} />
    </Pressable>
  );
}

function Conversation({ quote, children }: { quote?: boolean; children: ReactNode }) {
  const proposal = quote ? quoteReply : threadReply;
  const comment = proposal.comment!;
  return (
    <View style={styles.conversation}>
      {!quote && <Theirs comment={comment} />}
      {children}
    </View>
  );
}

function quoted(): string {
  return visibleMarkdown(quoteReply.comment!.body).split('\n\n')[0].replace(/[#*]/g, '').trim();
}

// ---------- A: edit in place ----------

// The bubble is the editor: click it and type. Under it, T3 Code's hover actions add Revert once
// the text differs from the suggestion, and an "Edited" label says so even without hovering.
function InPlace({ quote = false, edited }: { quote?: boolean; edited?: string }) {
  const proposal = quote ? quoteReply : threadReply;
  const draft = useDraft(proposal.reply, edited);
  const [focused, setFocused] = useState(false);
  const [hovered, setHovered] = useState(false);
  return (
    <Conversation quote={quote}>
      <View onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)}>
        <View style={styles.right}>
          <View
            style={[styles.bubble, focused && styles.bubbleFocused]}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
          >
            {quote && <Quote lines={3}>{quoted()}</Quote>}
            <GrowingInput value={draft.text} onChange={draft.setText} />
          </View>
        </View>
        <View style={styles.statusRow}>
          {draft.edited && <Text style={styles.status}>Edited</Text>}
          <MessageActions shown={hovered || focused}>
            {draft.edited && <GhostIcon icon={Undo2} label="Revert to the suggestion" onPress={draft.revert} />}
            <GhostIcon icon={Copy} label="Copy" />
          </MessageActions>
        </View>
      </View>
    </Conversation>
  );
}

// ---------- B: edit in a composer ----------

// Like T3 Code's queued messages: a pencil opens the text in a composer below the conversation,
// the bubble being edited is highlighted, and Save or Cancel closes it.
function Composer({ quote = false, edited, editing: startEditing = false }: { quote?: boolean; edited?: string; editing?: boolean }) {
  const proposal = quote ? quoteReply : threadReply;
  const draft = useDraft(proposal.reply, edited);
  const [editing, setEditing] = useState(startEditing ? draft.text : undefined);
  const [hovered, setHovered] = useState(false);
  return (
    <Conversation quote={quote}>
      <View onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)}>
        <View style={styles.right}>
          <View style={[styles.bubble, editing !== undefined && styles.bubbleEditing]}>
            {quote && <Quote lines={3}>{quoted()}</Quote>}
            <Markdown>{draft.text}</Markdown>
          </View>
        </View>
        <View style={styles.statusRow}>
          {editing !== undefined ? (
            <Text style={styles.status}>Editing below</Text>
          ) : (
            draft.edited && <Text style={styles.status}>Edited</Text>
          )}
          <MessageActions shown={hovered && editing === undefined}>
            {draft.edited && <GhostIcon icon={Undo2} label="Revert to the suggestion" onPress={draft.revert} />}
            <GhostIcon icon={Pencil} label="Edit in the composer" onPress={() => setEditing(draft.text)} />
          </MessageActions>
        </View>
      </View>
      {editing !== undefined && (
        <View style={styles.composer}>
          <GrowingInput value={editing} onChange={setEditing} autoFocus />
          <View style={styles.composerButtons}>
            <Button label="Cancel" variant="ghost" onPress={() => setEditing(undefined)} />
            <Button
              label="Save"
              variant="default"
              onPress={() => {
                draft.setText(editing);
                setEditing(undefined);
              }}
            />
          </View>
        </View>
      )}
    </Conversation>
  );
}

// ---------- C: two versions ----------

// Once edited, a small Suggested / Yours switch sits under the bubble, so both versions stay one
// click away; reverting is picking Suggested. The bubble is always editable.
function TwoVersions({ quote = false, edited }: { quote?: boolean; edited?: string }) {
  const proposal = quote ? quoteReply : threadReply;
  const [draft, setDraft] = useState<ReplyDraft>(
    edited ? { yours: edited, showing: 'yours' } : { yours: proposal.reply, showing: 'suggested' },
  );
  return (
    <Conversation quote={quote}>
      <EditableReply
        suggested={proposal.reply}
        draft={draft}
        onChange={setDraft}
        quote={quote ? quoted() : undefined}
      />
    </Conversation>
  );
}

// ---------- Layout ----------

const yourEdit =
  "Yes. It now retries up to `conn_options.max_retry` times like STT, but I capped TTS at 2 since a third replay is never worth it.\n";

function Column({ name, note, children }: { name: string; note: string; children: ReactNode }) {
  return (
    <View style={styles.column}>
      <Text style={styles.name}>{name}</Text>
      <Text style={styles.note}>{note}</Text>
      {children}
    </View>
  );
}

function State({ label, children }: { label: string; children: ReactNode }) {
  return (
    <View style={{ gap: 6 }}>
      <Text style={styles.stateLabel}>{label}</Text>
      <View style={styles.frame}>{children}</View>
    </View>
  );
}

function Board() {
  return (
    <View style={styles.board}>
      <Column name="A · Edit in place" note="Click the bubble and type. Hover shows Revert once it differs.">
        <State label="Suggested">
          <InPlace />
        </State>
        <State label="After editing (hover for Revert)">
          <InPlace edited={yourEdit} />
        </State>
        <State label="Quote reply">
          <InPlace quote />
        </State>
      </Column>
      <Column name="B · Edit in a composer" note="Pencil on hover opens the text below, like T3 Code's queued messages.">
        <State label="Suggested (hover for the pencil)">
          <Composer />
        </State>
        <State label="Editing">
          <Composer editing />
        </State>
        <State label="After saving">
          <Composer edited={yourEdit} />
        </State>
      </Column>
      <Column name="C · Two versions" note="Always editable; a Suggested / Yours switch appears once you change it.">
        <State label="Suggested">
          <TwoVersions />
        </State>
        <State label="After editing">
          <TwoVersions edited={yourEdit} />
        </State>
        <State label="Quote reply, edited">
          <TwoVersions quote edited={'No release needed: this only changes an internal retry, so no changeset.\n'} />
        </State>
      </Column>
    </View>
  );
}

const meta: Meta = { title: 'Proposals/Editing replies', parameters: { layout: 'fullscreen' } };
export default meta;

export const Approaches: StoryObj = { render: () => <Board /> };
export const EditInPlace: StoryObj = { name: 'A · Edit in place', render: () => <View style={styles.single}><InPlace /></View> };
export const EditInComposer: StoryObj = { name: 'B · Edit in a composer', render: () => <View style={styles.single}><Composer /></View> };
export const TwoVersionsStory: StoryObj = { name: 'C · Two versions', render: () => <View style={styles.single}><TwoVersions /></View> };

const styles = StyleSheet.create({
  board: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 24,
    padding: 20,
    minHeight: '100%',
    backgroundColor: t3.background,
  },
  column: {
    width: 380,
    gap: 12,
  },
  name: {
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '600',
  },
  note: {
    color: t3.muted,
    fontSize: 12,
    marginTop: -10,
  },
  stateLabel: {
    color: t3.faint,
    fontSize: 11,
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  frame: {
    padding: 10,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  single: {
    width: 400,
    padding: 16,
    backgroundColor: t3.background,
  },
  conversation: {
    gap: 10,
  },
  right: {
    alignItems: 'flex-end',
  },
  bubble: {
    maxWidth: '80%',
    width: '80%',
    gap: 8,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: 'transparent',
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  bubbleFocused: {
    borderColor: 'rgba(55, 103, 232, 0.7)',
  },
  bubbleEditing: {
    backgroundColor: 'rgba(55, 103, 232, 0.14)',
  },
  input: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
    padding: 0,
    outlineStyle: 'solid',
    outlineWidth: 0,
    fieldSizing: 'content',
    resize: 'none',
  } as object,
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 6,
    minHeight: 24,
    marginTop: 2,
  },
  status: {
    color: t3.muted,
    fontSize: 11,
  },
  actions: {
    flexDirection: 'row',
    gap: 2,
  },
  ghostIcon: {
    width: 24,
    height: 24,
    borderRadius: 6,
    alignItems: 'center',
    justifyContent: 'center',
  },
  composer: {
    gap: 8,
    padding: 12,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: t3.input,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  composerButtons: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 6,
  },
});
