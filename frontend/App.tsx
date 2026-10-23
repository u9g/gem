import { create, fromJson, type JsonValue, toJson } from '@bufbuild/protobuf';
import { timestampDate } from '@bufbuild/protobuf/wkt';
import * as Select from '@rn-primitives/select';
import { StatusBar } from 'expo-status-bar';
import { useCallback, useEffect, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { ProposalView } from './ProposalView';
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

  function approve(id: string, reply: string) {
    setBusy(id);
    post('/proposals/approve', toJson(ApproveProposalRequestSchema, create(ApproveProposalRequestSchema, { id, reply })))
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
  const proposals = (selected?.pullRequests ?? []).flatMap((pr) => pr.proposals);

  return (
    <View style={styles.container}>
      <View style={styles.sidebar}>
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
      <ScrollView style={styles.main} contentContainerStyle={styles.mainContent}>
        {selected && (
          <View style={styles.column}>
            <View style={styles.titleBlock}>
              <Text style={[styles.title, ready(selected) && styles.ready]}>{selected.name}</Text>
              <Text style={styles.meta}>
                {selected.updatedAt && timeAgo(timestampDate(selected.updatedAt))}
                {[...new Set(selected.pullRequests.map((pr) => pr.repository?.name))].map((repository) => (
                  <Text
                    key={repository}
                    style={
                      selected.pullRequests.some((pr) => pr.repository?.name === repository && pr.checksFailing) &&
                      styles.failing
                    }
                  >
                    {'  ·  '}
                    {repository}
                  </Text>
                ))}
              </Text>
            </View>
            {proposals.map((proposal) => (
              <ProposalView
                key={proposal.id}
                proposal={proposal}
                busy={busy === proposal.id}
                disabled={busy !== undefined}
                onApprove={(reply) => approve(proposal.id, reply)}
                onDismiss={() => dismiss(proposal.id)}
              />
            ))}
            <View>
              <Text style={styles.section}>Pull requests</Text>
              {selected.pullRequests.map((pr) => (
                <Pressable key={pr.url} style={styles.pr} onPress={() => Linking.openURL(pr.url)}>
                  <Text style={styles.prRef}>
                    {pr.repository?.owner}/{pr.repository?.name}#{pr.number}
                  </Text>
                  <Text numberOfLines={1} style={styles.prTitle}>
                    {pr.title}
                  </Text>
                </Pressable>
              ))}
            </View>
          </View>
        )}
      </ScrollView>
      <StatusBar style="auto" />
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
    width: 256,
    backgroundColor: '#1D2630',
    borderRightWidth: 1,
    borderRightColor: 'rgba(255, 255, 255, 0.06)',
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
    maxWidth: 736,
    gap: 12,
  },
  titleBlock: {
    gap: 4,
    marginBottom: 8,
  },
  title: {
    color: '#D0CFD4',
    fontSize: 20,
    fontWeight: '600',
  },
  ready: {
    color: '#4ADE80',
  },
  failing: {
    color: '#F87171',
  },
  meta: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 12,
  },
  section: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 11,
    fontWeight: '600',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    marginTop: 12,
    marginBottom: 4,
  },
  pr: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 32,
    borderTopWidth: 1,
    borderTopColor: 'rgba(255, 255, 255, 0.06)',
  },
  prRef: {
    color: 'rgba(208, 207, 212, 0.55)',
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  prTitle: {
    flex: 1,
    color: '#D0CFD4',
    fontSize: 14,
  },
});
