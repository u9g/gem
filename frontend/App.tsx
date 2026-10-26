import { create, fromJson, type JsonValue, toJson } from '@bufbuild/protobuf';
import * as Select from '@rn-primitives/select';
import { StatusBar } from 'expo-status-bar';
import { ChevronLeft } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Board } from './Board';
import { ClusterReview } from './ClusterFiles';
import {
  Legend,
  PhoneClusterRow,
  PhoneInvestigationRow,
  section,
  sections,
  SectionTitle,
} from './ClusterRow';
import { ClusterView, timeAgo } from './ClusterView';
import { Composer, InvestigationView } from './InvestigationView';
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
  MergeClusterRequestSchema,
  MessageClusterRequestSchema,
  MessageClusterResponseSchema,
  MessageInvestigationRequestSchema,
  RebasePullRequestRequestSchema,
  RestartClusterRequestSchema,
  type PullRequestCluster,
  type Repository,
} from './gen/gem_pb';

function sameRepository(a: Repository | undefined, b: Repository | undefined): boolean {
  return a?.owner === b?.owner && a?.name === b?.name;
}

// Select options carry string values; this encodes a repository so it can be looked up again by
// comparing values, never parsed.
function repositoryValue(repository: Repository): string {
  return JSON.stringify([repository.owner, repository.name]);
}

// What is open lives in the page's query, so the browser's back and forward move between what was
// opened and a link reopens it. An investigation is shown in place of the cluster while both are set.
type Place = { cluster?: string; investigation?: string };

const GITHUB = 'https://github.com/';

// A cluster on github.com is written as its oldest PR's owner/repo/number, which needs no escaping
// in a query; one on any other host is written as its whole escaped URL.
function currentPlace(): Place {
  const params = new URLSearchParams(location.search);
  const cluster = params.get('cluster') ?? undefined;
  const [owner, repo, number] = cluster?.split('/') ?? [];
  return {
    cluster: cluster?.includes('://') ? cluster : number && `${GITHUB}${owner}/${repo}/pull/${number}`,
    investigation: params.get('investigation') ?? undefined,
  };
}

function placeSearch(place: Place): string {
  const cluster = place.cluster?.startsWith(GITHUB)
    ? place.cluster.slice(GITHUB.length).replace('/pull/', '/')
    : place.cluster && encodeURIComponent(place.cluster);
  const params = [
    cluster && `cluster=${cluster}`,
    place.investigation && `investigation=${place.investigation}`,
  ].filter(Boolean);
  return params.length > 0 ? `?${params.join('&')}` : '';
}

