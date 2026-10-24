import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import {
  CheckCheck,
  CircleAlert,
  CircleCheck,
  CornerDownRight,
  FileCode,
  FileDiff,
  Folder,
  Hammer,
  MessageCircle,
  MessageSquareQuote,
  SquarePen,
} from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from '../../gen/gem_pb';
import { Bubble, DiffSurface, Quote, ResolvesTag, Theirs, ToolRequestBanner } from '../../surfaces';
import { basename, Button, Code, DiffStat, diffFiles, Icon, Markdown, Row, t3, visibleMarkdown } from '../../t3';
import { proposals } from '../fixtures';

const patchProposal = proposals[0];
const threadReply = proposals[0];
const quoteReply = proposals[3];
const toolRequest = proposals[4];
const failed = proposals[5];

// Lays the approaches for one action side by side, each in a sidebar-wide column.
function Approaches({ children }: { children: ReactNode }) {
  return <View style={styles.approaches}>{children}</View>;
}

function Approach({ name, note, children }: { name: string; note: string; children: ReactNode }) {
  return (
    <View style={styles.approach}>
      <Text style={styles.approachName}>{name}</Text>
      <Text style={styles.approachNote}>{note}</Text>
      <View style={styles.frame}>{children}</View>
    </View>
  );
}

// ---------- Patch ----------

// T3 Code's end-of-turn changed-files card: a header with the totals and Open diff, then a
// folder tree. The diff itself opens in a side panel.
function ChangedFilesCard({ proposal }: { proposal: Proposal }) {
  const [open, setOpen] = useState(false);
  const files = diffFiles(proposal.patch!.diff);
  const dirs = files[0].path.split('/').slice(0, -1);
  return (
    <View style={{ gap: 8 }}>
      <View style={styles.card}>
        <View style={styles.cardHeader}>
          <Text style={styles.cardTitle}>
            {files.length} changed {files.length === 1 ? 'file' : 'files'}{' '}
            <DiffStat
              added={files.reduce((n, f) => n + f.added, 0)}
              removed={files.reduce((n, f) => n + f.removed, 0)}
            />
          </Text>
          <Pressable style={styles.ghostButton} onPress={() => setOpen(!open)}>
            <Icon icon={FileDiff} size={12} />
            <Text style={styles.ghostButtonText}>{open ? 'Close diff' : 'Open diff'}</Text>
          </Pressable>
        </View>
        <View style={styles.treeRow}>
          <Icon icon={Folder} size={14} />
          <Text numberOfLines={1} style={styles.treeText}>
            {dirs.join('/')}
          </Text>
        </View>
        {files.map((file) => (
          <View key={file.path} style={[styles.treeRow, { paddingLeft: 22 }]}>
            <Icon icon={FileCode} size={14} />
            <Text style={[styles.treeText, { color: t3.foreground, flex: 1 }]}>{basename(file.path)}</Text>
            <DiffStat added={file.added} removed={file.removed} />
          </View>
        ))}
        <Text style={styles.commit}>{proposal.patch!.commitMessage}</Text>
      </View>
      {open && <DiffSurface diff={proposal.patch!.diff} />}
    </View>
  );
}

function PatchStory() {
  return (
    <Approaches>
      <Approach name="A · File row" note="Today: one row per file, raw diff on click">
        {diffFiles(patchProposal.patch!.diff).map((file) => (
          <Row
            key={file.path}
            icon={SquarePen}
            label={basename(file.path)}
            trailing={<DiffStat added={file.added} removed={file.removed} />}
            initiallyOpen
          >
            <Text style={styles.meta}>{patchProposal.patch!.commitMessage}</Text>
            <Code>{patchProposal.patch!.diff}</Code>
          </Row>
        ))}
      </Approach>
      <Approach name="B · Diff surface" note="T3 Code's diff renderer: tinted lines, line numbers, scrolls sideways">
        <Text style={[styles.meta, { marginBottom: 6 }]}>{patchProposal.patch!.commitMessage}</Text>
        <DiffSurface diff={patchProposal.patch!.diff} />
      </Approach>
      <Approach name="C · Changed-files card" note="T3 Code's end-of-turn card; Open diff shows the diff">
        <ChangedFilesCard proposal={patchProposal} />
      </Approach>
    </Approaches>
  );
}

