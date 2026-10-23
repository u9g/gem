import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { Check, GitPullRequest, X } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { ProposalView } from '../../ProposalView';
import { type Item, items, pr } from '../fixtures';
import {
  Button,
  Code,
  CommentRow,
  diffFiles,
  ErrorLine,
  firstLine,
  Icon,
  proposalIcon,
  Row,
  Sidebar,
  Steps,
  stepSentence,
  t3,
} from '../../t3';

// Proposals answering the same comment are alternatives: approving one clears the rest.
function byComment(list: Item[]): Item[][] {
  const groups = new Map<string, Item[]>();
  for (const item of list) {
    const id = item.proposal.comment?.id ?? item.proposal.id;
    groups.set(id, [...(groups.get(id) ?? []), item]);
  }
  return [...groups.values()];
}

// Approve and Dismiss stay hidden until the row is hovered, like T3 Code's timeline actions.
function HoverActions({ children, actions, shown = false }: { children: ReactNode; actions: ReactNode; shown?: boolean }) {
  const [hovered, setHovered] = useState(shown);
  return (
    <View onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)}>
      {children}
      <View style={[styles.hoverActions, { opacity: hovered ? 1 : 0 }]}>{actions}</View>
    </View>
  );
}

function IconButton({ icon, color, label }: { icon: typeof Check; color?: string; label: string }) {
  return (
    <Pressable accessibilityLabel={label} style={styles.iconButton}>
      <Icon icon={icon} size={14} color={color ?? t3.muted} />
    </Pressable>
  );
}

// 1. Work log: the comment, then one sentence per proposal; everything else is a click away.
function WorkLog({ expanded = false }: { expanded?: boolean }) {
  return (
    <Sidebar title="TTS retry fixes">
      <View style={styles.list}>
        {byComment(items).map((group) => (
          <View key={group[0].proposal.id} style={styles.section}>
            <CommentRow item={group[0]} />
            {group.map(({ proposal }) => (
              <HoverActions
                key={proposal.id}
                shown={expanded && proposal === items[0].proposal}
                actions={
                  <>
                    {!proposal.requestedTool && <IconButton icon={Check} color={t3.addition} label="Approve" />}
                    <IconButton icon={X} label="Dismiss" />
                  </>
                }
              >
                <Row
                  indent={12}
                  initiallyOpen={expanded && proposal === items[0].proposal}
                  icon={proposalIcon(proposal)}
                  label={proposal.summary}
                  labelColor={proposal.error ? t3.deletion : t3.muted}
                >
                  <Text style={styles.meta}>{stepSentence(proposal)}</Text>
                  <Steps proposal={proposal} />
                  <ErrorLine proposal={proposal} />
                </Row>
              </HoverActions>
            ))}
          </View>
        ))}
      </View>
    </Sidebar>
  );
}

// 2. Choose one: each comment is a question and its proposals are the answers, like T3 Code's
// pending user-input panel. Number keys pick; one Approve runs the picked answer.
function ChooseOne() {
  return (
    <Sidebar title="TTS retry fixes">
      <View style={styles.list}>
        {byComment(items).map((group) => (
          <Question key={group[0].proposal.id} group={group} />
        ))}
      </View>
    </Sidebar>
  );
}

function Question({ group }: { group: Item[] }) {
  const [picked, setPicked] = useState(0);
  const comment = group[0].proposal.comment!;
  const proposal = group[picked].proposal;
  return (
    <View style={styles.question}>
      <Text style={styles.eyebrow}>
        {comment.author} · {pr.repository?.name}#{pr.number}
      </Text>
      <Text numberOfLines={2} style={styles.questionText}>
        {firstLine(comment.body)}
      </Text>
      {group.map(({ proposal: option }, i) => (
        <Pressable
          key={option.id}
          onPress={() => setPicked(i)}
          style={[styles.option, i === picked && group.length > 1 && styles.optionPicked]}
        >
          <View style={{ flex: 1 }}>
            <Text numberOfLines={1} style={styles.optionLabel}>
              {option.summary}
            </Text>
            <Text style={[styles.meta, option.error !== '' && { color: t3.deletion }]}>
              {option.error ? 'Approval failed' : stepSentence(option)}
            </Text>
          </View>
          {group.length > 1 && <Text style={styles.kbd}>{i + 1}</Text>}
        </Pressable>
      ))}
      <Row label="Details" indent={0}>
        <Steps proposal={proposal} />
        <ErrorLine proposal={proposal} />
      </Row>
      <View style={styles.buttons}>
        <Button label="Dismiss" variant="ghost" />
        {!proposal.requestedTool && <Button label="Approve" variant="default" />}
      </View>
    </View>
  );
}

// 3. Banners: one approval banner per proposal, like T3 Code's command-approval banner above the
// composer. The app's sidebar uses this design.
function Banners() {
  return (
    <Sidebar title="TTS retry fixes">
      <View style={[styles.list, { gap: 8 }]}>
        {items.map(({ pr, proposal }) => (
          <ProposalView
            key={proposal.id}
            pr={pr}
            proposal={proposal}
            busy={false}
            disabled={false}
            onApprove={() => {}}
            onDismiss={() => {}}
          />
        ))}
      </View>
    </Sidebar>
  );
}

