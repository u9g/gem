import { create, fromJson, toJson } from '@bufbuild/protobuf';
import * as Select from '@rn-primitives/select';
import { StatusBar } from 'expo-status-bar';
import { ChevronLeft } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Board } from './Board';
import { ClusterReview } from './ClusterFiles';
import { AgentView, Composer } from './AgentView';
import { Legend, PhoneAgentRow, PhoneClusterRow, section, sections, SectionTitle } from './ClusterRow';
import { ClusterView, timeAgo } from './ClusterView';
import { DbContext, emptyDb, type Node, useDb } from './db';
import { ProfileChip, ProfileForm, ProfileTitle } from './Profiles';
import { SearchBox, useSearch } from './Search';
import { type Profile, useServer } from './server';
import { type Cluster, clusterKey, type PullRequest, view } from './shapes';
import { Login, RenderMarkdown } from './surfaces';
import { usePhone } from './t3';
import {
  ApproveProposalRequestSchema,
  CloseAgentRequestSchema,
  DismissProposalRequestSchema,
  InvestigateRequestSchema,
  MergeClusterRequestSchema,
  MessageClusterRequestSchema,
  MessageAgentRequestSchema,
  MessageClusterResponseSchema,
  RebaseClusterRequestSchema,
  RebasePullRequestRequestSchema,
  RenderMarkdownRequestSchema,
  RenderMarkdownResponseSchema,
  RestartClusterRequestSchema,
} from './gen/gem_pb';

function Contexts({
  login,
  render,
  children,
}: {
  login: string;
  render: (pr: string, markdown: string) => Promise<string>;
  children: ReactNode;
}) {
  return (
    <Login value={login}>
      <RenderMarkdown value={render}>{children}</RenderMarkdown>
    </Login>
  );
}

type Repository = { owner: string; name: string };

function repositoryOf(pr: Node<PullRequest>): Repository | undefined {
  const repository = pr.node('repository');
  return repository && { owner: repository.str('owner'), name: repository.str('name') };
}

// Every pull request's URL, which actions name the cluster by.
function urlsOf(cluster: Node<Cluster>): string[] {
  return cluster.list('pullRequests').map((pr) => pr.str('url'));
}

function sameRepository(a: Repository | undefined, b: Repository | undefined): boolean {
  return a?.owner === b?.owner && a?.name === b?.name;
}

// Select options carry string values; this encodes a repository so it can be looked up again by
// comparing values, never parsed.
function repositoryValue(repository: Repository): string {
  return JSON.stringify([repository.owner, repository.name]);
}

// What is open lives in the page's query, so the browser's back and forward move between what was
// opened and a link reopens it. An agent is shown in place of the cluster while both are set.
type Place = { cluster?: string; agent?: string };

const GITHUB = 'https://github.com/';

// A cluster on github.com is written as its oldest PR's owner/repo/number, which needs no escaping
// in a query; one on any other host is written as its whole escaped URL.
function currentPlace(): Place {
  const params = new URLSearchParams(location.search);
  const cluster = params.get('cluster') ?? undefined;
  const [owner, repo, number] = cluster?.split('/') ?? [];
  return {
    cluster: cluster?.includes('://') ? cluster : number && `${GITHUB}${owner}/${repo}/pull/${number}`,
    // Links from before every agent was one still open their investigation.
    agent: params.get('agent') ?? params.get('investigation') ?? undefined,
  };
}

function placeSearch(place: Place): string {
  const cluster = place.cluster?.startsWith(GITHUB)
    ? place.cluster.slice(GITHUB.length).replace('/pull/', '/')
    : place.cluster && encodeURIComponent(place.cluster);
  const params = [
    cluster && `cluster=${cluster}`,
    place.agent && `agent=${encodeURIComponent(place.agent)}`,
  ].filter(Boolean);
  return params.length > 0 ? `?${params.join('&')}` : '';
}

// Components read the server's view from this connection's replica of its database.
export default function App() {
  const server = useServer();
  return (
    <DbContext value={server.db ?? emptyDb}>
      <Main server={server} />
    </DbContext>
  );
}

