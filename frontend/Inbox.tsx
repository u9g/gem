import type { LucideIcon } from 'lucide-react';
import {
  Bot,
  Check,
  CircleX,
  Clock,
  ExternalLink,
  Eye,
  GitBranch,
  GitMerge,
  Inbox as InboxIcon,
  MessageCircle,
  SquarePen,
  Undo2,
  X,
} from 'lucide-react';
import { type ReactNode, useCallback, useMemo, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { active } from './agents';
import { Drawer } from './Board';
import { timeAgo } from './ClusterView';
import type { Agent, Cluster, Proposal, PullRequest } from './view-types';
import { t3 } from './t3';

// Why a cluster or agent is in the inbox, most urgent first: the order a row's reasons are listed in,
// and the first one names the row.
export const reasons = ['failing', 'proposal', 'comment', 'agent', 'behind', 'ready', 'review'] as const;
export type Reason = (typeof reasons)[number];

const ready = '#4ADE80';
const link = '#8FA8F5';

const reasonInfo: Record<Reason, { label: string; filter: string; icon: LucideIcon; color: string }> = {
  failing: { label: 'Checks failing', filter: 'Checks failing', icon: CircleX, color: t3.deletion },
  proposal: { label: 'Proposal', filter: 'Proposals', icon: SquarePen, color: link },
  comment: { label: 'Comment', filter: 'Comments', icon: MessageCircle, color: link },
  agent: { label: 'Agent finished', filter: 'Agents finished', icon: Bot, color: t3.addition },
  behind: { label: 'Behind base', filter: 'Behind base', icon: GitBranch, color: t3.warning },
  ready: { label: 'Ready to merge', filter: 'Ready to merge', icon: GitMerge, color: ready },
  review: { label: 'Review requested', filter: 'Review requested', icon: Eye, color: t3.muted },
};

// One row: a cluster, or an agent acting on no open pull request. `why` says, for each reason, what
// happened in a short line; a row without reasons waits on someone else, which `waiting` names.
export type Item = {
  key: string;
  cluster?: Cluster;
  agent?: Agent;
  repository: string;
  title: string;
  refs: string;
  url: string;
  at?: number;
  reasons: Reason[];
  why: Partial<Record<Reason, string>>;
  waiting: string;
};

// A comment's first line of prose, without the HTML comments and markdown bots wrap it in.
function firstLine(markdown: string): string {
  const prose = markdown
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/<[^>]+>/g, '')
    .replace(/[*_`#>]/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .split('\n')
    .map((line) => line.trim())
    .find((line) => line.length > 0);
  return prose ?? '';
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const ref = (pr: PullRequest) => `${pr.repository?.name ?? ''}#${pr.number}`;

function proposalWhy(proposals: Proposal[]): string {
  const first = proposals[0];
  const more = proposals.length > 1 ? ` · +${proposals.length - 1} more` : '';
  return `${first.summary}${more}`;
}

function commentWhy(proposals: Proposal[]): string {
  const comment = proposals[0].comment!;
  const more = proposals.length > 1 ? ` · +${plural(proposals.length - 1, 'comment')}` : '';
  return `${comment.author}: ${firstLine(comment.body)}${more}`;
}

function finishedWhy(agents: Agent[]): string {
  const agent = agents[0];
  const reply = firstLine(agent.reply);
  return reply ? `${agent.title}: ${reply}` : `${agent.title} is waiting on you`;
}

function clusterItem(cluster: Cluster): Item {
  const prs = cluster.pullRequests;
  const first = prs[0];
  const working = cluster.agents.filter(active);
  const running = (pr: PullRequest) => (pr.checks?.total ?? 0) > (pr.checks?.done ?? 0);
  const answered = (pr: PullRequest) => working.some((agent) => agent.pullRequestUrls.includes(pr.url));
  const proposals = [...prs.flatMap((pr) => pr.proposals), ...cluster.agents.flatMap((agent) => agent.proposals)];
  const comments = proposals.filter((proposal) => proposal.comment);
  const others = proposals.filter((proposal) => !proposal.comment);
  const finished = cluster.agents.filter((agent) => !active(agent) && agent.proposals.length === 0);
  const failing = prs.filter((pr) => pr.checksFailing && !running(pr) && !answered(pr));
  const behind = prs.filter((pr) => (pr.behind || pr.conflicts) && !pr.rebasing && !pr.rebased);
  const merging = prs.some((pr) => pr.merging);
  const unapproved = prs.filter((pr) => !pr.approved);

  const why: Partial<Record<Reason, string>> = {};
  if (failing.length > 0) {
    why.failing = failing
      .map((pr) => `${pr.checks ? `${pr.checks.failed} of ${pr.checks.total}` : 'Some'} checks failed on ${ref(pr)}`)
      .join(' · ');
  }
  if (others.length > 0) why.proposal = proposalWhy(others);
  if (comments.length > 0) why.comment = commentWhy(comments);
  if (finished.length > 0) why.agent = finishedWhy(finished);
  if (behind.length > 0) {
    why.behind = behind
      .map((pr) => `${ref(pr)} ${pr.conflicts ? 'conflicts with' : 'is behind'} ${pr.base || 'its base'}`)
      .join(' · ');
  }
  if (cluster.mergeBy !== 'unspecified' && !merging) {
    why.ready =
      cluster.mergeBy === 'admin'
        ? `Checks pass; ${plural(unapproved.length, 'PR')} unapproved, so it merges as an admin`
        : prs.length === 1
          ? 'Approved and passing'
          : `All ${prs.length} PRs approved and passing`;
  }
  if (
    Object.keys(why).length === 0 &&
    !merging &&
    working.length === 0 &&
    unapproved.length > 0 &&
    prs.every((pr) => pr.checksPassing)
  ) {
    why.review = `Checks pass; waiting on a review of ${unapproved.map(ref).join(', ')}`;
  }

  const waiting = merging
    ? 'Merging'
    : prs.some((pr) => pr.rebasing || pr.rebased)
      ? 'Rebasing'
      : working.length > 0
        ? `${plural(working.length, 'agent')} working`
        : prs.some(running)
          ? 'Checks running'
          : 'Waiting on review';

  return {
    key: first.url,
    cluster,
    repository: first.repository ? `${first.repository.owner}/${first.repository.name}` : '',
    title: cluster.name || first.title,
    refs: prs.map((pr) => (pr.repository?.name === first.repository?.name ? `#${pr.number}` : ref(pr))).join(' · '),
    url: first.url,
    at: cluster.updatedAt,
    reasons: reasons.filter((reason) => why[reason] !== undefined),
    why,
    waiting,
  };
}

function agentItem(agent: Agent): Item {
  const url = agent.pullRequestUrls[0] ?? '';
  const [, owner, name, , number] = url.replace('https://github.com', '').split('/');
  const why: Partial<Record<Reason, string>> = {};
  if (agent.proposals.length > 0) why.proposal = proposalWhy(agent.proposals);
  else if (!active(agent)) why.agent = firstLine(agent.reply) || 'Waiting on you';
  return {
    key: `agent:${agent.id}`,
    agent,
    repository: owner && name ? `${owner}/${name}` : 'Agents',
    title: agent.title,
    refs: number && !agent.title.includes(`#${number}`) ? `#${number}` : '',
    url,
    at: agent.startedAt,
    reasons: reasons.filter((reason) => why[reason] !== undefined),
    why,
    waiting: agent.state === 'queued' ? 'Queued' : 'Agent working',
  };
}

export function inboxItems(clusters: readonly Cluster[], agents: readonly Agent[]): Item[] {
  return [...clusters.map(clusterItem), ...agents.map(agentItem)].sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
}

type Folder = 'inbox' | 'waiting' | 'done';

const DONE_KEY = 'gem.inbox.done';

// What was marked done, with the time of the update it was marked at: a cluster that updates after
// returns to the inbox, as a GitHub thread does. Kept in this browser only.
function useDone() {
  const [done, setDone] = useState<Record<string, number>>(() => {
    try {
      return JSON.parse(globalThis.localStorage?.getItem(DONE_KEY) ?? '{}');
    } catch {
      return {};
    }
  });
  const save = useCallback((update: (done: Record<string, number>) => Record<string, number>) => {
    setDone((old) => {
      const next = update(old);
      try {
        globalThis.localStorage?.setItem(DONE_KEY, JSON.stringify(next));
      } catch {}
      return next;
    });
  }, []);
  const isDone = (item: Item) => done[item.key] !== undefined && done[item.key] >= (item.at ?? 0);
  const mark = (item: Item) => save((old) => ({ ...old, [item.key]: item.at ?? Date.now() }));
  const unmark = (item: Item) =>
    save((old) => {
      const { [item.key]: _, ...rest } = old;
      return rest;
    });
  return { isDone, mark, unmark };
}

function folderOf(item: Item, isDone: (item: Item) => boolean): Folder {
  if (isDone(item)) return 'done';
  return item.reasons.length > 0 ? 'inbox' : 'waiting';
}

// The day an item last changed, for grouping by date.
function dayOf(at: number | undefined): string {
  if (at === undefined) return 'Older';
  const days = (new Date().setHours(24, 0, 0, 0) - at) / 86_400_000;
  if (days <= 1) return 'Today';
  if (days <= 2) return 'Yesterday';
  if (days <= 7) return 'This week';
  return 'Older';
}

// GitHub's notifications inbox over the clusters and the agents acting on no open pull request: a
// row for each that may need the user, why, and when; a sidebar to filter them by reason and
// repository; and the open cluster or agent, `children`, in a drawer over it. On a phone the
// sidebar is a row of chips and the parent shows what is open.
export function Inbox({
  clusters,
  agents,
  toolbar,
  openKey,
  onOpenCluster,
  onOpenAgent,
  onClose,
  phone = false,
  wide = false,
  children,
}: {
  clusters: readonly Cluster[];
  agents: readonly Agent[];
  // The search box and server chips, at the top.
  toolbar?: ReactNode;
  // The open row's key: its cluster's oldest PR's URL, or `agent:` and the agent's ID.
  openKey?: string;
  onOpenCluster: (key: string) => void;
  onOpenAgent: (id: string) => void;
  onClose: () => void;
  phone?: boolean;
  wide?: boolean;
  children?: ReactNode;
}) {
  const items = useMemo(() => inboxItems(clusters, agents), [clusters, agents]);
  const { isDone, mark, unmark } = useDone();
  const [folder, setFolder] = useState<Folder>('inbox');
  const [reason, setReason] = useState<Reason>();
  const [repository, setRepository] = useState<string>();
  const [groupBy, setGroupBy] = useState<'repository' | 'date'>('repository');

  const inFolder = items.filter((item) => folderOf(item, isDone) === folder);
  const folderCounts: Record<Folder, number> = { inbox: 0, waiting: 0, done: 0 };
  for (const item of items) folderCounts[folderOf(item, isDone)]++;
  const reasonCounts = reasons.map((r) => ({
    reason: r,
    count: inFolder.filter((item) => item.reasons.includes(r) && (!repository || item.repository === repository)).length,
  }));
  const repositories = [...new Set(items.map((item) => item.repository))]
    .sort()
    .map((name) => ({
      name,
      count: inFolder.filter((item) => item.repository === name && (!reason || item.reasons.includes(reason))).length,
    }));
  const listed = inFolder.filter(
    (item) => (!reason || item.reasons.includes(reason)) && (!repository || item.repository === repository),
  );
  const groups: { name: string; items: Item[] }[] = [];
  for (const item of listed) {
    const name = groupBy === 'repository' ? item.repository : dayOf(item.at);
    const group = groups.find((g) => g.name === name);
    if (group) group.items.push(item);
    else groups.push({ name, items: [item] });
  }

  const open = (item: Item) => (item.agent ? onOpenAgent(item.agent.id) : onOpenCluster(item.key));
  const chooseFolder = (to: Folder) => {
    setFolder(to);
    setReason(undefined);
  };

  const empty = (
    <View style={styles.empty}>
      <Icon icon={folder === 'inbox' ? Check : InboxIcon} size={28} color={t3.faint} />
      <Text style={styles.emptyTitle}>
        {folder === 'inbox' ? 'All caught up' : folder === 'done' ? 'Nothing marked done' : 'Nothing waiting'}
      </Text>
      <Text style={styles.emptyText}>
        {reason || repository ? 'Nothing matches these filters.' : folder === 'inbox' ? 'Nothing needs you right now.' : ''}
      </Text>
    </View>
  );

  const list = (
    <ScrollView style={styles.list} contentContainerStyle={phone ? styles.phoneListContent : styles.listContent}>
      {groups.length === 0
        ? empty
        : groups.map((group) => (
            <View key={group.name} style={[styles.group, phone && styles.phoneGroup]}>
              <View style={styles.groupHead}>
                <Text style={styles.groupName}>{group.name}</Text>
                <Text style={styles.count}>{group.items.length}</Text>
              </View>
              {group.items.map((item, i) => (
                <InboxRow
                  key={item.key}
                  item={item}
                  reason={reason}
                  folder={folder}
                  phone={phone}
                  first={i === 0}
                  showRepository={groupBy === 'date'}
                  selected={item.key === openKey}
                  onOpen={open}
                  onDone={folder === 'done' ? unmark : mark}
                />
              ))}
            </View>
          ))}
    </ScrollView>
  );

  const activeChips = (
    <>
      {reason && <FilterChip label={`reason: ${reasonInfo[reason].filter}`} onRemove={() => setReason(undefined)} />}
      {repository && <FilterChip label={`repo: ${repository}`} onRemove={() => setRepository(undefined)} />}
    </>
  );

  if (phone) {
    return (
      <View style={styles.inbox}>
        {toolbar}
        <ScrollView horizontal showsHorizontalScrollIndicator={false} style={styles.chipScroll} contentContainerStyle={styles.chipRow}>
          {(['inbox', 'waiting', 'done'] as const).map((f) => (
            <Chip key={f} label={folderLabel[f]} count={folderCounts[f]} selected={folder === f} onPress={() => chooseFolder(f)} />
          ))}
          <View style={styles.chipDivider} />
          {reasonCounts
            .filter(({ count, reason: r }) => count > 0 || r === reason)
            .map(({ reason: r, count }) => (
              <Chip
                key={r}
                icon={reasonInfo[r].icon}
                color={reasonInfo[r].color}
                label={reasonInfo[r].filter}
                count={count}
                selected={reason === r}
                onPress={() => setReason(reason === r ? undefined : r)}
              />
            ))}
        </ScrollView>
        <ScrollView horizontal showsHorizontalScrollIndicator={false} style={styles.chipScroll} contentContainerStyle={styles.chipRow}>
          {repositories
            .filter(({ name, count }) => count > 0 || name === repository)
            .map(({ name, count }) => (
            <Chip
              key={name}
              label={name.split('/').pop()!}
              count={count}
              selected={repository === name}
              onPress={() => setRepository(repository === name ? undefined : name)}
            />
          ))}
        </ScrollView>
        {list}
      </View>
    );
  }

  return (
    <View style={styles.inbox}>
      <View style={styles.topBar}>{toolbar}</View>
      <View style={styles.body}>
        <ScrollView style={styles.sidebar} contentContainerStyle={styles.sidebarContent}>
          {(['inbox', 'waiting', 'done'] as const).map((f) => (
            <SideItem
              key={f}
              icon={folderIcon[f]}
              label={folderLabel[f]}
              count={folderCounts[f]}
              strong={f === 'inbox'}
              selected={folder === f && !reason}
              onPress={() => chooseFolder(f)}
            />
          ))}
          <Text style={styles.sideTitle}>Filters</Text>
          {reasonCounts.map(({ reason: r, count }) => (
            <SideItem
              key={r}
              icon={reasonInfo[r].icon}
              color={reasonInfo[r].color}
              label={reasonInfo[r].filter}
              count={count}
              selected={reason === r}
              onPress={() => setReason(reason === r ? undefined : r)}
            />
          ))}
          <Text style={styles.sideTitle}>Repositories</Text>
          {repositories.map(({ name, count }) => (
            <SideItem
              key={name}
              label={name}
              count={count}
              selected={repository === name}
              onPress={() => setRepository(repository === name ? undefined : name)}
            />
          ))}
        </ScrollView>
        <View style={styles.main}>
          <View style={styles.mainHead}>
            <Text style={styles.mainTitle}>{folderLabel[folder]}</Text>
            {activeChips}
            <View style={{ flex: 1 }} />
            <Text style={styles.groupLabel}>Group by</Text>
            <Segmented
              options={[
                ['repository', 'Repository'],
                ['date', 'Date'],
              ]}
              value={groupBy}
              onChange={setGroupBy}
            />
          </View>
          {list}
        </View>
        {children && (
          <Drawer wide={wide} onClose={onClose}>
            {children}
          </Drawer>
        )}
      </View>
    </View>
  );
}

const folderLabel: Record<Folder, string> = { inbox: 'Inbox', waiting: 'Waiting', done: 'Done' };
const folderIcon: Record<Folder, LucideIcon> = { inbox: InboxIcon, waiting: Clock, done: Check };

function Icon({ icon: I, size = 16, color = t3.muted }: { icon: LucideIcon; size?: number; color?: string }) {
  return <I size={size} color={color} strokeWidth={2} />;
}

function SideItem({
  icon,
  color,
  label,
  count,
  strong = false,
  selected,
  onPress,
}: {
  icon?: LucideIcon;
  color?: string;
  label: string;
  count: number;
  strong?: boolean;
  selected: boolean;
  onPress: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  return (
    <Pressable
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.sideItem, hovered && styles.hover, selected && styles.sideSelected]}
    >
      {selected && <View style={styles.sideMark} />}
      {icon && <Icon icon={icon} size={14} color={color} />}
      <Text numberOfLines={1} style={[styles.sideLabel, (strong || selected) && styles.sideLabelStrong]}>
        {label}
      </Text>
      {count > 0 && <Text style={[styles.count, strong && styles.countStrong]}>{count}</Text>}
    </Pressable>
  );
}

function Chip({
  icon,
  color,
  label,
  count,
  selected,
  onPress,
}: {
  icon?: LucideIcon;
  color?: string;
  label: string;
  count: number;
  selected: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable onPress={onPress} style={[styles.chip, selected && styles.chipSelected]}>
      {icon && <Icon icon={icon} size={12} color={color} />}
      <Text style={[styles.chipText, selected && styles.chipTextSelected]}>{label}</Text>
      <Text style={styles.chipCount}>{count}</Text>
    </Pressable>
  );
}

function FilterChip({ label, onRemove }: { label: string; onRemove: () => void }) {
  return (
    <Pressable onPress={onRemove} accessibilityLabel={`Remove ${label}`} style={styles.filterChip}>
      <Text style={styles.filterChipText}>{label}</Text>
      <Icon icon={X} size={12} color={link} />
    </Pressable>
  );
}

function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: [T, string][];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <View style={styles.segmented}>
      {options.map(([key, label]) => (
        <Pressable key={key} onPress={() => onChange(key)} style={[styles.segment, value === key && styles.segmentOn]}>
          <Text style={[styles.segmentText, value === key && styles.segmentTextOn]}>{label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

function ReasonChip({ reason }: { reason: Reason }) {
  const { label, color } = reasonInfo[reason];
  return (
    <Text numberOfLines={1} style={[styles.reason, { color, borderColor: `${color}55` }]}>
      {label}
    </Text>
  );
}

function RowAction({ icon, label, onPress }: { icon: LucideIcon; label: string; onPress: () => void }) {
  const [hovered, setHovered] = useState(false);
  return (
    <Pressable
      accessibilityLabel={label}
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.action, hovered && styles.actionHover]}
    >
      <Icon icon={icon} size={15} color={hovered ? t3.foreground : t3.muted} />
    </Pressable>
  );
}

// One notification: what it is and why it is here, and when, which hovering swaps for its actions.
function InboxRow({
  item,
  reason,
  folder,
  phone,
  first,
  showRepository,
  selected,
  onOpen,
  onDone,
}: {
  item: Item;
  // The reason filtered by, which names the row in place of its first.
  reason?: Reason;
  folder: Folder;
  phone: boolean;
  first: boolean;
  showRepository: boolean;
  selected: boolean;
  onOpen: (item: Item) => void;
  onDone: (item: Item) => void;
}) {
  const [hovered, setHovered] = useState(false);
  const primary = reason ?? item.reasons[0];
  const info = primary ? reasonInfo[primary] : undefined;
  const more = item.reasons.length - 1;
  const why = primary ? item.why[primary] : item.waiting;
  const when = item.at !== undefined ? timeAgo(new Date(item.at)) : '';
  const doneAction = (
    <RowAction
      icon={folder === 'done' ? Undo2 : Check}
      label={folder === 'done' ? 'Move to inbox' : 'Mark as done'}
      onPress={() => onDone(item)}
    />
  );
  const muted = folder !== 'inbox';

  return (
    <Pressable
      onPress={() => onOpen(item)}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.row, phone && styles.phoneRow, !first && styles.rowBorder, hovered && styles.hover, selected && styles.rowSelected]}
    >
      {!muted && <View style={[styles.unread, phone && styles.phoneUnread]} />}
      <View style={styles.status}>
        <Icon icon={info?.icon ?? (item.agent ? Bot : Clock)} color={info?.color ?? t3.faint} />
      </View>
      <View style={styles.text}>
        <Text numberOfLines={phone ? 2 : 1} style={styles.title}>
          <Text style={[styles.titleText, muted && styles.titleMuted]}>{item.title}</Text>
          {item.refs ? <Text style={styles.refs}>{'  '}{showRepository ? `${item.repository.split('/').pop()} ` : ''}{item.refs}</Text> : null}
        </Text>
        <Text numberOfLines={1} style={styles.why}>
          {why}
        </Text>
        {phone && (
          <View style={styles.phoneMeta}>
            {primary ? <ReasonChip reason={primary} /> : <Text style={styles.waiting}>{item.waiting}</Text>}
            {more > 0 && <Text style={styles.more}>+{more}</Text>}
            <Text style={styles.time}>{when}</Text>
          </View>
        )}
      </View>
      {phone ? (
        doneAction
      ) : (
        <>
          <View style={styles.reasonSlot}>
            {primary ? <ReasonChip reason={primary} /> : <Text style={styles.waiting}>{item.waiting}</Text>}
            {more > 0 && <Text style={styles.more}>+{more}</Text>}
          </View>
          <View style={styles.trailing}>
            {hovered ? (
              <View style={styles.actions}>
                {item.url ? (
                  <RowAction icon={ExternalLink} label="Open on GitHub" onPress={() => void Linking.openURL(item.url)} />
                ) : null}
                {doneAction}
              </View>
            ) : (
              <Text numberOfLines={1} style={styles.time}>
                {when}
              </Text>
            )}
          </View>
        </>
      )}
    </Pressable>
  );
}

const SIDEBAR_WIDTH = 248;

const styles = StyleSheet.create({
  inbox: {
    flex: 1,
    backgroundColor: t3.background,
  },
  topBar: {
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  body: {
    flex: 1,
    flexDirection: 'row',
  },
  sidebar: {
    width: SIDEBAR_WIDTH,
    flexGrow: 0,
    borderRightWidth: 1,
    borderRightColor: t3.border,
  },
  sidebarContent: {
    padding: 12,
    gap: 1,
  },
  sideTitle: {
    color: t3.faint,
    fontSize: 11,
    fontWeight: '600',
    letterSpacing: 0.5,
    textTransform: 'uppercase',
    paddingHorizontal: 10,
    paddingTop: 18,
    paddingBottom: 6,
  },
  sideItem: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 32,
    paddingHorizontal: 10,
    borderRadius: 8,
  },
  sideSelected: {
    backgroundColor: 'rgba(255, 255, 255, 0.07)',
  },
  sideMark: {
    position: 'absolute',
    left: -6,
    top: 7,
    bottom: 7,
    width: 3,
    borderRadius: 2,
    backgroundColor: t3.primary,
  },
  sideLabel: {
    flex: 1,
    color: t3.muted,
    fontSize: 13,
  },
  sideLabelStrong: {
    color: t3.foreground,
    fontWeight: '600',
  },
  count: {
    color: t3.muted,
    fontSize: 11,
    fontWeight: '600',
    minWidth: 20,
    paddingHorizontal: 6,
    lineHeight: 18,
    borderRadius: 9,
    textAlign: 'center',
    backgroundColor: t3.input,
    fontVariant: ['tabular-nums'],
  },
  countStrong: {
    color: '#FFFFFF',
    backgroundColor: t3.primary,
  },
  hover: {
    backgroundColor: t3.hover,
  },
  main: {
    flex: 1,
    minWidth: 0,
  },
  mainHead: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 8,
    paddingHorizontal: 24,
    paddingTop: 18,
    paddingBottom: 6,
  },
  mainTitle: {
    color: t3.foreground,
    fontSize: 18,
    fontWeight: '600',
    marginRight: 6,
  },
  groupLabel: {
    color: t3.faint,
    fontSize: 12,
  },
  segmented: {
    flexDirection: 'row',
    padding: 2,
    borderRadius: 8,
    backgroundColor: t3.input,
  },
  segment: {
    paddingHorizontal: 10,
    height: 22,
    justifyContent: 'center',
    borderRadius: 6,
  },
  segmentOn: {
    backgroundColor: '#2E3946',
  },
  segmentText: {
    color: t3.muted,
    fontSize: 12,
  },
  segmentTextOn: {
    color: t3.foreground,
    fontWeight: '500',
  },
  filterChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingLeft: 9,
    paddingRight: 6,
    height: 24,
    borderRadius: 999,
    borderWidth: 1,
    borderColor: 'rgba(55, 103, 232, 0.45)',
    backgroundColor: 'rgba(55, 103, 232, 0.12)',
  },
  filterChipText: {
    color: link,
    fontSize: 12,
  },
  list: {
    flex: 1,
  },
  listContent: {
    paddingHorizontal: 24,
    paddingTop: 10,
    paddingBottom: 32,
    gap: 16,
  },
  phoneListContent: {
    paddingTop: 4,
    paddingBottom: 24,
    gap: 12,
  },
  group: {
    borderWidth: 1,
    borderColor: t3.border,
    borderRadius: 12,
    overflow: 'hidden',
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  phoneGroup: {
    borderRadius: 0,
    borderLeftWidth: 0,
    borderRightWidth: 0,
  },
  groupHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 16,
    height: 38,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  groupName: {
    color: t3.foreground,
    fontSize: 13,
    fontWeight: '600',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    minHeight: 56,
    paddingLeft: 22,
    paddingRight: 12,
    paddingVertical: 8,
  },
  phoneRow: {
    alignItems: 'flex-start',
    paddingLeft: 18,
    paddingVertical: 10,
  },
  rowBorder: {
    borderTopWidth: 1,
    borderTopColor: t3.border,
  },
  rowSelected: {
    backgroundColor: 'rgba(55, 103, 232, 0.10)',
  },
  unread: {
    position: 'absolute',
    left: 9,
    top: 25,
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: t3.primary,
  },
  phoneUnread: {
    top: 18,
  },
  status: {
    width: 18,
    alignItems: 'center',
    paddingTop: 1,
  },
  text: {
    flex: 1,
    minWidth: 0,
    gap: 2,
  },
  title: {
    fontSize: 14,
    lineHeight: 20,
  },
  titleText: {
    color: t3.foreground,
    fontWeight: '600',
  },
  titleMuted: {
    color: t3.muted,
    fontWeight: '500',
  },
  refs: {
    color: t3.faint,
    fontVariant: ['tabular-nums'],
  },
  why: {
    color: t3.muted,
    fontSize: 12.5,
    lineHeight: 18,
  },
  reasonSlot: {
    width: 150,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 6,
  },
  reason: {
    fontSize: 11,
    fontWeight: '600',
    paddingHorizontal: 8,
    lineHeight: 18,
    borderRadius: 999,
    borderWidth: 1,
  },
  more: {
    color: t3.faint,
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  waiting: {
    color: t3.faint,
    fontSize: 12,
  },
  trailing: {
    width: 116,
    alignItems: 'flex-end',
  },
  time: {
    color: t3.faint,
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  actions: {
    flexDirection: 'row',
    gap: 2,
  },
  action: {
    width: 28,
    height: 28,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 6,
  },
  actionHover: {
    backgroundColor: t3.input,
  },
  phoneMeta: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    marginTop: 4,
  },
  chipScroll: {
    flexGrow: 0,
  },
  chipRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 12,
    paddingVertical: 6,
  },
  chipDivider: {
    width: 1,
    height: 18,
    marginHorizontal: 2,
    backgroundColor: t3.border,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    height: 30,
    paddingHorizontal: 11,
    borderRadius: 999,
    backgroundColor: t3.input,
    borderWidth: 1,
    borderColor: 'transparent',
  },
  chipSelected: {
    borderColor: 'rgba(55, 103, 232, 0.6)',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
  },
  chipText: {
    color: t3.muted,
    fontSize: 13,
  },
  chipTextSelected: {
    color: t3.foreground,
    fontWeight: '600',
  },
  chipCount: {
    color: t3.faint,
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  empty: {
    alignItems: 'center',
    gap: 6,
    paddingVertical: 80,
  },
  emptyTitle: {
    color: t3.foreground,
    fontSize: 15,
    fontWeight: '600',
  },
  emptyText: {
    color: t3.muted,
    fontSize: 13,
  },
});
