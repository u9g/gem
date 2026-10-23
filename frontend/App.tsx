import { create, fromJson, type JsonValue, toJson } from '@bufbuild/protobuf';
import { timestampDate } from '@bufbuild/protobuf/wkt';
import * as Select from '@rn-primitives/select';
import { StatusBar } from 'expo-status-bar';
import { useCallback, useEffect, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

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

const server = 'http://localhost:8080';

function post(path: string, body: JsonValue): Promise<JsonValue> {
  return fetch(`${server}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  }).then((res) => res.json());
}

function sameRepository(a: Repository | undefined, b: Repository | undefined): boolean {
  return a?.owner === b?.owner && a?.name === b?.name;
}

// Select options carry string values; this encodes a repository so it can be looked up again by
// comparing values, never parsed.
function repositoryValue(repository: Repository): string {
  return JSON.stringify([repository.owner, repository.name]);
}

// GitHub hides HTML comments, which bots use to tag their own comments.
function visibleMarkdown(markdown: string): string {
  return markdown.replace(/<!--[\s\S]*?-->/g, '').trim();
}

function pending(cluster: PullRequestCluster): number {
  return cluster.pullRequests.reduce((sum, pr) => sum + pr.proposals.length, 0);
}

export default function App() {
  const [clusters, setClusters] = useState<PullRequestCluster[]>([]);
  const [selectedUrl, setSelectedUrl] = useState<string>();
  const [repoFilter, setRepoFilter] = useState<Repository>();
  const [highlightedRepo, setHighlightedRepo] = useState<string>();
  // Proposal being approved or dismissed.
  const [busy, setBusy] = useState<string>();

  const load = useCallback(
    () =>
      post('/prs', toJson(ListOpenPullRequestsRequestSchema, create(ListOpenPullRequestsRequestSchema))).then(
        (json) => setClusters(fromJson(ListOpenPullRequestsResponseSchema, json).clusters),
      ),
    [],
  );

  // Proposals arrive whenever their agents finish, not only when the page loads.
  useEffect(() => {
    load();
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [load]);

  function approve(id: string) {
    setBusy(id);
    post('/proposals/approve', toJson(ApproveProposalRequestSchema, create(ApproveProposalRequestSchema, { id })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  function dismiss(id: string) {
    setBusy(id);
    post('/proposals/dismiss', toJson(DismissProposalRequestSchema, create(DismissProposalRequestSchema, { id })))
      .then(load)
      .finally(() => setBusy(undefined));
  }

  // A cluster is identified by its oldest PR, which stays put as the cluster grows.
  const selected = clusters.find((cluster) => cluster.pullRequests[0].url === selectedUrl);
  const proposals = (selected?.pullRequests ?? []).flatMap((pr) => pr.proposals.map((proposal) => ({ pr, proposal })));

  const repositories = clusters
    .flatMap((cluster) => cluster.pullRequests.flatMap((pr) => (pr.repository ? [pr.repository] : [])))
    .filter((repository, i, all) => all.findIndex((other) => sameRepository(other, repository)) === i)
    .sort((a, b) => a.name.localeCompare(b.name));
  const visible = repoFilter
    ? clusters.filter((cluster) => cluster.pullRequests.some((pr) => sameRepository(pr.repository, repoFilter)))
    : clusters;

  return (
    <View style={styles.container}>
      <ScrollView contentContainerStyle={styles.grid}>
        <View style={styles.header}>
          <View style={styles.filters}>
            {['org: livekit, livekit-examples', 'author: u9g'].map((filter) => (
              <Text key={filter} style={styles.filter}>
                {filter}
              </Text>
            ))}
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
        </View>
        {visible.map((cluster) => {
          const url = cluster.pullRequests[0].url;
          return (
            <View key={url} style={styles.cell}>
              <Pressable
                style={[styles.card, cluster === selected && styles.selectedCard]}
                onPress={() => setSelectedUrl(cluster === selected ? undefined : url)}
              >
                <View style={styles.cardHeader}>
                  {cluster.name !== '' && (
                    <Text
                      style={[
                        styles.name,
                        cluster.pullRequests.every((pr) => pr.approved && pr.checksPassing) && styles.ready,
                      ]}
                    >
                      {cluster.name}
                    </Text>
                  )}
                  {cluster.updatedAt && (
                    <Text style={styles.updated}>{timeAgo(timestampDate(cluster.updatedAt))}</Text>
                  )}
                  {pending(cluster) > 0 && <Text style={styles.pending}>{pending(cluster)} to approve</Text>}
                </View>
                <View style={styles.refs}>
                  {[...new Set(cluster.pullRequests.map((pr) => pr.repository?.name))].map((repository) => (
                    <Text
                      key={repository}
                      style={[
                        styles.repo,
                        cluster.pullRequests.some((pr) => pr.repository?.name === repository && pr.checksFailing) &&
                          styles.failing,
                      ]}
                    >
                      {repository}
                    </Text>
                  ))}
                </View>
              </Pressable>
            </View>
          );
        })}
      </ScrollView>
      {selected && (
        <View style={styles.sidebar}>
          <View style={styles.sidebarHeader}>
            <Text style={[styles.name, styles.sidebarTitle]}>{selected.name}</Text>
            <Pressable onPress={() => setSelectedUrl(undefined)}>
              <Text style={styles.close}>×</Text>
            </Pressable>
          </View>
          <ScrollView>
            {proposals.map(({ pr, proposal }) => (
              <View key={proposal.id} style={styles.proposal}>
                <Pressable onPress={() => proposal.comment && Linking.openURL(proposal.comment.url)}>
                  <Text style={styles.repo}>
                    {pr.repository?.name}#{pr.number} · {proposal.comment?.author}
                  </Text>
                  <Text style={styles.quote} numberOfLines={4}>
                    {visibleMarkdown(proposal.comment?.body ?? '')}
                  </Text>
                </Pressable>
                <Text style={styles.summary}>{proposal.summary}</Text>
                {proposal.patch && (
                  <>
                    <Text style={styles.step}>Commit: {proposal.patch.commitMessage}</Text>
                    <ScrollView style={styles.diff}>
                      <ScrollView horizontal>
                        <Text style={styles.code}>{proposal.patch.diff}</Text>
                      </ScrollView>
                    </ScrollView>
                  </>
                )}
                {proposal.reply !== '' && (
                  <>
                    <Text style={styles.step}>{proposal.threadId ? 'Reply in thread' : 'Quote reply'}</Text>
                    <Text style={styles.text}>{proposal.reply}</Text>
                  </>
                )}
                {proposal.resolveThread && <Text style={styles.step}>Resolve thread</Text>}
                {proposal.requestedTool !== '' && (
                  <>
                    <Text style={styles.step}>Requested tool</Text>
                    <Text style={styles.text}>{proposal.requestedTool}</Text>
                  </>
                )}
                {proposal.error !== '' && <Text style={styles.error}>{proposal.error}</Text>}
                <View style={styles.buttons}>
                  {proposal.requestedTool === '' && (
                    <Pressable
                      style={[styles.button, styles.approve]}
                      disabled={busy !== undefined}
                      onPress={() => approve(proposal.id)}
                    >
                      <Text style={styles.buttonText}>{busy === proposal.id ? 'Working…' : 'Approve'}</Text>
                    </Pressable>
                  )}
                  <Pressable style={styles.button} disabled={busy !== undefined} onPress={() => dismiss(proposal.id)}>
                    <Text style={styles.buttonText}>Dismiss</Text>
                  </Pressable>
                </View>
              </View>
            ))}
            {selected.pullRequests.map((pr, i) => (
              <Pressable
                key={pr.url}
                style={[styles.pr, i > 0 && styles.stacked]}
                onPress={() => Linking.openURL(pr.url)}
              >
                <Text style={styles.repo}>
                  {pr.repository?.owner}/{pr.repository?.name}#{pr.number}
                </Text>
                <Text style={styles.text}>{pr.title}</Text>
              </Pressable>
            ))}
          </ScrollView>
        </View>
      )}
      <StatusBar style="auto" />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#19212A',
  },
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    paddingHorizontal: '25%',
    paddingVertical: 24,
  },
  header: {
    width: '100%',
    padding: 8,
  },
  filters: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 8,
  },
  filter: {
    color: '#D0CFD4',
    fontSize: 12,
    paddingHorizontal: 8,
    paddingVertical: 4,
    borderRadius: 999,
    backgroundColor: '#2E3946',
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
  cell: {
    width: '33.333%',
    padding: 8,
  },
  card: {
    flex: 1,
    justifyContent: 'space-between',
    gap: 12,
    padding: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2E3946',
  },
  selectedCard: {
    borderColor: '#D0CFD4',
  },
  cardHeader: {
    gap: 2,
  },
  refs: {
    gap: 2,
  },
  sidebar: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    width: '25%',
    backgroundColor: '#19212A',
    borderLeftWidth: 1,
    borderLeftColor: '#2E3946',
  },
  sidebarHeader: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 12,
    padding: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#2E3946',
  },
  sidebarTitle: {
    flex: 1,
  },
  close: {
    color: '#D0CFD4',
    fontSize: 20,
    lineHeight: 20,
  },
  name: {
    color: '#D0CFD4',
    fontSize: 16,
    fontWeight: '600',
  },
  ready: {
    color: '#4ADE80',
  },
  failing: {
    color: '#F87171',
  },
  updated: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
  },
  pending: {
    color: '#D0CFD4',
    fontSize: 12,
    fontWeight: '600',
  },
  proposal: {
    gap: 6,
    padding: 12,
    borderBottomWidth: 1,
    borderBottomColor: '#2E3946',
  },
  quote: {
    color: '#D0CFD4',
    opacity: 0.6,
    paddingLeft: 8,
    borderLeftWidth: 2,
    borderLeftColor: '#2E3946',
  },
  summary: {
    color: '#D0CFD4',
    fontWeight: '600',
  },
  step: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
    marginTop: 4,
  },
  diff: {
    maxHeight: 240,
    padding: 8,
    borderRadius: 4,
    backgroundColor: '#11171E',
  },
  code: {
    color: '#D0CFD4',
    fontFamily: 'monospace',
    fontSize: 12,
  },
  error: {
    color: '#F87171',
    fontSize: 12,
  },
  buttons: {
    flexDirection: 'row',
    gap: 8,
    marginTop: 4,
  },
  button: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 6,
    backgroundColor: '#2E3946',
  },
  approve: {
    backgroundColor: '#2F6B45',
  },
  buttonText: {
    color: '#D0CFD4',
    fontWeight: '600',
  },
  pr: {
    padding: 12,
  },
  stacked: {
    borderTopWidth: 1,
    borderTopColor: '#2E3946',
  },
  repo: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
  },
  text: {
    color: '#D0CFD4',
    marginTop: 4,
  },
});
