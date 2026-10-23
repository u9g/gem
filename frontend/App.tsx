import { create, fromJson, toJson } from '@bufbuild/protobuf';
import { StatusBar } from 'expo-status-bar';
import { useEffect, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import {
  ListOpenPullRequestsRequestSchema,
  ListOpenPullRequestsResponseSchema,
  type PullRequest,
} from './gen/gem_pb';

export default function App() {
  const [prs, setPrs] = useState<PullRequest[]>([]);

  useEffect(() => {
    fetch('http://localhost:8080/prs', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(toJson(ListOpenPullRequestsRequestSchema, create(ListOpenPullRequestsRequestSchema))),
    })
      .then((res) => res.json())
      .then((json) => setPrs(fromJson(ListOpenPullRequestsResponseSchema, json).pullRequests));
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
      {prs.map((pr) => (
        <View key={pr.url} style={styles.cell}>
          <Pressable style={styles.card} onPress={() => Linking.openURL(pr.url)}>
            <Text style={styles.repo}>
              {pr.repository}#{pr.number}
            </Text>
            <Text style={styles.text}>{pr.title}</Text>
          </Pressable>
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
    padding: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: '#2E3946',
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