// ---------- Replies ----------

function ThreadReplyStory() {
  const comment = threadReply.comment!;
  return (
    <Approaches>
      <Approach name="A · Step row" note="Today: one row, the reply on click">
        <Row icon={MessageCircle} label="Reply in thread" initiallyOpen>
          <Markdown>{threadReply.reply}</Markdown>
        </Row>
      </Approach>
      <Approach name="B · Conversation" note="Their comment, then your reply as a T3 Code message bubble">
        <Theirs comment={comment} />
        <Bubble>
          <Markdown>{threadReply.reply}</Markdown>
        </Bubble>
      </Approach>
      <Approach name="C · One-line preview" note="The reply's first words in the row; click for the rest">
        <Row
          icon={CornerDownRight}
          label={
            <>
              <Text style={{ color: t3.foreground }}>{comment.author}</Text> {`“${threadReply.reply.trim()}”`}
            </>
          }
        >
          <Markdown>{threadReply.reply}</Markdown>
        </Row>
      </Approach>
    </Approaches>
  );
}

function QuoteReplyStory() {
  const comment = quoteReply.comment!;
  const firstParagraph = visibleMarkdown(comment.body).split('\n\n')[0].replace(/[#*]/g, '').trim();
  return (
    <Approaches>
      <Approach name="A · Step row" note="Today: one row, the reply on click">
        <Row icon={MessageSquareQuote} label="Quote reply" initiallyOpen>
          <Markdown>{quoteReply.reply}</Markdown>
        </Row>
      </Approach>
      <Approach name="B · Exact comment" note="What GitHub will show: the quote, then your reply, in a bubble">
        <Bubble>
          <Quote lines={3}>{firstParagraph}</Quote>
          <Markdown>{quoteReply.reply}</Markdown>
        </Bubble>
      </Approach>
      <Approach name="C · Context chip" note="Like T3 Code's composer: who you're quoting as a chip above the text">
        <View style={styles.chip}>
          <Icon icon={MessageSquareQuote} size={12} />
          <Text numberOfLines={1} style={styles.chipText}>
            {comment.author}: {firstParagraph}
          </Text>
        </View>
        <Markdown>{quoteReply.reply}</Markdown>
      </Approach>
    </Approaches>
  );
}

// ---------- Resolve ----------

function ResolveStory() {
  return (
    <Approaches>
      <Approach name="A · Step row" note="Today: its own row after the reply">
        <Row icon={MessageCircle} label="Reply in thread" />
        <Row icon={CheckCheck} label="Resolve thread" />
      </Approach>
      <Approach name="B · Folded into the reply" note="No row of its own: the reply row says it resolves">
        <Row
          icon={MessageCircle}
          label="Reply in thread"
          trailing={<ResolvesTag />}
        />
      </Approach>
      <Approach name="C · Outcome line" note="A muted line under the reply bubble, like a status under a message">
        <Bubble>
          <Markdown>{threadReply.reply}</Markdown>
        </Bubble>
        <View style={styles.outcome}>
          <Icon icon={CircleCheck} size={12} color={t3.addition} />
          <Text style={styles.outcomeText}>Then resolves the thread</Text>
        </View>
      </Approach>
    </Approaches>
  );
}

// ---------- Tool request ----------

function ToolRequestStory() {
  const [name, ...rest] = toolRequest.summary.split(':');
  return (
    <Approaches>
      <Approach name="A · Step row" note="Today: one row, the request on click">
        <Row icon={Hammer} label="Needs a tool" initiallyOpen>
          <Markdown>{toolRequest.requestedTool}</Markdown>
        </Row>
      </Approach>
      <Approach name="B · Warning banner" note="T3 Code's warning banner: amber label, tool name in mono, the why below">
        <ToolRequestBanner proposal={toolRequest} />
      </Approach>
      <Approach name="C · Tool chip" note="The tool's name as a code chip with a one-line why; click for more">
        <Row
          label={
            <>
              <Text style={styles.codeChip}> {name} </Text> {rest.join(':').trim()}
            </>
          }
        >
          <Markdown>{toolRequest.requestedTool}</Markdown>
        </Row>
      </Approach>
    </Approaches>
  );
}

// ---------- Failed approval ----------

function ErrorStory() {
  const [command, ...output] = failed.error.split(': ');
  return (
    <Approaches>
      <Approach name="A · Red row" note="Today: a red row, the error on click">
        <Row icon={CircleAlert} label="Approval failed" labelColor={t3.deletion} initiallyOpen>
          <Code>{failed.error}</Code>
        </Row>
      </Approach>
      <Approach name="B · Command block" note="T3 Code's command block for what failed, its output below in red">
        <View style={styles.commandBlock}>
          <Text style={styles.mono}>
            <Text style={{ color: t3.muted }}>$ </Text>
            {command} apply
          </Text>
        </View>
        <Text style={[styles.mono, { color: '#FCA5A5', marginTop: 6 }]}>{output.join(': ')}</Text>
      </Approach>
      <Approach name="C · Failed step" note="The step that failed gets the error icon; the rest stay as they are">
        <Row
          icon={SquarePen}
          label={basename(diffFiles(failed.patch!.diff)[0].path)}
          labelColor={t3.deletion}
          trailing={<Icon icon={CircleAlert} size={12} color={t3.deletion} />}
          initiallyOpen
        >
          <Text style={[styles.meta, { color: t3.deletion }]}>Patch no longer applies to the branch</Text>
        </Row>
        <Row icon={MessageCircle} label="Reply in thread" />
        <Row icon={CheckCheck} label="Resolve thread" />
        <View style={[styles.buttonsRow]}>
          <Button label="Retry" variant="default" />
        </View>
      </Approach>
    </Approaches>
  );
}

const meta: Meta = { title: 'Proposals/Actions', parameters: { layout: 'fullscreen' } };
export default meta;

export const Patch: StoryObj = { render: () => <PatchStory /> };
export const ThreadReply: StoryObj = { name: 'Thread reply', render: () => <ThreadReplyStory /> };
export const QuoteReply: StoryObj = { name: 'Quote reply', render: () => <QuoteReplyStory /> };
export const Resolve: StoryObj = { name: 'Resolve thread', render: () => <ResolveStory /> };
export const ToolRequest: StoryObj = { name: 'Tool request', render: () => <ToolRequestStory /> };
export const FailedApproval: StoryObj = { name: 'Failed approval', render: () => <ErrorStory /> };

const styles = StyleSheet.create({
  approaches: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 24,
    padding: 20,
    backgroundColor: t3.background,
    minHeight: '100%',
  },
  approach: {
    width: 380,
    gap: 2,
  },
  approachName: {
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '600',
  },
  approachNote: {
    color: t3.muted,
    fontSize: 12,
    marginBottom: 8,
  },
  frame: {
    padding: 10,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  meta: {
    color: t3.muted,
    fontSize: 11,
    lineHeight: 16,
  },
  mono: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  card: {
    borderRadius: 10,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
    paddingBottom: 6,
  },
  cardHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 12,
    paddingVertical: 8,
  },
  cardTitle: {
    color: t3.foreground,
    fontSize: 12,
    fontWeight: '500',
  },
  ghostButton: {
    flexDirection: 'row',
    alignItems: 'center',
    height: 24,
    paddingRight: 6,
    borderRadius: 6,
  },
  ghostButtonText: {
    color: t3.muted,
    fontSize: 12,
  },
  treeRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    height: 28,
    paddingLeft: 8,
    paddingRight: 12,
  },
  treeText: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
  },
  commit: {
    color: t3.muted,
    fontSize: 11,
    paddingHorizontal: 12,
    paddingTop: 4,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    alignSelf: 'flex-start',
    maxWidth: '100%',
    gap: 2,
    height: 24,
    paddingRight: 8,
    marginBottom: 8,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: t3.input,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  chipText: {
    flexShrink: 1,
    color: t3.muted,
    fontSize: 12,
  },
  outcome: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    marginTop: 4,
  },
  outcomeText: {
    color: t3.muted,
    fontSize: 11,
  },
  codeChip: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
    borderRadius: 4,
  },
  commandBlock: {
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.015)',
  },
  buttonsRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 6,
  },
});
