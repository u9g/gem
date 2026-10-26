import { create, fromJson, type JsonValue, toJson } from '@bufbuild/protobuf';
import { Search as SearchIcon } from 'lucide-react';
import { useEffect, useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, TextInput, View } from 'react-native';

import { SearchPullRequestsRequestSchema, SearchPullRequestsResponseSchema } from './gen/gem_pb';
import { t3 } from './t3';

// How long the query must stay unchanged before it is searched. Every search asks a model about
// every open PR.
const TYPING_PAUSE_MS = 800;

// Typing until the query stays unchanged for TYPING_PAUSE_MS, then searching until the server
// answers for it.
export type SearchStatus = 'idle' | 'typing' | 'searching' | 'done' | 'failed';

// The PR URLs matching the query once its search finished. Until then the previous query's
// matches are kept; there are none while the query is empty or after a search failed.
export function useSearch(post: (path: string, body: JsonValue) => Promise<JsonValue>) {
  const [query, setQuery] = useState('');
  const [searching, setSearching] = useState<string>();
  const [result, setResult] = useState<{ query: string; urls?: ReadonlySet<string>; error?: string }>();
  const trimmed = query.trim();

  useEffect(() => {
    if (!trimmed) return;
    const timer = setTimeout(() => {
      setSearching(trimmed);
      post('/prs/search', toJson(SearchPullRequestsRequestSchema, create(SearchPullRequestsRequestSchema, { query: trimmed })))
        .then(
          (json) => ({ query: trimmed, urls: new Set(fromJson(SearchPullRequestsResponseSchema, json).pullRequestUrls) }),
          (error: Error) => ({ query: trimmed, error: error.message }),
        )
        .then((finished) => {
          setResult(finished);
          setSearching((current) => (current === trimmed ? undefined : current));
        });
    }, TYPING_PAUSE_MS);
    return () => clearTimeout(timer);
  }, [trimmed, post]);

  const status: SearchStatus = !trimmed
    ? 'idle'
    : searching === trimmed
      ? 'searching'
      : result?.query !== trimmed
        ? 'typing'
        : result.error
          ? 'failed'
          : 'done';
  return { query, setQuery, status, matches: trimmed ? result?.urls : undefined, error: result?.error };
}

export function SearchBox({
  value,
  onChange,
  status,
  matched,
  error,
}: {
  value: string;
  onChange: (value: string) => void;
  status: SearchStatus;
  // Clusters shown for the finished search.
  matched: number;
  error?: string;
}) {
  return (
    <View style={styles.search}>
      <View style={styles.box}>
        <SearchIcon size={14} color={t3.muted} />
        <TextInput
          value={value}
          onChangeText={onChange}
          placeholder="Search PR descriptions…"
          placeholderTextColor={t3.faint}
          style={styles.input}
          accessibilityLabel="Search pull requests"
        />
      </View>
      {status === 'typing' && <Text style={styles.status}>waiting for you to stop typing…</Text>}
      {status === 'searching' && (
        <View style={styles.searching}>
          <ActivityIndicator size="small" color={t3.muted} />
          <Text style={styles.status}>asking about every PR…</Text>
        </View>
      )}
      {status === 'done' && (
        <Text style={styles.status}>
          {matched} {matched === 1 ? 'cluster matches' : 'clusters match'}
        </Text>
      )}
      {status === 'failed' && (
        <Text style={[styles.status, styles.failed]} numberOfLines={2}>
          search failed: {error}
        </Text>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  search: {
    flexGrow: 1,
    flexBasis: 240,
    gap: 4,
  },
  box: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    paddingVertical: 4,
    borderRadius: 999,
    backgroundColor: t3.input,
  },
  input: {
    flex: 1,
    minWidth: 0,
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 18,
    outlineStyle: 'none',
  } as object,
  searching: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingLeft: 10,
  },
  status: {
    color: t3.muted,
    fontSize: 11,
    paddingHorizontal: 10,
  },
  failed: {
    color: t3.deletion,
  },
});
