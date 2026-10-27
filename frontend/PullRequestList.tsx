import { Check, CircleCheck, CircleDashed, CircleX } from 'lucide-react';
import { ActivityIndicator, Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { type Agent, AgentState, MergeBy, type PullRequest } from './gen/gem_pb';
import { Button, Icon, t3, usePhone } from './t3';

// A cluster's PRs as a bordered list like T3 Code's changed-files panel: one row per PR with its
// checks, repo#number, title, review state and how many proposals wait on it. A row opens the PR.
// On a phone the review state is a check mark when approved, leaving the title room. A PR GitHub
// will not merge until it is rebased onto its base branch can be rebased by an agent. A PR spins
// while an agent works on it. Under the row, a line says what is happening to the PR and what it
// waits on.
export function PullRequestList({
  pullRequests,
  agents,
  busy,
  onRebase,
}: {
  pullRequests: PullRequest[];
  // The cluster's active agents.
  agents: Agent[];
  busy?: ReadonlySet<string>;
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
            {on(pr, agents).some((agent) => agent.state === AgentState.WORKING) && <ActivityIndicator size={12} color="#8FA8F5" />}
            {onRebase && (pr.behind || pr.rebasing || pr.rebased) && (
              <Button
                label="Rebase"
                disabled={pr.rebasing || pr.rebased || busy?.has(pr.url)}
                onPress={() => onRebase(pr.url)}
              />
            )}
            <View style={styles.countSlot}>
              {pr.proposals.length > 0 && <Text style={styles.count}>{pr.proposals.length}</Text>}
            </View>
          </View>
          <StatusLine pr={pr} agents={agents} />
        </Pressable>
      ))}
    </View>
  );
}

// The agents acting on the PR.
function on(pr: PullRequest, agents: Agent[]): Agent[] {
  return agents.filter((agent) => agent.pullRequestUrls.includes(pr.url));
}

// What is happening to the PR and what it waits on.
type Status = { doing: string; waiting?: string; ready?: boolean; forceable?: boolean };

function status(pr: PullRequest, agents: Agent[]): Status | undefined {
  const checks = pr.checks ?? { total: 0, done: 0, failed: 0 };
  const running = checks.total - checks.done;
  const checksFailed = `${checks.failed} ${checks.failed === 1 ? 'check' : 'checks'} failed`;
  const acting = on(pr, agents);
  const working = acting.filter((agent) => agent.state === AgentState.WORKING);
  if (pr.rebased) return { doing: 'Rebased', waiting: 'waiting on GitHub to list the new commit' };
  if (working.length === 1 && acting.length === 1) return { doing: working[0].title, waiting: 'waiting on its agent' };
  if (working.length > 0) {
    const queued = acting.length - working.length;
    return {
      doing: `${working.length} ${working.length === 1 ? 'agent' : 'agents'} working`,
      waiting: `${queued > 0 ? `${queued} queued · ` : ''}waiting on them`,
    };
  }
  if (acting.length > 0) return { doing: `${acting.length} ${acting.length === 1 ? 'agent' : 'agents'} queued`, waiting: 'waiting on other agents to finish' };
  // Its agent stopped without finishing: it waits in the agent list.
  if (pr.merging) return { doing: 'Merging', waiting: 'waiting on you to answer its agent' };
  if (pr.rebasing) return { doing: 'Rebasing', waiting: 'waiting on you to answer its agent' };
  if (running > 0 && checks.failed > 0) {
    return { doing: checksFailed, waiting: `waiting on ${running} still running before an agent looks at them` };
  }
  if (running > 0) return { doing: `${checks.done} of ${checks.total} checks done`, waiting: `waiting on ${running} running` };
  if (pr.proposals.length > 0) {
    return { doing: `${pr.proposals.length} ${pr.proposals.length === 1 ? 'proposal' : 'proposals'}`, waiting: 'waiting on you' };
  }
  if (checks.failed > 0) return { doing: checksFailed, waiting: 'no proposal for them' };
  if (pr.conflicts) return { doing: `Conflicts with ${pr.base}`, waiting: 'waiting on you to rebase' };
  if (pr.behind) return { doing: `${pr.base} requires it up to date`, waiting: 'waiting on you to rebase' };
  if (pr.mergeBy === MergeBy.APPROVAL) return { doing: 'Ready to merge', waiting: 'waiting on you', ready: true };
  if (pr.mergeBy === MergeBy.ADMIN) return { doing: 'Not approved', waiting: 'you can force it in as an admin', forceable: true };
  return undefined;
}

export function StatusLine({ pr, agents }: { pr: PullRequest; agents: Agent[] }) {
  const line = status(pr, agents);
  if (!line) return null;
  return (
    <View style={styles.status}>
      <Text numberOfLines={1} style={styles.statusText}>
        <Text style={[styles.doing, line.ready && { color: t3.addition }, line.forceable && { color: t3.warning }]}>{line.doing}</Text>
        {line.waiting && ` · ${line.waiting}`}
      </Text>
    </View>
  );
}

// Checks on the head commit: failed, passed, or still running. A pushed rebase's new commit has
// not had its checks listed yet.
export function ChecksIcon({ pr, size = 14 }: { pr: PullRequest; size?: number }) {
  if (pr.rebased) return <Icon icon={CircleDashed} size={size} color={t3.warning} />;
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