export default function App() {
  const { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post, snapshot } =
    useServer();
  // The profile being changed, or 'new' while one is being added.
  const [editing, setEditing] = useState<Profile | 'new'>();
  const [clusters, setClusters] = useState<PullRequestCluster[]>([]);
  const [investigations, setInvestigations] = useState<Investigation[]>([]);
  const [login, setLogin] = useState('');
  const [place, setPlace] = useState(currentPlace);
  const [repoFilter, setRepoFilter] = useState<Repository>();
  const [highlightedRepo, setHighlightedRepo] = useState<string>();
  // Each request in flight: proposals being approved, dismissed or investigated, the oldest PR's URL
  // of clusters being restarted, `merge` and that URL for ones whose merge is starting, the URLs of
  // PRs whose rebase is starting, and investigations being closed.
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const phone = usePhone();

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


  // The server sends a snapshot whenever this frontend or the server joins, and on every change.
  useEffect(() => {
    if (snapshot) show(snapshot);
  }, [snapshot, show]);

  // `replace` overwrites the current entry, for a place that can no longer be returned to.
  const go = useCallback((to: Place, replace = false) => {
    const search = placeSearch(to);
    if (search !== location.search) history[replace ? 'replaceState' : 'pushState'](null, '', search || location.pathname);
    setPlace(to);
  }, []);

  const closeDrawer = useCallback(() => go({}), [go]);

  // Another profile is another server: nothing shown belongs to it. The place a page was loaded
  // with is kept, since it was opened for the profile selected then.
  const profileId = profile?.id;
  const shownProfile = useRef(profileId);
  useEffect(() => {
    setClusters([]);
    setInvestigations([]);
    setLogin('');
    setRepoFilter(undefined);
    if (shownProfile.current !== profileId) go({}, true);
    shownProfile.current = profileId;
  }, [profileId, go]);

  useEffect(() => {
    const restore = () => setPlace(currentPlace());
    addEventListener('popstate', restore);
    return () => removeEventListener('popstate', restore);
  }, []);

  // Marks `key` busy until the request and the reload after it settle.
  function track(key: string, request: Promise<unknown>) {
    setBusy((keys) => new Set(keys).add(key));
    request
      .then(load)
      .finally(() =>
        setBusy((keys) => {
          const rest = new Set(keys);
          rest.delete(key);
          return rest;
        }),
      );
  }

  function approve(id: string, reply: string, followUp: string, diff: string) {
    track(
      id,
      post(
        '/proposals/approve',
        toJson(ApproveProposalRequestSchema, create(ApproveProposalRequestSchema, { id, reply, followUp, diff })),
      ),
    );
  }

  function dismiss(id: string) {
    track(id, post('/proposals/dismiss', toJson(DismissProposalRequestSchema, create(DismissProposalRequestSchema, { id }))));
  }

  function restart(cluster: PullRequestCluster) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    track(
      cluster.pullRequests[0].url,
      post('/clusters/restart', toJson(RestartClusterRequestSchema, create(RestartClusterRequestSchema, { pullRequestUrls }))),
    );
  }

  // Starts an agent merging the cluster's PRs, without opening its thread.
  function merge(cluster: PullRequestCluster, admin: boolean) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    track(
      `merge ${cluster.pullRequests[0].url}`,
      post('/clusters/merge', toJson(MergeClusterRequestSchema, create(MergeClusterRequestSchema, { pullRequestUrls, admin }))),
    );
  }

  // Starts an agent rebasing the PR, without opening its thread.
  function rebase(url: string) {
    track(url, post('/prs/rebase', toJson(RebasePullRequestRequestSchema, create(RebasePullRequestRequestSchema, { url }))));
  }

  function investigate(proposalId: string) {
    track(
      proposalId,
      post('/investigations/start', toJson(InvestigateRequestSchema, create(InvestigateRequestSchema, { proposalId }))),
    );
  }

  // Starts an agent on the message, then opens it, where the conversation goes on.
  function messageCluster(cluster: PullRequestCluster, message: string) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    return post(
      '/clusters/message',
      toJson(MessageClusterRequestSchema, create(MessageClusterRequestSchema, { pullRequestUrls, message })),
    )
      .then((json) => fromJson(MessageClusterResponseSchema, json).investigationId)
      .then((id) => load().then(() => go({ cluster: place.cluster, investigation: id })));
  }

  function messageInvestigation(id: string, message: string) {
    return post(
      '/investigations/message',
      toJson(MessageInvestigationRequestSchema, create(MessageInvestigationRequestSchema, { id, message })),
    ).then(load);
  }

  function closeInvestigation(id: string) {
    track(
      id,
      post('/investigations/close', toJson(CloseInvestigationRequestSchema, create(CloseInvestigationRequestSchema, { id }))).then(
        () => {
          // The investigation is gone, so its entry is replaced, unless something else was opened meanwhile.
          const now = currentPlace();
          if (now.investigation === id) go({ cluster: now.cluster }, true);
        },
      ),
    );
  }

  const investigation = investigations.find((i) => i.id === place.investigation);
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
  // A cluster is identified by its oldest PR, which stays put as the cluster grows.
  const opened = clusters.find((cluster) => cluster.pullRequests[0].url === place.cluster);

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

  // One screen at a time: the cluster list, by what each cluster waits on, or the cluster tapped in
  // it, with the box to ask about it under the scrolling content.
  if (phone) {
    const listed = sections
      .map((name) => ({ name, clusters: visible.filter((cluster) => section(cluster) === name) }))
      .filter(({ clusters }) => clusters.length > 0);
    return (
      <Login value={login}>
        <View style={styles.phone}>
          {investigationView || opened ? (
            <>
              <Pressable accessibilityLabel="Back to clusters" style={styles.back} onPress={closeDrawer}>
                <ChevronLeft size={22} color="#8FA8F5" />
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
                    onMerge={merge}
                    onInvestigate={investigate}
                  />
                )}
              </ScrollView>
              {!investigationView && (
                <View style={styles.dock}>
                  <Composer placeholder="Ask an agent about this cluster…" onSend={(message) => messageCluster(opened!, message)} />
                </View>
              )}
            </>
          ) : (
            <>
              <ProfileTitle profiles={profiles} profile={profile} onSelect={selectProfile} onAdd={() => setEditing('new')} />
              {filters}
              {/* The legend is the third child: after the investigations and the first section's title. */}
              <ScrollView contentContainerStyle={styles.phoneList} stickyHeaderIndices={[2]}>
                <View>
                  {investigations.length > 0 && (
                    <View style={styles.phoneSection}>
                      <SectionTitle>Investigations</SectionTitle>
                    </View>
                  )}
                  {investigations.map((i) => (
                    <PhoneInvestigationRow
                      key={i.id}
                      investigation={i}
                      onPress={() => go({ investigation: i.id })}
                    />
                  ))}
                </View>
                {listed.flatMap(({ name, clusters }, i) => [
                  <View key={name} style={styles.phoneSection}>
                    <SectionTitle>{name}</SectionTitle>
                  </View>,
                  ...(i === 0 ? [<Legend key="legend" />] : []),
                  ...clusters.map((cluster) => (
                    <PhoneClusterRow key={cluster.pullRequests[0].url} cluster={cluster} onPress={() => go({ cluster: cluster.pullRequests[0].url })} />
                  )),
                ])}
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
      <View style={styles.topBar}>{filters}</View>
      <Login value={login}>
        <Board
          clusters={visible}
          investigations={investigations}
          openUrl={investigation ? undefined : opened?.pullRequests[0].url}
          openInvestigation={investigation?.id}
          onOpenCluster={(cluster) => go({ cluster: cluster.pullRequests[0].url })}
          onOpenInvestigation={(id) => go({ cluster: place.cluster, investigation: id })}
          onClose={closeDrawer}
          wide={!investigationView && opened !== undefined}
        >
          {investigationView ||
            (opened && (
              <ClusterReview
                cluster={opened}
                busy={busy}
                onApprove={approve}
                onDismiss={dismiss}
                onRestart={restart}
                onRebase={rebase}
                onMerge={merge}
                onInvestigate={investigate}
                onMessage={(message) => messageCluster(opened, message)}
              />
            ))}
        </Board>
      </Login>
      <StatusBar style="auto" />
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
    paddingBottom: 24,
  },
  phoneList: {
    paddingBottom: 24,
  },
  dock: {
    paddingHorizontal: 12,
    paddingBottom: 12,
    borderTopWidth: 1,
    borderTopColor: 'rgba(255, 255, 255, 0.06)',
  },
  phoneSection: {
    paddingHorizontal: 8,
    paddingTop: 8,
  },
  container: {
    flex: 1,
    backgroundColor: '#19212A',
  },
  topBar: {
    borderBottomWidth: 1,
    borderBottomColor: 'rgba(255, 255, 255, 0.06)',
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
});
