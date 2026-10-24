import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { CircleAlert, ExternalLink, MessageCircle } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from '../../gen/gem_pb';
import { bySubject, ProposalGroup } from '../../ProposalView';
import {
  Bubble,
  DiffSurface,
  EditableReply,
  Html,
  type ReplyDraft,
  ClosesTag,
  ResolvesTag,
  ToolRequestBanner,
} from '../../surfaces';
import { Button, Icon, proposalIcon, stepSentence, t3 } from '../../t3';
import { proposals } from '../fixtures';
import { long } from './long';

const groups = [long, ...bySubject(proposals)];

// ---------- shared ----------

// Cuts its content to `max` pixels, fading out the last lines, with Show more under it when
// anything was cut.
function Clamp({ max, children, align = 'left' }: { max: number; children: ReactNode; align?: 'left' | 'right' }) {
  const [open, setOpen] = useState(false);
  const [height, setHeight] = useState(0);
  const cut = height > max + 24;
  return (
    <View>
      <View
        style={[
          { overflow: 'hidden' },
          cut && !open && ({ maxHeight: max, maskImage: 'linear-gradient(to bottom, black 60%, transparent)' } as object),
        ]}
      >
        <View onLayout={(event) => setHeight(event.nativeEvent.layout.height)}>{children}</View>
      </View>
      {cut && (
        <Pressable onPress={() => setOpen(!open)} style={{ alignSelf: align === 'right' ? 'flex-end' : 'flex-start' }}>
          <Text style={styles.more}>{open ? 'Show less' : 'Show more'}</Text>
        </Pressable>
      )}
    </View>
  );
}

// Their comment as a message, cut to `max` pixels.
function Comment({ proposal, max }: { proposal: Proposal; max: number }) {
  const comment = proposal.comment!;
  return (
    <View style={{ gap: 4 }}>
      <View style={styles.author}>
        <Text style={styles.meta}>{comment.author}</Text>
        <Icon icon={ExternalLink} size={12} />
      </View>
      <Bubble side="left">
        <Clamp max={max}>
          <Html html={comment.bodyHtml} />
        </Clamp>
      </Bubble>
    </View>
  );
}

// u9g's reply rendered as it will look on GitHub, cut to `max` pixels. Clicking it switches to the
// editable text.
function Reply({ proposal, max }: { proposal: Proposal; max: number }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<ReplyDraft>({ yours: proposal.reply, showing: 'suggested' });
  if (!proposal.reply) return null;
  return (
    <View style={{ gap: 6 }}>
      {editing ? (
        <EditableReply suggested={proposal.reply} draft={draft} onChange={setDraft} />
      ) : (
        <Bubble side="right">
          <Clamp max={max}>
            <Html html={proposal.replyHtml} onPress={() => setEditing(true)} />
          </Clamp>
        </Bubble>
      )}
      {(proposal.resolveThread || proposal.closePullRequest) && (
        <View style={styles.rightRow}>
          {proposal.resolveThread && <ResolvesTag />}
          {proposal.closePullRequest && <ClosesTag />}
        </View>
      )}
    </View>
  );
}

function Change({ proposal, max }: { proposal: Proposal; max: number }) {
  if (proposal.requestedTool) return <ToolRequestBanner proposal={proposal} />;
  if (!proposal.patch) return null;
  return (
    <View style={{ gap: 6 }}>
      <Text style={styles.meta}>{proposal.patch.commitMessage}</Text>
      <Clamp max={max}>
        <DiffSurface
          diff={proposal.patch.diff}
          header={proposal.error ? <Icon icon={CircleAlert} size={12} color={t3.deletion} /> : undefined}
        />
      </Clamp>
      {proposal.error !== '' && <Text style={styles.error}>{proposal.error}</Text>}
    </View>
  );
}

function Actions({ proposal }: { proposal: Proposal }) {
  return (
    <View style={styles.buttons}>
      <Button label="Dismiss" />
      {!proposal.requestedTool && <Button label={proposal.error ? 'Retry' : 'Approve'} variant="default" />}
    </View>
  );
}

function Summary({ proposal }: { proposal: Proposal }) {
  return (
    <View style={styles.summaryRow}>
      <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
      <Text style={styles.summary}>{proposal.summary}</Text>
    </View>
  );
}

// The alternatives for one comment as a segmented control, labelled by what approving each does.
function Options({ group, picked, onPick }: { group: Proposal[]; picked: number; onPick: (i: number) => void }) {
  if (group.length < 2) return null;
  return (
    <View style={styles.tabs}>
      {group.map((option, i) => (
        <Pressable key={option.id} onPress={() => onPick(i)} style={[styles.tab, i === picked && styles.tabOn]}>
          <Text style={[styles.tabText, i === picked && { color: t3.foreground }]}>{stepSentence(option)}</Text>
        </Pressable>
      ))}
    </View>
  );
}

function Where({ proposal }: { proposal: Proposal }) {
  return <Text style={styles.where}>agents#4821 · {proposal.threadId ? 'review thread' : 'conversation'}</Text>;
}

