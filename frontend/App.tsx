import { create, fromJson, toJson } from '@bufbuild/protobuf';
import { timestampDate } from '@bufbuild/protobuf/wkt';
import { StatusBar } from 'expo-status-bar';
import { useEffect, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import {
  ListOpenPullRequestsRequestSchema,
  ListOpenPullRequestsResponseSchema,
  type PullRequestCluster,
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

export default function App() {
  const [clusters, setClusters] = useState<PullRequestCluster[]>([]);
  const [selectedUrl, setSelectedUrl] = useState<string>();

  useEffect(() => {
    fetch('http://localhost:8080/prs', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(toJson(ListOpenPullRequestsRequestSchema, create(ListOpenPullRequestsRequestSchema))),
    })
      .then((res) => res.json())
      .then((json) => setClusters(fromJson(ListOpenPullRequestsResponseSchema, json).clusters));
  }, []);

  // A cluster is identified by its oldest PR, which stays put as the cluster grows.
  const selected = clusters.find((cluster) => cluster.pullRequests[0].url === selectedUrl);

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
          </View>
        </View>
        {clusters.map((cluster) => {
          const url = cluster.pullRequests[0].url;
          return (
            <View key={url} style={styles.cell}>
              <Pressable
                style={[styles.card, cluster === selected && styles.selectedCard]}
                onPress={() => setSelectedUrl(cluster === selected ? undefined : url)}
              >
                <View style={styles.cardHeader}>
                  {cluster.name !== '' && <Text style={styles.name}>{cluster.name}</Text>}
                  {cluster.updatedAt && (
                    <Text style={styles.updated}>{timeAgo(timestampDate(cluster.updatedAt))}</Text>
                  )}
                </View>
                <View style={styles.refs}>
                  {cluster.pullRequests.map((pr) => (
                    <Text key={pr.url} style={styles.repo}>
                      {pr.repository}#{pr.number}
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
            {selected.pullRequests.map((pr, i) => (
              <Pressable
                key={pr.url}
                style={[styles.pr, i > 0 && styles.stacked]}
                onPress={() => Linking.openURL(pr.url)}
              >
                <Text style={styles.repo}>
                  {pr.repository}#{pr.number}
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
  updated: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
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
