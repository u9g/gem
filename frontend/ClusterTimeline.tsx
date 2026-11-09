import type { LucideIcon } from 'lucide-react';
import {
  Bot,
  Check,
  CircleCheck,
  CircleDashed,
  CircleX,
  GitBranch,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  MessageSquare,
  RefreshCw,
  TriangleAlert,
} from 'lucide-react';
import type { ReactNode } from 'react';
import { ActivityIndicator, Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { active } from './agents';
import { Composer } from './AgentView';
import { ClusterAgents, CloseClusterButton, type ClusterView, RebaseClusterButton, timeAgo } from './ClusterView';
import { bySubject, ProposalGroup } from './ProposalView';
import { ChecksIcon, StatusLine } from './PullRequestList';
import { Html } from './surfaces';
import { Button, Icon, t3, usePhone } from './t3';
import type { Agent, Cluster, Proposal, PullRequest } from './view-types';

type Props = Parameters<typeof ClusterView>[0] & {
  // Starts an agent on the message, which reads the cluster through gem's MCP server. Resolves
  // once the agent started.
  onMessage?: (message: string) => Promise<unknown>;
};

export const ref = (pr: PullRequest) => `${pr.repository?.name}#${pr.number}`;

// The open cluster as a GitHub pull request's Conversation tab: its title, PRs and actions on top;
// one timeline across its PRs, ending in what it waits on and a box to message an agent; and beside
// it the stack, each PR's checks, review and base, and the cluster's agents. A phone puts that
// sidebar above the timeline.
//
// Only PRs opening and agents starting carry a time, so those come first, oldest first; an agent's
// proposals follow it. What each PR is now, its checks, review threads and base, has no time and
// closes the timeline, PR by PR in stack order, as of the cluster's last update.
export function ClusterTimeline(props: Props) {
  const { cluster, busy, onRestart, onRebaseCluster, onCloseCluster, onOpenAgent, onMessage } = props;
  const phone = usePhone();
  const key = cluster.pullRequests[0].url;
  const restarting = busy?.has(key);
  const merging = busy?.has(`merge ${key}`);
  const updatedAt = cluster.updatedAt;
  const by = cluster.mergeBy;
  const count = cluster.pullRequests.length;

  const sidebar = (
    <View style={[styles.sidebar, phone && styles.sidebarPhone]}>
      <SideSection title={`Stack · ${count}`}>
        <Stack {...props} />
      </SideSection>
      {cluster.agents.length > 0 && (
        <SideSection title={`Agents · ${cluster.agents.length}`}>
          <ClusterAgents agents={cluster.agents} onOpenAgent={onOpenAgent} />
        </SideSection>
      )}
    </View>
  );

  return (
    <View style={styles.page}>
      <View style={[styles.header, phone && styles.headerPhone]}>
        <View style={styles.titleBlock}>
          <Text
            style={[
              styles.title,
              phone && styles.titlePhone,
              by === 'approval' && { color: '#4ADE80' },
              by === 'admin' && { color: t3.warning },
            ]}
          >
            {cluster.name || cluster.pullRequests[0].title}
          </Text>
          <View style={styles.refs}>
            <StateBadge cluster={cluster} />
            {cluster.pullRequests.map((pr) => (
              <Pressable key={pr.url} style={styles.refChip} onPress={() => Linking.openURL(pr.url)}>
                <ChecksIcon pr={pr} size={12} />
                <Text style={styles.refText}>{ref(pr)}</Text>
              </Pressable>
            ))}
            <Text style={styles.meta}>
              {count} {count === 1 ? 'pull request' : 'pull requests'}
              {updatedAt !== undefined && ` · updated ${timeAgo(new Date(updatedAt))}`}
            </Text>
          </View>
        </View>
        <View style={[styles.actions, phone && styles.actionsPhone]}>
          <RebaseClusterButton cluster={cluster} busy={busy} onRebaseCluster={onRebaseCluster} />
          <Button
            label={restarting ? 'Restarting…' : 'Restart investigation'}
            disabled={restarting || merging}
            onPress={() => onRestart(cluster)}
          />
          <CloseClusterButton cluster={cluster} busy={busy} onCloseCluster={onCloseCluster} />
          <MergeButton {...props} />
        </View>
      </View>

      <View style={[styles.body, phone && styles.bodyPhone]}>
        {phone && sidebar}
        <View style={styles.main}>
          <Timeline {...props} />
          <MergeBox cluster={cluster} />
          {onMessage && (
            <View style={styles.composerRow}>
              <Badge icon={MessageSquare} />
              <View style={styles.composer}>
                <Text style={styles.composerTitle}>Message an agent about this cluster</Text>
                <Composer placeholder="Ask an agent to change, explain or check something…" onSend={onMessage} />
              </View>
            </View>
          )}
        </View>
        {!phone && sidebar}
      </View>
    </View>
  );
}

// Open while any PR is open, merging once one is, as GitHub's state label.
function StateBadge({ cluster }: { cluster: Cluster }) {
  const merging = cluster.pullRequests.some((pr) => pr.merging);
  return (
    <View style={[styles.state, merging && styles.stateMerging]}>
      <GitPullRequestArrow size={12} color="#fff" strokeWidth={2.25} />
      <Text style={styles.stateText}>{merging ? 'Merging' : 'Open'}</Text>
    </View>
  );
}

// Merges the cluster once the server says it can, forcing the unapproved PRs in as an admin when
// that is how. MergeBox under the timeline says why it can.
function MergeButton({ cluster, busy, onMerge }: Props) {
  const key = cluster.pullRequests[0].url;
  const restarting = busy?.has(key);
  const merging = busy?.has(`merge ${key}`);
  const admin = cluster.mergeBy === 'admin';
  if (cluster.mergeBy === 'unspecified' || cluster.pullRequests.some((pr) => pr.merging)) return null;
  return (
    <Button
      label={merging ? 'Starting an agent…' : admin ? 'Force-merge' : 'Merge'}
      variant="default"
      disabled={restarting || merging}
      onPress={() => onMerge(cluster, admin)}
    />
  );
}

// ——— Timeline ———

type Dated = { at: number; key: string; node: ReactNode };

function Timeline(props: Props) {
  const { cluster } = props;
  const prs = cluster.pullRequests;
  // A proposal an agent made follows that agent; the rest close the timeline on their PR.
  const fromAgents = new Set(cluster.agents.flatMap((agent) => agent.proposals.map((p) => p.id)));

  const dated: Dated[] = [];
  const undated: ReactNode[] = [];
  for (const [i, pr] of prs.entries()) {
    const opened = <Opened key={`open ${pr.url}`} pr={pr} below={i > 0 ? prs[i - 1] : undefined} />;
    if (pr.createdAt !== undefined) dated.push({ at: pr.createdAt, key: pr.url, node: opened });
    else undated.push(opened);
  }
  for (const agent of cluster.agents) {
    const node = <AgentEvent key={agent.id} agent={agent} {...props} />;
    if (agent.startedAt !== undefined) dated.push({ at: agent.startedAt, key: agent.id, node });
    else undated.push(node);
  }
  dated.sort((a, b) => a.at - b.at || a.key.localeCompare(b.key));

  return (
    <Rail>
      {undated}
      {dated.map((d) => d.node)}
      <Divider updatedAt={cluster.updatedAt} />
      {prs.map((pr) => (
        <PullRequestNow key={pr.url} pr={pr} hidden={fromAgents} {...props} />
      ))}
    </Rail>
  );
}

// Events down a rail through their badges.
export function Rail({ children }: { children: ReactNode }) {
  return (
    <View style={styles.timeline}>
      <View style={styles.rail} />
      {children}
    </View>
  );
}

// One line of a timeline: a badge on the rail, then what happened. `children` hang under it,
// beside the rail. Activity's timelines are drawn with these too.
export function Event({
  icon,
  tone,
  children,
  trailing,
  body,
}: {
  icon: LucideIcon;
  tone?: string;
  children: ReactNode;
  trailing?: ReactNode;
  body?: ReactNode;
}) {
  return (
    <View style={styles.event}>
      <View style={styles.eventLine}>
        <Badge icon={icon} tone={tone} />
        <Text style={styles.eventText}>{children}</Text>
        {trailing}
      </View>
      {body && <View style={styles.eventBody}>{body}</View>}
    </View>
  );
}

export function Badge({ icon, tone }: { icon: LucideIcon; tone?: string }) {
  return (
    <View style={[styles.badge, tone !== undefined && { borderColor: tone }]}>
      <Icon icon={icon} size={14} color={tone ?? t3.muted} />
    </View>
  );
}

export const Strong = ({ children }: { children: ReactNode }) => <Text style={styles.strong}>{children}</Text>;
const Mono = ({ children }: { children: ReactNode }) => <Text style={styles.mono}>{children}</Text>;
export const When = ({ at }: { at?: number }) =>
  at === undefined ? null : <Text style={styles.when}> · {timeAgo(new Date(at))}</Text>;

function Opened({ pr, below }: { pr: PullRequest; below?: PullRequest }) {
  return (
    <Event icon={GitPullRequest} tone="#8FA8F5">
      <Strong>{ref(pr)}</Strong> opened <Text style={styles.prTitle}>{pr.title}</Text> into <Mono>{pr.base}</Mono>
      {below && pr.base !== 'main' && pr.base !== 'master' && (
        <Text style={styles.when}> · stacked on {ref(below)}</Text>
      )}
      <When at={pr.createdAt} />
    </Event>
  );
}

// An agent started on the cluster: what it is doing, its reply once it has one, and what it
// proposes, each to approve or dismiss in place.
function AgentEvent({ agent, busy, onApprove, onDismiss, onInvestigate, onOpenAgent }: { agent: Agent } & Props) {
  const working = agent.state === 'working';
  const groups = bySubject(agent.proposals);
  const verb = working ? 'is working' : agent.state === 'queued' ? 'is queued' : 'finished';
  return (
    <Event
      icon={Bot}
      tone={active(agent) ? '#8FA8F5' : t3.addition}
      trailing={
        <View style={styles.trailing}>
          {working && <ActivityIndicator size={12} color="#8FA8F5" />}
          <Button label="Open" variant="ghost" onPress={() => onOpenAgent(agent.id)} />
        </View>
      }
      body={
        (agent.reply !== '' || groups.length > 0) && (
          <>
            {agent.reply !== '' && (
              <View style={styles.card}>
                <View style={styles.cardHead}>
                  <Text style={styles.strong}>{agent.title}</Text>
                  <Text style={styles.when}>replied</Text>
                </View>
                <View style={styles.cardBody}>
                  <Html html={agent.replyHtml} />
                </View>
              </View>
            )}
            {groups.map((group) => (
              <ProposalGroup
                key={group[0].id}
                proposals={group}
                busy={busy}
                onApprove={onApprove}
                onDismiss={onDismiss}
                onInvestigate={onInvestigate}
              />
            ))}
          </>
        )
      }
    >
      Agent <Strong>{agent.title}</Strong> {verb}
      {groups.length > 0 && ` with ${groups.length} ${groups.length === 1 ? 'proposal' : 'proposals'}`}
      <When at={agent.startedAt} />
    </Event>
  );
}

function Divider({ updatedAt }: { updatedAt?: number }) {
  return (
    <View style={styles.divider}>
      <View style={styles.dividerLine} />
      <Text style={styles.dividerText}>Now{updatedAt !== undefined && ` · as of ${timeAgo(new Date(updatedAt))}`}</Text>
      <View style={styles.dividerLine} />
    </View>
  );
}

// What one PR is now: its checks, the conversations on it waiting on the user, its review, and
// whether its base moved on.
function PullRequestNow({
  pr,
  hidden,
  busy,
  onApprove,
  onDismiss,
  onInvestigate,
  onRebase,
}: { pr: PullRequest; hidden: ReadonlySet<string> } & Props) {
  const checks = pr.checks;
  const total = checks?.total ?? 0;
  const done = checks?.done ?? 0;
  const failed = checks?.failed ?? 0;
  const groups = bySubject(pr.proposals.filter((p) => !hidden.has(p.id)));
  const threads = groups.filter((g) => g[0].comment).length;
  const rebaseButton = (pr.behind || pr.rebasing || pr.rebased) && (
    <Button
      label={pr.rebasing ? 'Rebasing…' : pr.rebased ? 'Rebased' : 'Rebase'}
      disabled={pr.rebasing || pr.rebased || busy?.has(pr.url)}
      onPress={() => onRebase(pr.url)}
    />
  );
  return (
    <>
      {total > 0 &&
        (pr.rebased ? (
          <Event icon={CircleDashed} tone={t3.warning}>
            <Strong>{ref(pr)}</Strong> rebased · waiting on GitHub to list the new commit's checks
          </Event>
        ) : failed > 0 ? (
          <Event icon={CircleX} tone={t3.deletion}>
            <Strong>{ref(pr)}</Strong> <Text style={{ color: t3.deletion }}>{failed} of {total} checks failed</Text>
            {done < total && ` · ${total - done} still running`} on <Mono>{pr.headOid.slice(0, 7) || 'head'}</Mono>
          </Event>
        ) : done < total ? (
          <Event icon={CircleDashed} tone={t3.warning}>
            <Strong>{ref(pr)}</Strong> {done} of {total} checks done · {total - done} running
          </Event>
        ) : (
          <Event icon={CircleCheck} tone={t3.addition}>
            <Strong>{ref(pr)}</Strong> all {total} checks passed
          </Event>
        ))}
      {groups.length > 0 && (
        <Event
          icon={MessageSquare}
          tone={t3.warning}
          body={groups.map((group) => (
            <ProposalGroup
              key={group[0].id}
              proposals={group}
              busy={busy}
              onApprove={onApprove}
              onDismiss={onDismiss}
              onInvestigate={onInvestigate}
            />
          ))}
        >
          <Strong>{ref(pr)}</Strong>{' '}
          {threads > 0
            ? `${threads} ${threads === 1 ? 'conversation' : 'conversations'} with a proposed answer`
            : `${groups.length} ${groups.length === 1 ? 'proposal' : 'proposals'}`}
          {threads > 0 && groups.length > threads && ` · ${groups.length - threads} more ${groups.length - threads === 1 ? 'proposal' : 'proposals'}`}
          <Text style={styles.when}> · waiting on you</Text>
        </Event>
      )}
      {pr.approved && (
        <Event icon={Check} tone={t3.addition}>
          <Strong>{ref(pr)}</Strong> approved
        </Event>
      )}
      {pr.conflicts ? (
        <Event icon={TriangleAlert} tone={t3.deletion} trailing={rebaseButton}>
          <Strong>{ref(pr)}</Strong> conflicts with <Mono>{pr.base}</Mono>
        </Event>
      ) : pr.behind || pr.rebasing ? (
        <Event icon={GitBranch} tone={t3.warning} trailing={rebaseButton}>
          <Strong>{ref(pr)}</Strong> {pr.rebasing ? 'is being rebased onto' : 'is behind'} <Mono>{pr.base}</Mono>
          {!pr.rebasing && <Text style={styles.when}> · {pr.base} requires it up to date</Text>}
        </Event>
      ) : null}
      {pr.merging && (
        <Event icon={GitMerge} tone="#A371F7">
          <Strong>{ref(pr)}</Strong> is being merged
        </Event>
      )}
    </>
  );
}

// GitHub's merge box: each thing between the cluster and merging it, or that nothing is.
function MergeBox({ cluster }: { cluster: Cluster }) {
  const prs = cluster.pullRequests;
  const failing = prs.filter((pr) => pr.checksFailing);
  const running = prs.filter((pr) => !pr.checksFailing && !pr.checksPassing);
  const unapproved = prs.filter((pr) => !pr.approved);
  const behind = prs.filter((pr) => pr.behind || pr.conflicts);
  // Alternatives answering the same thing wait on the user as one.
  const waiting = bySubject(prs.flatMap((pr) => pr.proposals)).length;
  const by = cluster.mergeBy;
  const forced = prs.filter((pr) => pr.mergeBy === 'admin').length;
  const refs = (list: PullRequest[]) => list.map(ref).join(', ');
  const rows: [LucideIcon, string, string][] = [];
  rows.push(
    failing.length > 0
      ? [CircleX, t3.deletion, `Checks fail on ${refs(failing)}`]
      : running.length > 0
        ? [CircleDashed, t3.warning, `Checks running on ${refs(running)}`]
        : [CircleCheck, t3.addition, 'All checks have passed'],
  );
  rows.push(
    unapproved.length === 0
      ? [CircleCheck, t3.addition, 'Every pull request is approved']
      : by === 'admin'
        ? [TriangleAlert, t3.warning, `${forced} not approved · you are an admin, so you can force ${forced === 1 ? 'it' : 'them'} in`]
        : [CircleDashed, t3.warning, `Review required on ${refs(unapproved)}`],
  );
  if (behind.length > 0) rows.push([GitBranch, t3.warning, `${refs(behind)} must be rebased onto ${behind[0].base}`]);
  if (waiting > 0) rows.push([MessageSquare, t3.warning, `${waiting} ${waiting === 1 ? 'conversation waits' : 'conversations wait'} on you`]);
  const tone = by === 'approval' ? t3.addition : by === 'admin' ? t3.warning : t3.border;
  return (
    <View style={styles.mergeRow}>
      <Badge icon={GitMerge} tone={by === 'unspecified' ? undefined : tone} />
      <View style={[styles.mergeBox, { borderColor: by === 'unspecified' ? t3.border : tone }]}>
        <Text style={styles.strong}>
          {by === 'approval'
            ? 'Ready to merge'
            : by === 'admin'
              ? 'Ready to force-merge'
              : 'Not ready to merge'}
        </Text>
        {rows.map(([icon, color, text]) => (
          <View key={text} style={styles.mergeLine}>
            <Icon icon={icon} size={14} color={color} />
            <Text style={styles.mergeText}>{text}</Text>
          </View>
        ))}
      </View>
    </View>
  );
}

// ——— Sidebar ———

function SideSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <View style={styles.sideSection}>
      <Text style={styles.sideTitle}>{title}</Text>
      {children}
    </View>
  );
}