function Main({ server }: { server: ReturnType<typeof useServer> }) {
  const { status, profiles, profile, selectProfile, saveProfile, removeProfile, reported, post } = server;
  // What the replica holds now: the clusters, most recently updated first, and the agents acting
  // on no open pull request. App renders again when a cluster changes; each card only when its own
  // cluster did.
  const { clusters, agents, login, search: searched } = useDb(view);
  // The profile being changed, or 'new' while one is being added.
  const [editing, setEditing] = useState<Profile | 'new'>();
  const [place, setPlace] = useState(currentPlace);
  const [repoFilter, setRepoFilter] = useState<Repository>();
  const [highlightedRepo, setHighlightedRepo] = useState<string>();
  // Each request in flight: proposals being approved, dismissed or investigated, the oldest PR's URL
  // of clusters being restarted, `merge` and that URL for ones whose merge is starting, `rebase` and
  // that URL for clusters whose rebase is starting, the URLs of PRs whose rebase is starting, and
  // agents being closed.
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const phone = usePhone();
  const search = useSearch(post);
  const render = useCallback(
    (pullRequestUrl: string, markdown: string) =>
      post(
        '/markdown/render',
        toJson(RenderMarkdownRequestSchema, create(RenderMarkdownRequestSchema, { pullRequestUrl, markdown })),
      ).then((json) => fromJson(RenderMarkdownResponseSchema, json).html),
    [post],
  );

  // Labels the profile with what its server last reported.
  useEffect(() => {
    if (login) reported(login, searched);
  }, [login, searched, reported]);

  // A pushed entry's state is the query of the entry it was pushed from, so going to that place goes
  // back to it instead of stacking a copy. `replace` overwrites the current entry otherwise, for a
  // place that can no longer be returned to.
  const go = useCallback((to: Place, replace = false) => {
    const search = placeSearch(to);
    if (search === location.search) return setPlace(to);
    if (history.state === search) return history.back();
    if (replace) history.replaceState(history.state, '', search || location.pathname);
    else history.pushState(location.search, '', search || location.pathname);
    setPlace(to);
  }, []);

  const closeDrawer = useCallback(() => go({}), [go]);
  const openCluster = useCallback((key: string) => go({ cluster: key }), [go]);
  const openAgentAlone = useCallback((id: string) => go({ agent: id }), [go]);

  // Another profile is another server: nothing shown belongs to it. The place a page was loaded
  // with is kept, since it was opened for the profile selected then.
  const profileId = profile?.id;
  const shownProfile = useRef(profileId);
  useEffect(() => {
    setRepoFilter(undefined);
    if (shownProfile.current && shownProfile.current !== profileId) go({}, true);
    shownProfile.current = profileId;
  }, [profileId, go]);

  useEffect(() => {
    const restore = () => setPlace(currentPlace());
    addEventListener('popstate', restore);
    return () => removeEventListener('popstate', restore);
  }, []);

  // Marks `key` busy until the request settles. The server publishes the view as it answers, so
  // the change, or for a refused request, like one on a cluster whose pull requests changed, the
  // view as it is now, reaches every frontend with the answer.
  function track(key: string, request: Promise<unknown>) {
    setBusy((keys) => new Set(keys).add(key));
    request
      .catch(() => {})
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

  function restart(cluster: Node<Cluster>) {
    const pullRequestUrls = urlsOf(cluster);
    track(
      clusterKey(cluster),
      post('/clusters/restart', toJson(RestartClusterRequestSchema, create(RestartClusterRequestSchema, { pullRequestUrls }))),
    );
  }

  // Starts an agent merging the cluster's PRs, without opening its thread.
  function merge(cluster: Node<Cluster>, admin: boolean) {
    const pullRequestUrls = urlsOf(cluster);
    track(
      `merge ${clusterKey(cluster)}`,
      post('/clusters/merge', toJson(MergeClusterRequestSchema, create(MergeClusterRequestSchema, { pullRequestUrls, admin }))),
    );
  }

  // Starts an agent rebasing the PR, without opening its thread.
  function rebase(url: string) {
    track(url, post('/prs/rebase', toJson(RebasePullRequestRequestSchema, create(RebasePullRequestRequestSchema, { url }))));
  }

  // Starts one agent rebasing every PR of the cluster, without opening its thread.
  function rebaseCluster(cluster: Node<Cluster>) {
    const pullRequestUrls = urlsOf(cluster);
    track(
      `rebase ${clusterKey(cluster)}`,
      post('/clusters/rebase', toJson(RebaseClusterRequestSchema, create(RebaseClusterRequestSchema, { pullRequestUrls }))),
    );
  }

  function investigate(proposalId: string) {
    track(
      proposalId,
      post('/investigations/start', toJson(InvestigateRequestSchema, create(InvestigateRequestSchema, { proposalId }))),
    );
  }

  // Starts an agent on the message, then opens it, where the conversation goes on.
  function messageCluster(cluster: Node<Cluster>, message: string) {
    const pullRequestUrls = urlsOf(cluster);
    return post(
      '/clusters/message',
      toJson(MessageClusterRequestSchema, create(MessageClusterRequestSchema, { pullRequestUrls, message })),
    )
      .then((json) => fromJson(MessageClusterResponseSchema, json).agentId)
      .then((id) => go({ cluster: place.cluster, agent: id }));
  }

  function messageAgent(id: string, message: string) {
    return post('/agents/message', toJson(MessageAgentRequestSchema, create(MessageAgentRequestSchema, { id, message }))).then(
      () => {},
    );
  }

  function closeAgent(id: string) {
    track(
      id,
      post('/agents/close', toJson(CloseAgentRequestSchema, create(CloseAgentRequestSchema, { id }))).then(() => {
        // The agent is gone, so its entry is not kept. Nothing moves if something else was opened
        // meanwhile.
        const now = currentPlace();
        if (now.agent === id) go({ cluster: now.cluster }, true);
      }),
    );
  }

  const openAgent = (id: string) => go({ cluster: place.cluster, agent: id });
  const agent = [...agents, ...clusters.flatMap((cluster) => cluster.list('agents'))].find(
    (a) => a.str('id') === place.agent,
  );
  const agentView = agent && (
    <AgentView
      agent={agent}
      ago={timeAgo}
      busy={busy}
      onApprove={approve}
      onDismiss={dismiss}
      onMessage={(message) => messageAgent(agent.str('id'), message)}
      onClose={() => closeAgent(agent.str('id'))}
    />
  );

  const repositories = clusters
    .flatMap((cluster) => cluster.list('pullRequests').flatMap((pr) => repositoryOf(pr) ?? []))
    .filter((repository, i, all) => all.findIndex((other) => sameRepository(other, repository)) === i)
    .sort((a, b) => a.name.localeCompare(b.name));
  const visible = clusters
    .filter((cluster) => !repoFilter || cluster.list('pullRequests').some((pr) => sameRepository(repositoryOf(pr), repoFilter)))
    .filter((cluster) => !search.matches || urlsOf(cluster).some((url) => search.matches!.has(url)));
  // A cluster is linked to by its oldest PR, and any of its PRs opens it, so a link still opens it
  // after an agent joins it to an older cluster.
  const opened = clusters.find((cluster) => urlsOf(cluster).includes(place.cluster ?? ''));

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
      <SearchBox
        value={search.query}
        onChange={search.setQuery}
        status={search.status}
        matched={visible.length}
        error={search.error}
      />
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
      <Contexts login={login} render={render}>
        <View style={styles.phone}>
          {agentView || opened ? (
            <>
              <Pressable accessibilityLabel="Back to clusters" style={styles.back} onPress={closeDrawer}>
                <ChevronLeft size={22} color="#8FA8F5" />
                <Text style={styles.backText}>Clusters</Text>
              </Pressable>
              <ScrollView contentContainerStyle={styles.phoneContent}>
                {agentView || (
                  <ClusterView
                    cluster={opened!}
                    onOpenAgent={openAgent}
                    busy={busy}
                    onApprove={approve}
                    onDismiss={dismiss}
                    onRestart={restart}
                    onRebase={rebase}
                    onRebaseCluster={rebaseCluster}
                    onMerge={merge}
                    onInvestigate={investigate}
                  />
                )}
              </ScrollView>
              {!agentView && (
                <View style={styles.dock}>
                  <Composer placeholder="Ask an agent about this cluster…" onSend={(message) => messageCluster(opened!, message)} />
                </View>
              )}
            </>
          ) : (
            <>
              <ProfileTitle profiles={profiles} profile={profile} onSelect={selectProfile} onAdd={() => setEditing('new')} />
              {filters}
              {/* The legend is the third child: after the agents and the first section's title. */}
              <ScrollView contentContainerStyle={styles.phoneList} stickyHeaderIndices={[2]}>
                <View>
                  {agents.length > 0 && (
                    <View style={styles.phoneSection}>
                      <SectionTitle>Agents</SectionTitle>
                    </View>
                  )}
                  {agents.map((a) => (
                    <PhoneAgentRow key={a.str('id')} agent={a} onPress={openAgentAlone} />
                  ))}
                </View>
                {listed.flatMap(({ name, clusters }, i) => [
                  <View key={name} style={styles.phoneSection}>
                    <SectionTitle>{name}</SectionTitle>
                  </View>,
                  ...(i === 0 ? [<Legend key="legend" />] : []),
                  ...clusters.map((cluster) => (
                    <PhoneClusterRow key={clusterKey(cluster)} cluster={cluster} onPress={openCluster} />
                  )),
                ])}
              </ScrollView>
            </>
          )}
        </View>
        <StatusBar style="auto" />
</Contexts>
    );
  }

  return (
    <View style={styles.container}>
      <View style={styles.topBar}>{filters}</View>
      <Contexts login={login} render={render}>
        <Board
          clusters={visible}
          agents={agents}
          openUrl={agent || !opened ? undefined : clusterKey(opened)}
          openAgent={agent?.str('id')}
          onOpenCluster={openCluster}
          onOpenAgent={openAgent}
          onClose={closeDrawer}
          wide={!agentView && opened !== undefined}
        >
          {agentView ||
            (opened && (
              <ClusterReview
                cluster={opened}
                onOpenAgent={openAgent}
                busy={busy}
                onApprove={approve}
                onDismiss={dismiss}
                onRestart={restart}
                onRebase={rebase}
                onRebaseCluster={rebaseCluster}
                onMerge={merge}
                onInvestigate={investigate}
                onMessage={(message) => messageCluster(opened, message)}
              />
            ))}
        </Board>
</Contexts>
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
