import { create, fromJson, type JsonValue, toJson } from '@bufbuild/protobuf';
import * as Select from '@rn-primitives/select';
import { StatusBar } from 'expo-status-bar';
import { ChevronLeft } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import {
  ClusterRow,
  InvestigationRow,
  PhoneClusterRow,
  PhoneInvestigationRow,
  SectionTitle,
} from './ClusterRow';
import { ClusterView, timeAgo } from './ClusterView';
import { InvestigationView } from './InvestigationView';
import { ProfileChip, ProfileForm, ProfileTitle } from './Profiles';
import { type Profile, useServer } from './server';
import { Login } from './surfaces';
import { usePhone } from './t3';
import {
  ApproveProposalRequestSchema,
  CloseInvestigationRequestSchema,
  DismissProposalRequestSchema,
  InvestigateRequestSchema,
  type Investigation,
  ListOpenPullRequestsRequestSchema,
  ListOpenPullRequestsResponseSchema,
  MessageClusterRequestSchema,
  MessageClusterResponseSchema,
  MessageInvestigationRequestSchema,
  RebasePullRequestRequestSchema,
  RestartClusterRequestSchema,
  type PullRequestCluster,
  type Repository,
} from './gen/gem_pb';

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

export default function App() {
  const { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post, snapshot } =
    useServer();
  // The profile being changed, or 'new' while one is being added.
  const [editing, setEditing] = useState<Profile | 'new'>();
  const [clusters, setClusters] = useState<PullRequestCluster[]>([]);
  const [investigations, setInvestigations] = useState<Investigation[]>([]);
  const [login, setLogin] = useState('');
  const [selectedUrl, setSelectedUrl] = useState<string>();
  // Shown in place of any cluster while set.
  const [selectedInvestigation, setSelectedInvestigation] = useState<string>();
  const [repoFilter, setRepoFilter] = useState<Repository>();
  const [highlightedRepo, setHighlightedRepo] = useState<string>();
  // Proposal being approved, dismissed or investigated, the oldest PR's URL of the cluster being
  // restarted, the URL of the PR whose rebase is starting, or the investigation being closed.
  const [busy, setBusy] = useState<string>();
  const phone = usePhone();
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    clampSidebarWidth(Number(localStorage.getItem(SIDEBAR_WIDTH_KEY)) || SIDEBAR_DEFAULT_WIDTH),
  );

  const show = useCallback(
    (json: JsonValue) => {
      const response = fromJson(ListOpenPullRequestsResponseSchema, json);
      setClusters(response.clusters);
      setInvestigations(response.investigations);
      setLogin(response.login);
      reported(response.login, response.search);
    },
    [reported],
  );

  const load = useCallback(
    () => post('/prs', toJson(ListOpenPullRequestsRequestSchema, create(ListOpenPullRequestsRequestSchema))).then(show),
    [post, show],
  );

  // Another profile is another server: nothing shown belongs to it.
  const profileId = profile?.id;
  useEffect(() => {
    setClusters([]);
    setInvestigations([]);
    setLogin('');
    setSelectedUrl(undefined);
    setSelectedInvestigation(undefined);
    setRepoFilter(undefined);
  }, [profileId]);

  // Later changes arrive as snapshots.
  useEffect(() => {
    if (status === 'online') load();
  }, [status, load]);

  useEffect(() => {
    if (snapshot) show(snapshot);
  }, [snapshot, show]);

  // On a phone an open cluster or investigation is a history entry, so the browser's back returns
  // to the list.
  useEffect(() => {
    const close = () => {
      setSelectedUrl(undefined);
      setSelectedInvestigation(undefined);
    };
    addEventListener('popstate', close);
    return () => removeEventListener('popstate', close);
  }, []);

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

  function restart(cluster: PullRequestCluster) {
    setBusy(cluster.pullRequests[0].url);
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    post('/clusters/restart', toJson(RestartClusterRequestSchema, create(RestartClusterRequestSchema, { pullRequestUrls })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  // Starts an agent rebasing the PR, without opening its thread.
  function rebase(url: string) {
    setBusy(url);
    post('/prs/rebase', toJson(RebasePullRequestRequestSchema, create(RebasePullRequestRequestSchema, { url })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  function investigate(proposalId: string) {
    setBusy(proposalId);
    post('/investigations/start', toJson(InvestigateRequestSchema, create(InvestigateRequestSchema, { proposalId })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  // Starts an agent on the message, then opens it, where the conversation goes on.
  function messageCluster(cluster: PullRequestCluster, message: string) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    return post(
      '/clusters/message',
      toJson(MessageClusterRequestSchema, create(MessageClusterRequestSchema, { pullRequestUrls, message })),
    )
      .then((json) => fromJson(MessageClusterResponseSchema, json).investigationId)
      .then((id) => load().then(() => setSelectedInvestigation(id)));
  }

  function messageInvestigation(id: string, message: string) {
    return post(
      '/investigations/message',
      toJson(MessageInvestigationRequestSchema, create(MessageInvestigationRequestSchema, { id, message })),
    ).then(load);
  }

  function closeInvestigation(id: string) {
    setBusy(id);
    post('/investigations/close', toJson(CloseInvestigationRequestSchema, create(CloseInvestigationRequestSchema, { id })))
      .then(() => {
        if (phone) history.back();
        setSelectedInvestigation(undefined);
        return load();
      })
      .finally(() => setBusy(undefined));
  }

  const investigation = investigations.find((i) => i.id === selectedInvestigation);
  const investigationView = investigation && (
    <InvestigationView
      investigation={investigation}
      ago={timeAgo}
      busy={busy}
      onApprove={approve}
      onDismiss={dismiss}
      onMessage={(message) => messageInvestigation(investigation.id, message)}
      onClose={() => closeInvestigation(investigation.id)}
    />
  );

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
      <Pressable accessibilityLabel="Change profile" onPress={() => setEditing(profile)}>
        <Text style={styles.filter}>server: {status}</Text>
      </Pressable>
      {!phone && profile && (
        <ProfileChip profiles={profiles} profile={profile} onSelect={selectProfile} onAdd={() => setEditing('new')} />
      )}
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

  if (!profile || editing) {
    const changing = editing === 'new' ? undefined : (editing ?? undefined);
    return (
      <ProfileForm
        key={changing?.id ?? 'new'}
        editing={changing}
        onSave={(saved) => {
          saveProfile(saved);
          setEditing(undefined);
        }}
        onCancel={profile && (() => setEditing(undefined))}
        onRemove={
          changing &&
          (() => {
            removeProfile(changing.id);
            setEditing(undefined);
          })
        }
      />
    );
  }

  // One screen at a time: the cluster list, or the cluster tapped in it.
  if (phone) {
    const opened = visible.find((cluster) => cluster.pullRequests[0].url === selectedUrl);
    return (
      <Login value={login}>
        <View style={styles.phone}>
          {investigationView || opened ? (
            <>
              <Pressable accessibilityLabel="Back to clusters" style={styles.back} onPress={() => history.back()}>
                <ChevronLeft size={20} color="#8FA8F5" />
                <Text style={styles.backText}>Clusters</Text>
              </Pressable>
              <ScrollView contentContainerStyle={styles.phoneContent}>
                {investigationView || (
                  <ClusterView
                    cluster={opened!}
                    busy={busy}
                    onApprove={approve}
                    onDismiss={dismiss}
                    onRestart={restart}
                onRebase={rebase}
                    onInvestigate={investigate}
                    onMessage={(message) => messageCluster(opened!, message)}
                  />
                )}
              </ScrollView>
            </>
          ) : (
            <>
              <ProfileTitle profiles={profiles} profile={profile} onSelect={selectProfile} onAdd={() => setEditing('new')} />
              {filters}
              <ScrollView>
                {investigations.length > 0 && (
                  <View style={styles.phoneSection}>
                    <SectionTitle>Agents</SectionTitle>
                  </View>
                )}
                {investigations.map((i) => (
                  <PhoneInvestigationRow
                    key={i.id}
                    investigation={i}
                    onPress={() => {
                      history.pushState(null, '');
                      setSelectedInvestigation(i.id);
                    }}
                  />
                ))}
                {investigations.length > 0 && (
                  <View style={styles.phoneSection}>
                    <SectionTitle>Clusters</SectionTitle>
                  </View>
                )}
                {visible.map((cluster) => (
                  <PhoneClusterRow
                    key={cluster.pullRequests[0].url}
                    cluster={cluster}
                    onPress={() => {
                      history.pushState(null, '');
                      setSelectedUrl(cluster.pullRequests[0].url);
                    }}
                  />
                ))}
              </ScrollView>
            </>
          )}
        </View>
        <StatusBar style="auto" />
      </Login>
    );
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
          {investigations.length > 0 && <SectionTitle>Agents</SectionTitle>}
          {investigations.map((i) => (
            <InvestigationRow
              key={i.id}
              investigation={i}
              selected={i === investigation}
              onPress={() => setSelectedInvestigation(i.id)}
            />
          ))}
          {investigations.length > 0 && <SectionTitle>Clusters</SectionTitle>}
          {visible.map((cluster) => (
            <ClusterRow
              key={cluster.pullRequests[0].url}
              cluster={cluster}
              selected={!investigation && cluster === selected}
              onPress={() => {
                setSelectedInvestigation(undefined);
                setSelectedUrl(cluster.pullRequests[0].url);
              }}
            />
          ))}
        </ScrollView>
      </View>
      <Login value={login}>
        <ScrollView style={styles.main} contentContainerStyle={styles.mainContent}>
          {investigationView ||
            (selected && (
              <ClusterView
                cluster={selected}
                busy={busy}
                onApprove={approve}
                onDismiss={dismiss}
                onRestart={restart}
                onRebase={rebase}
                onInvestigate={investigate}
                onMessage={(message) => messageCluster(selected, message)}
              />
            ))}
        </ScrollView>
      </Login>
      <StatusBar style="auto" />
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

const styles = StyleSheet.create({
  phone: {
    flex: 1,
    backgroundColor: '#19212A',
  },
  back: {
    flexDirection: 'row',
    alignItems: 'center',
    height: 44,
    paddingHorizontal: 8,
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(255, 255, 255, 0.06)',
  },
  backText: {
    color: '#8FA8F5',
    fontSize: 16,
  },
  phoneContent: {
    padding: 12,
  },
  phoneSection: {
    paddingHorizontal: 8,
  },
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
  main: {
    flex: 1,
  },
  mainContent: {
    alignItems: 'center',
    paddingHorizontal: 24,
    paddingVertical: 32,
  },
});
