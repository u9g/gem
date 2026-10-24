import { create, fromJson, toJson } from '@bufbuild/protobuf';
import { timestampDate } from '@bufbuild/protobuf/wkt';
import * as Select from '@rn-primitives/select';
import { StatusBar } from 'expo-status-bar';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';

import { byComment, ProposalGroup } from './ProposalView';
import { PullRequestList } from './PullRequestList';
import { type Project, useServer } from './server';
import { Login } from './surfaces';
import {
  ApproveProposalRequestSchema,
  DismissProposalRequestSchema,
  ListOpenPullRequestsRequestSchema,
  ListOpenPullRequestsResponseSchema,
  type PullRequestCluster,
  type Repository,
} from './gen/gem_pb';

const units: [Intl.RelativeTimeFormatUnit, number][] = [
  ['year', 365 * 24 * 60 * 60],
  ['month', 30 * 24 * 60 * 60],
  ['week', 7 * 24 * 60 * 60],
  ['day', 24 * 60 * 60],
  ['hour', 60 * 60],
  ['minute', 60],
  ['second', 1],
];
const relativeTime = new Intl.RelativeTimeFormat('en', { numeric: 'always' });

// Largest unit that fits, truncated: 59 minutes stays "59 minutes ago", not "1 hour ago".
function timeAgo(date: Date): string {
  const seconds = (date.getTime() - Date.now()) / 1000;
  const [unit, size] = units.find(([, size]) => Math.abs(seconds) >= size) ?? units[units.length - 1];
  return relativeTime.format(Math.trunc(seconds / size), unit);
}

const SIDEBAR_WIDTH_KEY = 'gem.sidebarWidth';
const SIDEBAR_DEFAULT_WIDTH = 375;
const SIDEBAR_MIN_WIDTH = 208;
// The main column keeps at least this much of the window.
const MAIN_MIN_WIDTH = 640;

function clampSidebarWidth(width: number): number {
  return Math.round(Math.max(SIDEBAR_MIN_WIDTH, Math.min(width, window.innerWidth - MAIN_MIN_WIDTH)));
}

function sameRepository(a: Repository | undefined, b: Repository | undefined): boolean {
  return a?.owner === b?.owner && a?.name === b?.name;
}

// Select options carry string values; this encodes a repository so it can be looked up again by
// comparing values, never parsed.
function repositoryValue(repository: Repository): string {
  return JSON.stringify([repository.owner, repository.name]);
}

// "5m", "3h", "2d": the largest unit that fits, truncated, for one-line rows.
function shortAgo(date: Date): string {
  const seconds = (Date.now() - date.getTime()) / 1000;
  const [unit, size] = units.find(([, size]) => seconds >= size) ?? units[units.length - 1];
  return `${Math.trunc(seconds / size)}${unit === 'month' ? 'mo' : unit[0]}`;
}

function ready(cluster: PullRequestCluster): boolean {
  return cluster.pullRequests.every((pr) => pr.approved && pr.checksPassing);
}

function pending(cluster: PullRequestCluster): number {
  return cluster.pullRequests.reduce((sum, pr) => sum + pr.proposals.length, 0);
}

