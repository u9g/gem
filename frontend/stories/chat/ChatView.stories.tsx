import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { CircleAlert, ExternalLink, FileCode, MessageCircle, MessageSquareQuote } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal } from '../../gen/gem_pb';
import {
  DiffSurface,
  EditableReply,
  type ReplyDraft,
  ResolvesTag,
  Theirs,
  ToolRequestBanner,
} from '../../surfaces';
import { Button, DiffStat, diffFiles, Icon, proposalIcon, stepSentence, t3, visibleMarkdown } from '../../t3';
import { pr, proposals } from '../fixtures';

// Proposals answering the same comment are alternatives: approving one clears the rest.
function byComment(list: Proposal[]): Proposal[][] {
  const groups = new Map<string, Proposal[]>();
  for (const proposal of list) {
    const id = proposal.comment?.id ?? proposal.id;
    groups.set(id, [...(groups.get(id) ?? []), proposal]);
  }
  return [...groups.values()];
}

function quoted(proposal: Proposal): string {
  return visibleMarkdown(proposal.comment!.body).split('\n\n')[0].replace(/[#*]/g, '').trim();
}

function totals(proposal: Proposal) {
  const files = proposal.patch ? diffFiles(proposal.patch.diff) : [];
  return {
    files: files.length,
    added: files.reduce((n, f) => n + f.added, 0),
    removed: files.reduce((n, f) => n + f.removed, 0),
  };
}

// Every message in the conversation, oldest first: the thread's earlier comments, then the one
// waiting on u9g, with a link to it on GitHub.
function Thread({ proposal }: { proposal: Proposal }) {
  const comment = proposal.comment!;
  return (
    <View style={{ gap: 10 }}>
      {proposal.earlierComments.map((earlier) => (
        <Theirs key={earlier.id} author={earlier.author} body={earlier.body} />
      ))}
      <Theirs
        author={comment.author}
        body={comment.body}
        trailing={
          <Pressable accessibilityLabel="Open on GitHub">
            <Icon icon={ExternalLink} size={12} />
          </Pressable>
        }
      />
    </View>
  );
}

// u9g's reply as it will be posted, editable, with the resolves tag under it.
function Reply({ proposal }: { proposal: Proposal }) {
  const [draft, setDraft] = useState<ReplyDraft>({ yours: proposal.reply, showing: 'suggested' });
  if (!proposal.reply) return null;
  return (
    <View style={{ gap: 6 }}>
      <EditableReply
        suggested={proposal.reply}
        draft={draft}
        onChange={setDraft}
        quote={proposal.threadId ? undefined : quoted(proposal)}
      />
      {proposal.resolveThread && (
        <View style={styles.rightRow}>
          <ResolvesTag />
        </View>
      )}
    </View>
  );
}

function Patch({ proposal }: { proposal: Proposal }) {
  if (!proposal.patch) return null;
  const failed = proposal.error !== '';
  return (
    <View style={{ gap: 6 }}>
      <Text style={styles.meta}>{proposal.patch.commitMessage}</Text>
      <DiffSurface
        diff={proposal.patch.diff}
        header={failed ? <Icon icon={CircleAlert} size={12} color={t3.deletion} /> : undefined}
      />
      {failed && <Text style={styles.error}>{proposal.error}</Text>}
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

function Header({ proposal }: { proposal: Proposal }) {
  const where = proposal.threadId ? 'review thread' : 'conversation';
  return (
    <Text style={styles.where}>
      {pr.repository?.name}#{pr.number} · {where}
    </Text>
  );
}

// ---------- A: chat ----------

// Each conversation reads like a T3 Code chat: their messages, then each proposed answer as the
// agent's turn (a one-line summary, the diff, u9g's reply bubble), closed by T3 Code's approval
// banner saying what approving does.
function ChatTurn({ proposal, option }: { proposal: Proposal; option?: string }) {
  const t = totals(proposal);
  return (
    <View style={styles.turn}>
      <View style={styles.turnSummary}>
        <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
        <Text style={styles.turnText}>
          {option && <Text style={styles.option}>{option} </Text>}
          {proposal.summary}
        </Text>
      </View>
      {proposal.requestedTool !== '' && <ToolRequestBanner proposal={proposal} />}
      <Patch proposal={proposal} />
      <Reply proposal={proposal} />
      <View style={[styles.approvalBar, proposal.error !== '' && styles.approvalBarError]}>
        <Text style={styles.approvalText} numberOfLines={1}>
          <Text style={{ color: proposal.error ? t3.deletion : t3.foreground, fontWeight: '500' }}>
            {proposal.error ? 'Approval failed' : stepSentence(proposal)}
          </Text>
          {t.files > 0 && (
            <>
              {'  '}
              <DiffStat added={t.added} removed={t.removed} />
            </>
          )}
        </Text>
        <Actions proposal={proposal} />
      </View>
    </View>
  );
}

function Chat() {
  return (
    <Column>
      {byComment(proposals).map((group) => (
        <View key={group[0].id} style={styles.conversation}>
          <Header proposal={group[0]} />
          <Thread proposal={group[0]} />
          {group.map((proposal, i) => (
            <ChatTurn
              key={proposal.id}
              proposal={proposal}
              option={group.length > 1 ? `Option ${i + 1} of ${group.length} ·` : undefined}
            />
          ))}
        </View>
      ))}
    </Column>
  );
}

// ---------- B: side by side ----------

// Like a code review: the conversation and u9g's reply on the left, the change on the right, and
// the actions under the reply. Alternatives stack, split by "or".
function SideBySide() {
  return (
    <View style={[styles.page, { alignItems: 'stretch' }]}>
      <View style={{ width: '100%', maxWidth: 1240, alignSelf: 'center', gap: 28 }}>
        <Title />
        {byComment(proposals).map((group) => (
          <View key={group[0].id} style={styles.conversation}>
            <Header proposal={group[0]} />
            {group.map((proposal, i) => (
              <View key={proposal.id} style={{ gap: 16 }}>
                {i > 0 && (
                  <View style={styles.orRow}>
                    <View style={styles.orLine} />
                    <Text style={styles.orText}>or</Text>
                    <View style={styles.orLine} />
                  </View>
                )}
                <View style={styles.split}>
                  <View style={styles.splitLeft}>
                    {i === 0 && <Thread proposal={proposal} />}
                    <Text style={styles.turnText}>{proposal.summary}</Text>
                    <Reply proposal={proposal} />
                    <Actions proposal={proposal} />
                  </View>
                  <View style={styles.splitRight}>
                    {proposal.patch ? (
                      <Patch proposal={proposal} />
                    ) : proposal.requestedTool ? (
                      <ToolRequestBanner proposal={proposal} />
                    ) : (
                      <View style={styles.noChange}>
                        <Icon icon={proposal.threadId ? MessageCircle : MessageSquareQuote} />
                        <Text style={styles.meta}>No code change: approving only replies</Text>
                      </View>
                    )}
                  </View>
                </View>
              </View>
            ))}
          </View>
        ))}
      </View>
    </View>
  );
}

// ---------- C: open cards ----------

// One card per comment with everything showing. When the agent offered alternatives, tabs at the
// top of the card switch between them; the thread above stays put.
function OpenCard({ group }: { group: Proposal[] }) {
  const [picked, setPicked] = useState(0);
  const proposal = group[picked];
  const t = totals(proposal);
  return (
    <View style={[styles.card, proposal.error !== '' && styles.cardError]}>
      <Header proposal={proposal} />
      <Thread proposal={proposal} />
      {group.length > 1 && (
        <View style={styles.tabs}>
          {group.map((option, i) => (
            <Pressable key={option.id} onPress={() => setPicked(i)} style={[styles.tab, i === picked && styles.tabOn]}>
              <Icon icon={option.patch ? FileCode : MessageCircle} size={12} />
              <Text style={[styles.tabText, i === picked && { color: t3.foreground }]}>{stepSentence(option)}</Text>
            </Pressable>
          ))}
        </View>
      )}
      <View style={styles.turnSummary}>
        <Icon icon={proposalIcon(proposal)} color={proposal.error ? t3.deletion : t3.muted} />
        <Text style={styles.turnText}>{proposal.summary}</Text>
      </View>
      {proposal.requestedTool !== '' && <ToolRequestBanner proposal={proposal} />}
      <Patch proposal={proposal} />
      <Reply proposal={proposal} />
      <View style={styles.cardFooter}>
        <Text style={[styles.meta, { flex: 1 }]}>
          {proposal.error ? 'Approval failed' : stepSentence(proposal)}
          {t.files > 0 && (
            <>
              {'  '}
              <DiffStat added={t.added} removed={t.removed} />
            </>
          )}
        </Text>
        <Actions proposal={proposal} />
      </View>
    </View>
  );
}

function OpenCards() {
  return (
    <Column>
      {byComment(proposals).map((group) => (
        <OpenCard key={group[0].id} group={group} />
      ))}
    </Column>
  );
}

// ---------- Layout ----------

function Title() {
  return (
    <View style={{ gap: 4 }}>
      <Text style={styles.title}>TTS retry fixes</Text>
      <Text style={styles.meta}>
        2 hours ago · <Text style={{ color: t3.deletion }}>agents</Text>
      </Text>
    </View>
  );
}

function Column({ children }: { children: ReactNode }) {
  return (
    <View style={styles.page}>
      <View style={styles.column}>
        <Title />
        {children}
      </View>
    </View>
  );
}

const meta: Meta = { title: 'Chat view', parameters: { layout: 'fullscreen' } };
export default meta;

export const AChat: StoryObj = { name: 'A · Chat', render: () => <Chat /> };
export const BSideBySide: StoryObj = { name: 'B · Side by side', render: () => <SideBySide /> };
export const COpenCards: StoryObj = { name: 'C · Open cards', render: () => <OpenCards /> };

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
    maxWidth: 736,
    gap: 28,
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
  error: {
    color: t3.deletion,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
  },
  where: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  conversation: {
    gap: 14,
    paddingBottom: 28,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  rightRow: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
  },
  buttons: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 6,
  },
  turn: {
    gap: 10,
  },
  turnSummary: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 6,
  },
  turnText: {
    flex: 1,
    color: t3.foreground,
    fontSize: 14,
    lineHeight: 22,
  },
  option: {
    color: t3.muted,
  },
  approvalBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingLeft: 12,
    paddingRight: 6,
    paddingVertical: 6,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: t3.input,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  approvalBarError: {
    borderColor: 'rgba(242, 109, 109, 0.28)',
    backgroundColor: 'rgba(242, 109, 109, 0.06)',
  },
  approvalText: {
    flex: 1,
    color: t3.muted,
    fontSize: 12,
  },
  orRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  orLine: {
    flex: 1,
    height: 1,
    backgroundColor: t3.border,
  },
  orText: {
    color: t3.faint,
    fontSize: 11,
  },
  split: {
    flexDirection: 'row',
    gap: 24,
    alignItems: 'flex-start',
  },
  splitLeft: {
    flex: 4,
    gap: 12,
  },
  splitRight: {
    flex: 5,
    gap: 8,
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
  card: {
    gap: 14,
    padding: 16,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  cardError: {
    borderColor: 'rgba(242, 109, 109, 0.28)',
  },
  tabs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 4,
    padding: 3,
    alignSelf: 'flex-start',
    borderRadius: 10,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  tab: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingRight: 10,
    height: 26,
    borderRadius: 8,
  },
  tabOn: {
    backgroundColor: 'rgba(255, 255, 255, 0.08)',
  },
  tabText: {
    color: t3.muted,
    fontSize: 12,
  },
  cardFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingTop: 12,
    borderTopWidth: 1,
    borderTopColor: t3.border,
  },
});
