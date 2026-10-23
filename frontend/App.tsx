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

  useEffect(() => {
    fetch('http://localhost:8080/prs', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(toJson(ListOpenPullRequestsRequestSchema, create(ListOpenPullRequestsRequestSchema))),
    })
      .then((res) => res.json())
      .then((json) => setClusters(fromJson(ListOpenPullRequestsResponseSchema, json).clusters));
  }, []);

  return (
    <ScrollView style={styles.container} contentContainerStyle={styles.grid}>
      <View style={styles.header}>
        <View style={styles.filters}>
          {['org: livekit, livekit-examples', 'author: u9g'].map((filter) => (
            <Text key={filter} style={styles.filter}>
              {filter}
            </Text>
          ))}
        </View>
      </View>
      {clusters.map((cluster) => (
        <View key={cluster.pullRequests[0].url} style={styles.cell}>
          <View style={styles.card}>
            {cluster.updatedAt && (
              <Text style={styles.updated}>{timeAgo(timestampDate(cluster.updatedAt))}</Text>
            )}
            {cluster.pullRequests.map((pr, i) => (
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
          </View>
        </View>
      ))}
      <StatusBar style="auto" />
    </ScrollView>
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
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2E3946',
  },
  updated: {
    color: '#D0CFD4',
    opacity: 0.6,
    fontSize: 12,
    paddingHorizontal: 12,
    paddingTop: 12,
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
    marginBottom: 4,
  },
  text: {
    color: '#D0CFD4',
  },
});