export default function App() {
  const { status, addProject, post } = useServer();
  const [clusters, setClusters] = useState<PullRequestCluster[]>([]);
  const [login, setLogin] = useState('');
  const [selectedUrl, setSelectedUrl] = useState<string>();
  const [repoFilter, setRepoFilter] = useState<Repository>();
  const [highlightedRepo, setHighlightedRepo] = useState<string>();
  // Proposal being approved or dismissed.
  const [busy, setBusy] = useState<string>();
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    clampSidebarWidth(Number(localStorage.getItem(SIDEBAR_WIDTH_KEY)) || SIDEBAR_DEFAULT_WIDTH),
  );

  const load = useCallback(
    () =>
      post('/prs', toJson(ListOpenPullRequestsRequestSchema, create(ListOpenPullRequestsRequestSchema))).then(
        (json) => {
          const response = fromJson(ListOpenPullRequestsResponseSchema, json);
          setClusters(response.clusters);
          setLogin(response.login);
        },
      ),
    [post],
  );

  // Proposals arrive whenever their agents finish, not only when the page loads.
  useEffect(() => {
    if (status !== 'online') return;
    load();
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [status, load]);

  function approve(id: string, reply: string, followUp: string) {
    setBusy(id);
    post('/proposals/approve', toJson(ApproveProposalRequestSchema, create(ApproveProposalRequestSchema, { id, reply, followUp })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  function dismiss(id: string) {
    setBusy(id);
    post('/proposals/dismiss', toJson(DismissProposalRequestSchema, create(DismissProposalRequestSchema, { id })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  const repositories = clusters
    .flatMap((cluster) => cluster.pullRequests.flatMap((pr) => (pr.repository ? [pr.repository] : [])))
    .filter((repository, i, all) => all.findIndex((other) => sameRepository(other, repository)) === i)
    .sort((a, b) => a.name.localeCompare(b.name));
  const visible = repoFilter
    ? clusters.filter((cluster) => cluster.pullRequests.some((pr) => sameRepository(pr.repository, repoFilter)))
    : clusters;
  // A cluster is identified by its oldest PR, which stays put as the cluster grows. Until one is
  // picked, the most recently updated one is shown.
  const selected = visible.find((cluster) => cluster.pullRequests[0].url === selectedUrl) ?? visible[0];

  const filters = (
    <View style={styles.filters}>
      <Pressable accessibilityLabel="Change LiveKit project" onPress={() => addProject(undefined)}>
        <Text style={styles.filter}>server: {status}</Text>
      </Pressable>
      <Text style={styles.filter}>author: {login}</Text>
      <Select.Root
        value={repoFilter && { value: repositoryValue(repoFilter), label: repoFilter.name }}
        onValueChange={(option) =>
          setRepoFilter(repositories.find((repository) => repositoryValue(repository) === option?.value))
        }
      >
        <Select.Trigger>
          <Text style={styles.filter}>repos: {repoFilter?.name ?? '*'}</Text>
        </Select.Trigger>
        <Select.Portal>
          <Select.Content position="popper" sideOffset={4} style={styles.menu}>
            <Select.Viewport>
              {[undefined, ...repositories].map((repository) => {
                const value = repository ? repositoryValue(repository) : '*';
                const label = repository?.name ?? '*';
                return (
                  <Select.Item
                    key={value}
                    value={value}
                    label={label}
                    // On web the Item is Radix's DOM element, so its style must be one plain
                    // object of CSS properties. Radix focuses whichever item the pointer or
                    // keyboard highlights.
                    onFocus={() => setHighlightedRepo(value)}
                    onBlur={() => setHighlightedRepo(undefined)}
                    style={StyleSheet.flatten([
                      styles.menuItem,
                      sameRepository(repository, repoFilter) && styles.menuItemSelected,
                      highlightedRepo === value && styles.menuItemActive,
                    ])}
                  >
                    <Select.ItemText style={styles.menuText} />
                  </Select.Item>
                );
              })}
            </Select.Viewport>
          </Select.Content>
        </Select.Portal>
      </Select.Root>
    </View>
  );

  if (status === 'no-project') {
    return <AddProject onAdd={addProject} />;
  }

  return (
    <View style={styles.container}>
      <View style={[styles.sidebar, { width: sidebarWidth }]}>
        <ResizeHandle
          width={sidebarWidth}
          onResize={(width) => setSidebarWidth(clampSidebarWidth(width))}
          onEnd={() => localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth))}
        />
        {filters}
        <ScrollView contentContainerStyle={styles.rows}>
          {visible.map((cluster) => (
            <ClusterRow
              key={cluster.pullRequests[0].url}
              cluster={cluster}
              selected={cluster === selected}
              onPress={() => setSelectedUrl(cluster.pullRequests[0].url)}
            />
          ))}
        </ScrollView>
      </View>
      <Login value={login}>
        <ScrollView style={styles.main} contentContainerStyle={styles.mainContent}>
          {selected && (
            <ClusterView cluster={selected} busy={busy} onApprove={approve} onDismiss={dismiss} />
          )}
        </ScrollView>
      </Login>
      <StatusBar style="auto" />
    </View>
  );
}

// A cluster's title, its PRs, and the conversations waiting on u9g.
function ClusterView({
  cluster,
  busy,
  onApprove,
  onDismiss,
}: {
  cluster: PullRequestCluster;
  busy?: string;
  onApprove: (id: string, reply: string, followUp: string) => void;
  onDismiss: (id: string) => void;
}) {
  return (
    <View style={styles.column}>
      <View style={styles.titleBlock}>
        <Text style={[styles.title, ready(cluster) && styles.ready]}>{cluster.name}</Text>
        <Text style={styles.meta}>
          {cluster.updatedAt && timeAgo(timestampDate(cluster.updatedAt))}
        </Text>
      </View>
      <View style={styles.pullRequests}>
        <PullRequestList pullRequests={cluster.pullRequests} />
      </View>
      {byComment(cluster.pullRequests.flatMap((pr) => pr.proposals)).map((group) => (
        <ProposalGroup
          key={group[0].id}
          proposals={group}
          busy={busy}
          onApprove={onApprove}
          onDismiss={onDismiss}
        />
      ))}
    </View>
  );
}

// The LiveKit project the gem server is configured with: its LIVEKIT_URL, LIVEKIT_API_KEY and
// LIVEKIT_API_SECRET.
function AddProject({ onAdd }: { onAdd: (project: Project) => void }) {
  const [project, setProject] = useState<Project>({ url: '', apiKey: '', apiSecret: '' });
  const field = (key: keyof Project, placeholder: string) => (
    <TextInput
      style={styles.projectInput}
      placeholder={placeholder}
      placeholderTextColor="#6B6A70"
      secureTextEntry={key === 'apiSecret'}
      autoCapitalize="none"
      value={project[key]}
      onChangeText={(value) => setProject({ ...project, [key]: value })}
    />
  );
  return (
    <View style={[styles.container, styles.project]}>
      <Text style={styles.projectText}>Add the LiveKit project your gem server uses</Text>
      {field('url', 'LIVEKIT_URL')}
      {field('apiKey', 'LIVEKIT_API_KEY')}
      {field('apiSecret', 'LIVEKIT_API_SECRET')}
      <Pressable style={styles.projectButton} onPress={() => onAdd(project)}>
        <Text style={styles.projectText}>Connect</Text>
      </Pressable>
    </View>
  );
}

// T3 Code's panel resize handle: an 8px strip over the sidebar's right edge whose 1px line shows
// on hover and turns blue while dragging. The width is saved when the drag ends, not on every move.
function ResizeHandle({
  width,
  onResize,
  onEnd,
}: {
  width: number;
  onResize: (width: number) => void;
  onEnd: () => void;
}) {
  const start = useRef<{ x: number; width: number }>(undefined);
  const [hovered, setHovered] = useState(false);
  const [dragging, setDragging] = useState(false);
  return (
    <View
      style={styles.handle}
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
      onPointerDown={(e) => {
        e.preventDefault();
        (e.currentTarget as unknown as Element).setPointerCapture(e.nativeEvent.pointerId);
        start.current = { x: e.nativeEvent.clientX, width };
        setDragging(true);
      }}
      onPointerMove={(e) => {
        if (start.current) onResize(start.current.width + e.nativeEvent.clientX - start.current.x);
      }}
      onPointerUp={() => {
        start.current = undefined;
        setDragging(false);
        onEnd();
      }}
    >
      <View style={[styles.handleLine, hovered && styles.handleLineHover, dragging && styles.handleLineDrag]} />
    </View>
  );
}

// One line per cluster, like a thread in T3 Code's sidebar: a red dot when a PR's checks fail, the
// name (green when every PR is approved and passing), how many proposals wait, and how long ago.
function ClusterRow({
  cluster,
  selected,
  onPress,
}: {
  cluster: PullRequestCluster;
  selected: boolean;
  onPress: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  const count = pending(cluster);
  return (
    <Pressable
      onPress={onPress}
      onHoverIn={() => setHovered(true)}
      onHoverOut={() => setHovered(false)}
      style={[styles.row, (selected || hovered) && styles.rowActive]}
    >
      <View style={[styles.dot, cluster.pullRequests.some((pr) => pr.checksFailing) && styles.dotFailing]} />
      <Text numberOfLines={1} style={[styles.rowName, ready(cluster) && styles.ready]}>
        {cluster.name || cluster.pullRequests[0].title}
      </Text>
      {count > 0 && <Text style={styles.count}>{count}</Text>}
      {cluster.updatedAt && <Text style={styles.rowTime}>{shortAgo(timestampDate(cluster.updatedAt))}</Text>}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    flexDirection: 'row',
    backgroundColor: '#19212A',
  },
  sidebar: {
    backgroundColor: '#1D2630',
    borderRightWidth: 1,
    borderRightColor: 'rgba(255, 255, 255, 0.06)',
  },
  handle: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    right: -4,
    width: 8,
    zIndex: 1,
    alignItems: 'center',
    cursor: 'col-resize',
    userSelect: 'none',
  } as object,
  handleLine: {
    width: 1,
    height: '100%',
  },
  handleLineHover: {
    backgroundColor: 'rgba(255, 255, 255, 0.12)',
  },
  handleLineDrag: {
    backgroundColor: 'rgba(55, 103, 232, 0.6)',
  },
  filters: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 6,
    padding: 10,
  },
  project: {
    flexDirection: 'column',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 10,
  },
  projectInput: {
    width: 320,
    color: '#D0CFD4',
    fontSize: 14,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 8,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  projectButton: {
    paddingHorizontal: 16,
    paddingVertical: 10,
    borderRadius: 8,
    backgroundColor: 'rgba(255, 255, 255, 0.1)',
  },
  projectText: {
    color: '#D0CFD4',
    fontSize: 14,
  },
  filter: {
    color: '#D0CFD4',
    fontSize: 11,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 999,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  menu: {
    minWidth: 200,
    maxHeight: 320,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2E3946',
    backgroundColor: '#19212A',
  },
  // Reaches the DOM unconverted (see the Select.Item), so only properties CSS also has.
  menuItem: {
    paddingLeft: 12,
    paddingRight: 12,
    paddingTop: 6,
    paddingBottom: 6,
    // Chrome's focus ring (outline-style: auto) ignores outline-width; a solid 0px one draws nothing.
    outlineStyle: 'solid',
    outlineWidth: 0,
  },
  menuText: {
    color: '#D0CFD4',
  },
  menuItemActive: {
    backgroundColor: '#3A4757',
  },
  menuItemSelected: {
    backgroundColor: '#2E3946',
  },
  rows: {
    paddingHorizontal: 8,
    paddingBottom: 8,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 32,
    paddingHorizontal: 8,
    borderRadius: 8,
  },
  rowActive: {
    backgroundColor: 'rgba(255, 255, 255, 0.05)',
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  dotFailing: {
    backgroundColor: '#F87171',
  },
  rowName: {
    flex: 1,
    color: '#D0CFD4',
    fontSize: 14,
  },
  count: {
    color: '#8FA9F5',
    fontSize: 10,
    fontWeight: '600',
    paddingHorizontal: 6,
    lineHeight: 18,
    borderRadius: 4,
    borderWidth: 1,
    borderColor: 'rgba(55, 103, 232, 0.4)',
    backgroundColor: 'rgba(55, 103, 232, 0.1)',
    fontVariant: ['tabular-nums'],
  },
  rowTime: {
    color: 'rgba(208, 207, 212, 0.45)',
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  main: {
    flex: 1,
  },
  mainContent: {
    alignItems: 'center',
    paddingHorizontal: 24,
    paddingVertical: 32,
  },
  column: {
    width: '100%',
    maxWidth: 1240,
    gap: 12,
  },
  titleBlock: {
    gap: 4,
    marginBottom: 8,
  },
  pullRequests: {
    marginBottom: 16,
  },
  title: {
    color: '#D0CFD4',
    fontSize: 20,
    fontWeight: '600',
  },
  ready: {
    color: '#4ADE80',
  },
  meta: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 12,
  },
});
