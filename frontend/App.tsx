import { create, fromJson, toJson } from '@bufbuild/protobuf';
import { StatusBar } from 'expo-status-bar';
import { ChevronLeft } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { ClusterReview } from './ClusterFiles';
import { AgentView, Composer } from './AgentView';
import { ClusterView, timeAgo } from './ClusterView';
import { DbContext, emptyDb, useDb, usePending } from './db';
import { Inbox } from './Inbox';
import { ProfileChip, ProfileForm, ProfileTitle } from './Profiles';
import { SearchBox, useSearch } from './Search';
import { failureMessage, type Profile, useServer } from './server';
import { shown } from './view';
import type { Cluster } from './view-types';
import { type Failure, FailureBanner, Login, RenderMarkdown } from './surfaces';
import { usePhone } from './t3';
import {
  ApproveProposalRequestSchema,
  CloseAgentRequestSchema,
  CloseClusterRequestSchema,
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
  const { clusters, agents, login, search: searched } = useDb(shown);
  // The profile being changed, or 'new' while one is being added.
  const [editing, setEditing] = useState<Profile | 'new'>();
  const [place, setPlace] = useState(currentPlace);
  // Each action in progress, from any frontend, as the view holds it: proposals being approved,
  // dismissed or investigated, the oldest PR's URL of clusters being restarted, `merge` and that
  // URL for ones whose merge is starting, `close` and that URL for ones being closed, `rebase` and
  // that URL for clusters whose rebase is starting, the URLs of PRs whose rebase is starting, and agents being closed. The server marks
  // each pending before it runs and refuses a second of the same, so they're greyed out
  // everywhere at once.
  const busy = usePending();
  // The last action that failed and why, until it's dismissed or another fails. What an action
  // does shows in the view, so one that failed would otherwise look like it did nothing. Approve
  // answers once it's done, and shows how it failed on the proposal, so it fails here only when
  // the server didn't answer.
  const [failure, setFailure] = useState<Failure>();
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
    setFailure(undefined);
    if (shownProfile.current && shownProfile.current !== profileId) go({}, true);
    shownProfile.current = profileId;
  }, [profileId, go]);

  useEffect(() => {
    const restore = () => setPlace(currentPlace());
    addEventListener('popstate', restore);
    return () => removeEventListener('popstate', restore);
  }, []);

  // Shows why `request` fails, if it does. It's returned for callers that wait on it.
  function act<T>(action: string, request: Promise<T>): Promise<T> {
    request.catch((error: unknown) => setFailure({ action, message: failureMessage(error) }));
    return request;
  }

  function approve(id: string, reply: string, followUp: string, diff: string) {
    void act(
      'approve',
      post(
        '/proposals/approve',
        toJson(ApproveProposalRequestSchema, create(ApproveProposalRequestSchema, { id, reply, followUp, diff })),
      ),
    );
  }

  function dismiss(id: string) {
    void act('dismiss', post('/proposals/dismiss', toJson(DismissProposalRequestSchema, create(DismissProposalRequestSchema, { id }))));
  }

  function restart(cluster: Cluster) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    void act('restart', post('/clusters/restart', toJson(RestartClusterRequestSchema, create(RestartClusterRequestSchema, { pullRequestUrls }))));
  }

  // Starts an agent merging the cluster's PRs, without opening its thread.
  function merge(cluster: Cluster, admin: boolean) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    void act('merge', post('/clusters/merge', toJson(MergeClusterRequestSchema, create(MergeClusterRequestSchema, { pullRequestUrls, admin }))));
  }

  // Closes every PR of the cluster without merging it.
  function closeCluster(cluster: Cluster) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    void act('close', post('/clusters/close', toJson(CloseClusterRequestSchema, create(CloseClusterRequestSchema, { pullRequestUrls }))));
  }

  // Starts an agent rebasing the PR, without opening its thread.
  function rebase(url: string) {
    void act('rebase', post('/prs/rebase', toJson(RebasePullRequestRequestSchema, create(RebasePullRequestRequestSchema, { url }))));
  }

  // Starts one agent rebasing every PR of the cluster, without opening its thread.
  function rebaseCluster(cluster: Cluster) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    void act('rebase', post('/clusters/rebase', toJson(RebaseClusterRequestSchema, create(RebaseClusterRequestSchema, { pullRequestUrls }))));
  }

  function investigate(proposalId: string) {
    void act('investigate', post('/investigations/start', toJson(InvestigateRequestSchema, create(InvestigateRequestSchema, { proposalId }))));
  }

  // Starts an agent on the message, then opens it, where the conversation goes on.
  function messageCluster(cluster: Cluster, message: string) {
    const pullRequestUrls = cluster.pullRequests.map((pr) => pr.url);
    return act(
      'message',
      post(
        '/clusters/message',
        toJson(MessageClusterRequestSchema, create(MessageClusterRequestSchema, { pullRequestUrls, message })),
      )
        .then((json) => fromJson(MessageClusterResponseSchema, json).agentId)
        .then((id) => go({ cluster: place.cluster, agent: id })),
    );
  }

  function messageAgent(id: string, message: string) {
    return act(
      'message',
      post('/agents/message', toJson(MessageAgentRequestSchema, create(MessageAgentRequestSchema, { id, message }))).then(
        () => {},
      ),
    );
  }

  function closeAgent(id: string) {
    void act(
      'close agent',
      post('/agents/close', toJson(CloseAgentRequestSchema, create(CloseAgentRequestSchema, { id }))).then(() => {
        // The agent is gone, so its entry is not kept. Nothing moves if something else was opened
        // meanwhile.
        const now = currentPlace();
        if (now.agent === id) go({ cluster: now.cluster }, true);
      }),
    );
  }

  const openAgent = (id: string) => go({ cluster: place.cluster, agent: id });
  const agent = [...agents, ...clusters.flatMap((cluster) => cluster.agents)].find(
    (a) => a.id === place.agent,
  );
  const agentView = agent && (
    <AgentView
      agent={agent}
      ago={timeAgo}
      busy={busy}
      onApprove={approve}
      onDismiss={dismiss}
      onMessage={(message) => messageAgent(agent.id, message)}
      onClose={() => closeAgent(agent.id)}
    />
  );

  const visible = clusters
    .filter((cluster) => !search.matches || cluster.pullRequests.some((pr) => search.matches!.has(pr.url)));
  // A cluster is linked to by its oldest PR, and any of its PRs opens it, so a link still opens it
  // after an agent joins it to an older cluster.
  const opened = clusters.find((cluster) => cluster.pullRequests.some((pr) => pr.url === place.cluster));
  const failed = failure && <FailureBanner failure={failure} onDismiss={() => setFailure(undefined)} />;

  const filters = (
    <View style={styles.filters}>
      <Pressable accessibilityLabel="Change profile" onPress={() => setEditing(profile)}>
        <Text style={styles.filter}>server: {status}</Text>
      </Pressable>
      {!phone && profile && (
        <ProfileChip profiles={profiles} profile={profile} onSelect={selectProfile} onAdd={() => setEditing('new')} />
      )}
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
    return (
      <Contexts login={login} render={render}>
        <View style={styles.phone}>
          {failed}
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
                    onCloseCluster={closeCluster}
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
              <Inbox
                phone
                clusters={visible}
                agents={agents}
                toolbar={filters}
                onOpenCluster={openCluster}
                onOpenAgent={openAgentAlone}
                onClose={closeDrawer}
              />
            </>
          )}
        </View>
        <StatusBar style="auto" />
</Contexts>
    );
  }

  return (
    <View style={styles.container}>
      {failed}
      <Contexts login={login} render={render}>
        <Inbox
          clusters={visible}
          agents={agents}
          toolbar={filters}
          openKey={agent ? `agent:${agent.id}` : opened?.pullRequests[0].url}
          onOpenCluster={openCluster}
          onOpenAgent={openAgentAlone}
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
                onCloseCluster={closeCluster}
                onInvestigate={investigate}
                onMessage={(message) => messageCluster(opened, message)}
              />
            ))}
        </Inbox>
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
  dock: {
    paddingHorizontal: 12,
    paddingBottom: 12,
    borderTopWidth: 1,
    borderTopColor: 'rgba(255, 255, 255, 0.06)',
  },
  container: {
    flex: 1,
    backgroundColor: '#19212A',
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
});
