import { Check, CircleCheck, CircleDashed, CircleX, MessageCircleQuestionMark } from 'lucide-react';
import { ActivityIndicator, Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import type { PullRequest } from './gen/gem_pb';
import { Button, Icon, t3, usePhone } from './t3';

// A cluster's PRs as a bordered list like T3 Code's changed-files panel: one row per PR with its
// checks, repo#number, title, review state and how many proposals wait on it. A row opens the PR.
// On a phone the review state is a check mark when approved, leaving the title room. A PR behind
// its base branch can be rebased by an agent, which spins until its thread settles; a question
// from it opens the thread, as does one from an agent merging the cluster. Under the row, a line says what is happening to the PR and what it
// waits on.
export function PullRequestList({
  pullRequests,
  busy,
  onRebase,
}: {
  pullRequests: PullRequest[];
  busy?: string;
  onRebase?: (url: string) => void;
}) {
  const phone = usePhone();
  // Wide enough for the longest repo#number, so they line up and never wrap.
  const refWidth = Math.max(...pullRequests.map((pr) => `${pr.repository?.name}#${pr.number}`.length)) * 7.5;
  return (
    <View style={styles.list}>
      {pullRequests.map((pr, i) => (
        <Pressable
          key={pr.url}
          style={[styles.item, i > 0 && styles.rowBorder]}
          onPress={() => Linking.openURL(pr.url)}
        >
          <View style={styles.row}>
            <ChecksIcon pr={pr} />
            <Text numberOfLines={1} style={[styles.ref, { width: refWidth }]}>
              {pr.repository?.name}#{pr.number}
            </Text>
            <Text numberOfLines={1} style={styles.title}>
              {pr.title}
            </Text>
            {phone ? (
              pr.approved && <Icon icon={Check} size={14} color={t3.addition} />
            ) : (
              <Text style={[styles.review, pr.approved && { color: t3.addition }]}>
                {pr.approved ? 'Approved' : 'Review required'}
              </Text>
            )}
            {pr.merge?.question && (
              <Pressable
                accessibilityLabel="Open the merge's question"
                style={styles.question}
                onPress={() => Linking.openURL(pr.merge!.threadUrl)}
              >
                <Icon icon={MessageCircleQuestionMark} size={14} color={t3.warning} />
                {!phone && <Text style={styles.questionText}>Question</Text>}
              </Pressable>
            )}
            {pr.rebase?.question && (
              <Pressable
                accessibilityLabel="Open the rebase's question"
                style={styles.question}
                onPress={() => Linking.openURL(pr.rebase!.threadUrl)}
              >
                <Icon icon={MessageCircleQuestionMark} size={14} color={t3.warning} />
                {!phone && <Text style={styles.questionText}>Question</Text>}
              </Pressable>
            )}
            {(pr.rebase || pr.merge || pr.agents.some((agent) => !agent.queued)) && <ActivityIndicator size={12} color="#8FA8F5" />}
            {onRebase && (pr.behind || pr.rebase) && (
              <Button
                label="Rebase"
                disabled={pr.rebase !== undefined || busy === pr.url}
                onPress={() => onRebase(pr.url)}
              />
            )}
            <View style={styles.countSlot}>
              {pr.proposals.length > 0 && <Text style={styles.count}>{pr.proposals.length}</Text>}
            </View>
          </View>
          <StatusLine pr={pr} />
        </Pressable>
      ))}
    </View>
  );
}

// What is happening to the PR, what it waits on, and a link to the agent doing it.
type Status = { doing: string; waiting?: string; link?: string; ready?: boolean };

function status(pr: PullRequest): Status | undefined {
  const checks = pr.checks ?? { total: 0, done: 0, failed: 0 };
  const running = checks.total - checks.done;
  const noun = checks.failed === 1 ? 'check' : 'checks';
  const checksFailed = `${checks.failed} ${noun} failed`;
  if (pr.merge?.question) return { doing: 'Merging', waiting: 'waiting on you to answer its agent', link: pr.merge.threadUrl };
  if (pr.merge) return { doing: 'Merging', waiting: 'waiting on its agent', link: pr.merge.threadUrl };
  if (pr.rebase?.pushed) return { doing: 'Rebased', waiting: 'waiting on GitHub to list the new commit' };
  if (pr.rebase?.question) return { doing: 'Rebasing', waiting: 'waiting on you to answer its agent', link: pr.rebase.threadUrl };
  if (pr.rebase) return { doing: 'Rebasing', waiting: 'waiting on its agent', link: pr.rebase.threadUrl };
  if (pr.agents.length > 0) {
    const comments = pr.agents.filter((agent) => !agent.checks).length;
    const subject = [
      comments > 0 && `${comments} ${comments === 1 ? 'comment' : 'comments'}`,
      pr.agents.some((agent) => agent.checks) && `the ${checks.failed} failed ${noun}`,
    ]
      .filter(Boolean)
      .join(' and ');
    const working = pr.agents.filter((agent) => !agent.queued);
    const queued = pr.agents.length - working.length;
    if (working.length === 0) return { doing: `Queued to answer ${subject}`, waiting: 'waiting on other agents to finish' };
    return {
      doing: `Answering ${subject}`,
      waiting: `${queued > 0 ? `${queued} of ${pr.agents.length} agents queued · ` : ''}waiting on proposals`,
      link: working.find((agent) => agent.threadUrl)?.threadUrl,
    };
  }
  if (running > 0 && checks.failed > 0) {
    return { doing: checksFailed, waiting: `waiting on ${running} still running before an agent looks at them` };
  }
  if (running > 0) return { doing: `${checks.done} of ${checks.total} checks done`, waiting: `waiting on ${running} running` };
  if (pr.proposals.length > 0) {
    return { doing: `${pr.proposals.length} ${pr.proposals.length === 1 ? 'proposal' : 'proposals'}`, waiting: 'waiting on you' };
  }
  if (checks.failed > 0) return { doing: checksFailed, waiting: 'no proposal for them' };
  if (pr.behind) return { doing: 'Behind its base branch', waiting: 'waiting on you to rebase' };
  if (pr.approved && pr.checksPassing) return { doing: 'Ready to merge', waiting: 'waiting on you', ready: true };
  return undefined;
}

function StatusLine({ pr }: { pr: PullRequest }) {
  const line = status(pr);
  if (!line) return null;
  return (
    <View style={styles.status}>
      <Text numberOfLines={1} style={styles.statusText}>
        <Text style={[styles.doing, line.ready && { color: t3.addition }]}>{line.doing}</Text>
        {line.waiting && ` · ${line.waiting}`}
      </Text>
      {line.link && (
        <Pressable onPress={() => Linking.openURL(line.link!)}>
          <Text style={styles.link}>Thread ↗</Text>
        </Pressable>
      )}
    </View>
  );
}

// Checks on the head commit: failed, passed, or still running. A pushed rebase's new commit has
// not had its checks listed yet.
export function ChecksIcon({ pr, size = 14 }: { pr: PullRequest; size?: number }) {
  if (pr.rebase?.pushed) return <Icon icon={CircleDashed} size={size} color={t3.warning} />;
  if (pr.checksFailing) return <Icon icon={CircleX} size={size} color={t3.deletion} />;
  if (pr.checksPassing) return <Icon icon={CircleCheck} size={size} color={t3.addition} />;
  return <Icon icon={CircleDashed} size={size} color={t3.warning} />;
}

const styles = StyleSheet.create({
  list: {
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  item: {
    paddingHorizontal: 6,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 36,
  },
  status: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginTop: -6,
    paddingBottom: 8,
    paddingLeft: 22,
  },
  statusText: {
    flexShrink: 1,
    color: t3.faint,
    fontSize: 12,
  },
  doing: {
    color: t3.muted,
  },
  link: {
    color: '#8FA8F5',
    fontSize: 12,
  },
  rowBorder: {
    borderTopWidth: 1,
    borderTopColor: t3.border,
  },
  ref: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  title: {
    flex: 1,
    color: t3.foreground,
    fontSize: 13,
  },
  review: {
    color: t3.muted,
    fontSize: 12,
  },
  question: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    height: 24,
    paddingHorizontal: 6,
    borderRadius: 8,
    backgroundColor: 'rgba(245, 158, 11, 0.12)',
  },
  questionText: {
    color: t3.warning,
    fontSize: 12,
  },
  countSlot: {
    width: 24,
    alignItems: 'flex-end',
  },
  count: {
    minWidth: 18,
    paddingHorizontal: 5,
    borderRadius: 6,
    color: '#8FA8F5',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
    fontSize: 11,
    lineHeight: 18,
    textAlign: 'center',
    fontVariant: ['tabular-nums'],
  },
});