// The cluster's PRs, base first, each with its checks, review and base.
// A phone lists each PR's facts on one wrapping line, so the timeline starts on the first screen.
function Stack({ cluster, busy, onRebase }: Props) {
  const phone = usePhone();
  return (
    <View style={styles.stack}>
      {cluster.pullRequests.map((pr, i) => {
        const checks = pr.checks;
        const total = checks?.total ?? 0;
        const failed = checks?.failed ?? 0;
        const passed = (checks?.done ?? 0) - failed;
        return (
          <View key={pr.url} style={styles.stackItem}>
            <View style={styles.stackRail}>
              <View style={[styles.stackDot, i === 0 && styles.stackDotBase]} />
              {i < cluster.pullRequests.length - 1 && <View style={styles.stackLine} />}
            </View>
            <View style={styles.stackBody}>
              <Pressable onPress={() => Linking.openURL(pr.url)}>
                <Text style={styles.stackRef}>{ref(pr)}</Text>
                <Text numberOfLines={2} style={styles.stackTitle}>
                  {pr.title}
                </Text>
              </Pressable>
              <View style={[styles.facts, phone && styles.factsPhone]}>
                <Fact icon={pr.checksFailing ? CircleX : pr.checksPassing ? CircleCheck : CircleDashed}
                  color={pr.checksFailing ? t3.deletion : pr.checksPassing ? t3.addition : t3.warning}
                  text={total === 0 ? 'No checks' : failed > 0 ? `${failed} failed · ${passed} passed` : `${passed}/${total} passed`}
                />
                <Fact
                  icon={pr.approved ? Check : CircleDashed}
                  color={pr.approved ? t3.addition : t3.faint}
                  text={pr.approved ? 'Approved' : 'Review required'}
                />
                {(!phone || pr.behind || pr.conflicts) && <Fact
                  icon={pr.conflicts ? TriangleAlert : pr.behind ? GitBranch : RefreshCw}
                  color={pr.conflicts ? t3.deletion : pr.behind ? t3.warning : t3.faint}
                  text={pr.conflicts ? `Conflicts with ${pr.base}` : pr.behind ? `Behind ${pr.base}` : `Up to date with ${pr.base}`}
                />}
              </View>
              {(pr.behind || pr.rebasing || pr.rebased) && (
                <View style={styles.stackAction}>
                  <Button
                    label={pr.rebasing ? 'Rebasing…' : pr.rebased ? 'Rebased' : 'Rebase'}
                    disabled={pr.rebasing || pr.rebased || busy?.has(pr.url)}
                    onPress={() => onRebase(pr.url)}
                  />
                </View>
              )}
              <StatusLine pr={pr} agents={cluster.agents} />
            </View>
          </View>
        );
      })}
    </View>
  );
}