// 4. One fold: the sidebar says only what is waiting, as one T3 Code group sentence. Opening it
// shows the work log.
function OneFold({ expanded = false }: { expanded?: boolean }) {
  const files = items.reduce((n, { proposal }) => n + (proposal.patch ? diffFiles(proposal.patch.diff).length : 0), 0);
  const replies = items.filter(({ proposal }) => proposal.reply).length;
  const comments = byComment(items).length;
  return (
    <Sidebar title="TTS retry fixes">
      <View style={styles.list}>
        <Row
          icon={GitPullRequest}
          label={`${comments} waiting · ${files} files, ${replies} replies`}
          labelColor={t3.foreground}
          initiallyOpen={expanded}
        >
          {byComment(items).map((group) => (
            <View key={group[0].proposal.id}>
              <CommentRow item={group[0]} />
              {group.map(({ proposal }) => (
                <Row key={proposal.id} indent={12} icon={proposalIcon(proposal)} label={proposal.summary}>
                  <Steps proposal={proposal} />
                  <ErrorLine proposal={proposal} />
                  <View style={styles.buttons}>
                    <Button label="Dismiss" variant="ghost" />
                    {!proposal.requestedTool && <Button label="Approve" variant="default" />}
                  </View>
                </Row>
              ))}
            </View>
          ))}
        </Row>
      </View>
    </Sidebar>
  );
}

// 5. Under the PR: proposals live in the sidebar's PR list as one-line children of the PR they
// answer on, with a count on the PR row. Nothing sits above the PRs.
function UnderThePr({ expanded = false }: { expanded?: boolean }) {
  return (
    <Sidebar title="TTS retry fixes">
      <View style={styles.list}>
        <Row
          icon={GitPullRequest}
          label={
            <>
              <Text style={{ color: t3.foreground }}>{pr.title}</Text> {pr.repository?.name}#{pr.number}
            </>
          }
          trailing={<Text style={styles.count}>{items.length}</Text>}
          initiallyOpen
        >
          {items.map(({ proposal }) => (
            <HoverActions
              key={proposal.id}
              shown={expanded && proposal === items[1].proposal}
              actions={
                <>
                  {!proposal.requestedTool && <IconButton icon={Check} color={t3.addition} label="Approve" />}
                  <IconButton icon={X} label="Dismiss" />
                </>
              }
            >
              <Row
                initiallyOpen={expanded && proposal === items[1].proposal}
                icon={proposalIcon(proposal)}
                label={
                  <>
                    <Text style={{ color: t3.foreground }}>{proposal.comment?.author}</Text> {proposal.summary}
                  </>
                }
                labelColor={proposal.error ? t3.deletion : t3.muted}
              >
                <CommentRow item={{ pr, proposal }} />
                <Steps proposal={proposal} />
                <ErrorLine proposal={proposal} />
              </Row>
            </HoverActions>
          ))}
        </Row>
        <Row
          icon={GitPullRequest}
          label={
            <>
              <Text style={{ color: t3.foreground }}>feat(stt): stream partials to the agent</Text> agents#4830
            </>
          }
        />
      </View>
    </Sidebar>
  );
}

const meta: Meta = { title: 'Proposals/Designs' };
export default meta;

export const WorkLogStory: StoryObj = { name: '1 Work log', render: () => <WorkLog /> };
export const WorkLogOpenStory: StoryObj = { name: '1 Work log, one open', render: () => <WorkLog expanded /> };
export const ChooseOneStory: StoryObj = { name: '2 Choose one', render: () => <ChooseOne /> };
export const BannersStory: StoryObj = { name: '3 Banners', render: () => <Banners /> };
export const OneFoldStory: StoryObj = { name: '4 One fold', render: () => <OneFold /> };
export const OneFoldOpenStory: StoryObj = { name: '4 One fold, open', render: () => <OneFold expanded /> };
export const UnderThePrStory: StoryObj = { name: '5 Under the PR', render: () => <UnderThePr /> };
export const UnderThePrOpenStory: StoryObj = {
  name: '5 Under the PR, one open',
  render: () => <UnderThePr expanded />,
};

const styles = StyleSheet.create({
  list: {
    padding: 8,
  },
  section: {
    paddingBottom: 8,
    marginBottom: 8,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  meta: {
    color: t3.muted,
    fontSize: 11,
  },
  hoverActions: {
    position: 'absolute',
    right: 22,
    top: 0,
    height: 24,
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: t3.background,
  },
  iconButton: {
    width: 24,
    height: 24,
    borderRadius: 6,
    alignItems: 'center',
    justifyContent: 'center',
  },
  question: {
    gap: 6,
    padding: 8,
    marginBottom: 8,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  eyebrow: {
    color: t3.muted,
    fontSize: 11,
  },
  questionText: {
    color: t3.foreground,
    fontSize: 14,
  },
  option: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 10,
    paddingVertical: 8,
    borderRadius: 8,
  },
  optionPicked: {
    backgroundColor: 'rgba(255, 255, 255, 0.055)',
  },
  optionLabel: {
    color: t3.foreground,
    fontSize: 14,
    fontWeight: '500',
  },
  kbd: {
    width: 20,
    height: 20,
    lineHeight: 20,
    textAlign: 'center',
    borderRadius: 4,
    borderWidth: 1,
    borderColor: t3.input,
    color: t3.muted,
    fontSize: 10,
  },
  buttons: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 6,
  },
  count: {
    color: t3.muted,
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
});