function Page({ children }: { children: ReactNode }) {
  return (
    <View style={styles.page}>
      <View style={styles.column}>
        <Text style={styles.title}>TTS retry fixes</Text>
        {children}
      </View>
    </View>
  );
}

// ---------- A: clamped ----------

// Today's layout with long parts cut: comments and replies to about eight lines, diffs to about
// twelve, each with Show more. Replies render as GitHub will show them, so links are words, not
// URLs; clicking one edits it. Alternatives are tabs instead of a stack.
function ClampedGroup({ group }: { group: Proposal[] }) {
  const [picked, setPicked] = useState(0);
  const proposal = group[picked];
  return (
    <View style={styles.conversation}>
      <Where proposal={proposal} />
      <View style={styles.split}>
        <View style={styles.left}>
          <Comment proposal={proposal} max={150} />
          <Options group={group} picked={picked} onPick={setPicked} />
          <Summary proposal={proposal} />
          <Reply key={proposal.id} proposal={proposal} max={150} />
          <Actions proposal={proposal} />
        </View>
        <View style={styles.right}>
          {proposal.patch || proposal.requestedTool ? (
            <Change proposal={proposal} max={230} />
          ) : (
            <View style={styles.noChange}>
              <Icon icon={MessageCircle} />
              <Text style={styles.meta}>No code change: approving only replies</Text>
            </View>
          )}
        </View>
      </View>
    </View>
  );
}

// ---------- B: thread | answer ----------

// Their side on the left, u9g's on the right: the reply moves next to the diff, so a reply-only
// answer fills the right column instead of leaving it empty.
function ThreadAnswerGroup({ group }: { group: Proposal[] }) {
  const [picked, setPicked] = useState(0);
  const proposal = group[picked];
  return (
    <View style={styles.conversation}>
      <Where proposal={proposal} />
      <View style={styles.split}>
        <View style={styles.halfLeft}>
          <Comment proposal={proposal} max={220} />
        </View>
        <View style={styles.halfRight}>
          <Options group={group} picked={picked} onPick={setPicked} />
          <Summary proposal={proposal} />
          <Reply key={proposal.id} proposal={proposal} max={150} />
          <Change proposal={proposal} max={200} />
          <Actions proposal={proposal} />
        </View>
      </View>
    </View>
  );
}

const meta: Meta = { title: 'Compact proposals', parameters: { layout: 'fullscreen' } };
export default meta;

export const AClamped: StoryObj = {
  name: 'A · Clamped',
  render: () => (
    <Page>
      {groups.map((group) => (
        <ClampedGroup key={group[0].id} group={group} />
      ))}
    </Page>
  ),
};
export const BThreadAnswer: StoryObj = {
  name: 'B · Thread | answer',
  render: () => (
    <Page>
      {groups.map((group) => (
        <ThreadAnswerGroup key={group[0].id} group={group} />
      ))}
    </Page>
  ),
};
// The app's design.
export const CDigest: StoryObj = {
  name: 'C · Digest',
  render: () => (
    <Page>
      {groups.map((group) => (
        <ProposalGroup key={group[0].id} proposals={group} onApprove={() => {}} onDismiss={() => {}} />
      ))}
    </Page>
  ),
};

const styles = StyleSheet.create({
  page: {
    minHeight: '100%',
    alignItems: 'center',
    paddingHorizontal: 24,
    paddingVertical: 32,
    backgroundColor: t3.background,
  },
  column: {
    width: '100%',
    maxWidth: 1240,
    gap: 24,
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
  more: {
    color: t3.muted,
    fontSize: 12,
    paddingTop: 4,
    textDecorationLine: 'underline',
  },
  error: {
    color: t3.deletion,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
  },
  author: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    paddingLeft: 14,
  },
  where: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  conversation: {
    gap: 12,
    paddingBottom: 24,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  split: {
    flexDirection: 'row',
    gap: 24,
    alignItems: 'flex-start',
  },
  left: {
    flex: 4,
    gap: 10,
  },
  right: {
    flex: 5,
    gap: 8,
  },
  halfLeft: {
    flex: 1,
  },
  halfRight: {
    flex: 1,
    gap: 10,
  },
  rightRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 6,
  },
  buttons: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 6,
  },
  summaryRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 6,
  },
  summary: {
    flex: 1,
    color: t3.foreground,
    fontSize: 14,
    lineHeight: 22,
  },
  noChange: {
    flexDirection: 'row',
    alignItems: 'center',
    padding: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t3.input,
  },
  tabs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignSelf: 'flex-start',
    gap: 2,
    padding: 3,
    borderRadius: 10,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  tab: {
    height: 26,
    justifyContent: 'center',
    paddingHorizontal: 8,
    borderRadius: 8,
  },
  tabOn: {
    backgroundColor: 'rgba(255, 255, 255, 0.08)',
  },
  tabText: {
    color: t3.muted,
    fontSize: 12,
  },
});