function Fact({ icon, color, text }: { icon: LucideIcon; color: string; text: string }) {
  return (
    <View style={styles.fact}>
      <Icon icon={icon} size={12} color={color} />
      <Text style={styles.factText}>{text}</Text>
    </View>
  );
}

const BADGE = 28;
const SIDEBAR = 300;

const styles = StyleSheet.create({
  page: {
    width: '100%',
    maxWidth: 1240,
    alignSelf: 'center',
    gap: 20,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 16,
    paddingBottom: 16,
    paddingRight: 24,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  headerPhone: {
    flexDirection: 'column',
    paddingRight: 0,
    gap: 12,
  },
  titleBlock: {
    flex: 1,
    gap: 10,
  },
  title: {
    color: t3.foreground,
    fontSize: 22,
    fontWeight: '600',
    lineHeight: 28,
  },
  titlePhone: {
    fontSize: 20,
    lineHeight: 26,
  },
  refs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 6,
  },
  state: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 10,
    height: 24,
    borderRadius: 999,
    backgroundColor: '#238636',
  },
  stateMerging: {
    backgroundColor: '#8957E5',
  },
  stateText: {
    color: '#fff',
    fontSize: 12,
    fontWeight: '600',
  },
  refChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 8,
    height: 24,
    borderRadius: 999,
    backgroundColor: t3.input,
  },
  refText: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 8,
  },
  actionsPhone: {
    justifyContent: 'flex-start',
  },
  body: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 32,
  },
  bodyPhone: {
    flexDirection: 'column',
    alignItems: 'stretch',
    gap: 20,
  },
  main: {
    flex: 1,
    minWidth: 0,
    gap: 16,
  },
  sidebar: {
    position: 'sticky',
    top: 0,
    width: SIDEBAR,
    flexShrink: 0,
    gap: 20,
  } as object,
  sidebarPhone: {
    position: 'relative',
    width: '100%',
    padding: 12,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  } as object,
  sideSection: {
    gap: 10,
  },
  sideTitle: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  stack: {
    gap: 0,
  },
  stackItem: {
    flexDirection: 'row',
    gap: 10,
  },
  stackRail: {
    width: 12,
    alignItems: 'center',
    paddingTop: 4,
  },
  stackDot: {
    width: 10,
    height: 10,
    borderRadius: 5,
    borderWidth: 2,
    borderColor: '#8FA8F5',
    backgroundColor: t3.background,
  },
  stackDotBase: {
    backgroundColor: '#8FA8F5',
  },
  stackLine: {
    flex: 1,
    width: 2,
    marginVertical: 2,
    backgroundColor: 'rgba(143, 168, 245, 0.3)',
  },
  stackBody: {
    flex: 1,
    minWidth: 0,
    gap: 6,
    paddingBottom: 14,
  },
  stackRef: {
    color: '#8FA8F5',
    fontFamily: t3.mono,
    fontSize: 12,
  },
  stackTitle: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 18,
  },
  facts: {
    gap: 3,
  },
  factsPhone: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    columnGap: 12,
    rowGap: 3,
  },
  fact: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  factText: {
    color: t3.muted,
    fontSize: 12,
  },
  stackAction: {
    flexDirection: 'row',
  },
  timeline: {
    gap: 14,
  },
  // Runs through the badges' centers, under them.
  rail: {
    position: 'absolute',
    top: 8,
    bottom: 8,
    left: BADGE / 2 - 1,
    width: 2,
    backgroundColor: t3.border,
  },
  event: {
    gap: 10,
  },
  eventLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    minHeight: BADGE,
  },
  eventText: {
    flex: 1,
    color: t3.muted,
    fontSize: 13,
    lineHeight: 19,
  },
  eventBody: {
    marginLeft: BADGE + 10,
    gap: 10,
  },
  trailing: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  badge: {
    width: BADGE,
    height: BADGE,
    borderRadius: BADGE / 2,
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: '#222C37',
  },
  strong: {
    color: t3.foreground,
    fontWeight: '600',
  },
  mono: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
    paddingHorizontal: 4,
    borderRadius: 4,
    backgroundColor: t3.input,
  },
  prTitle: {
    color: t3.foreground,
  },
  when: {
    color: t3.faint,
  },
  card: {
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    overflow: 'hidden',
  },
  cardHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  cardBody: {
    padding: 12,
  },
  divider: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    marginVertical: 4,
  },
  dividerLine: {
    flex: 1,
    height: 1,
    backgroundColor: t3.border,
  },
  dividerText: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
  },
  mergeRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 10,
  },
  mergeBox: {
    flex: 1,
    gap: 8,
    padding: 12,
    borderRadius: 12,
    borderWidth: 1,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  mergeLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  mergeText: {
    flex: 1,
    color: t3.muted,
    fontSize: 13,
  },
  composerRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 10,
  },
  composer: {
    flex: 1,
    padding: 12,
    gap: 4,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
  },
  composerTitle: {
    color: t3.foreground,
    fontSize: 13,
    fontWeight: '600',
  },
  meta: {
    color: t3.faint,
    fontSize: 12,
  },
});
