import type { JsonValue } from '@bufbuild/protobuf';
import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { StyleSheet, Text, View } from 'react-native';

import { SearchBox, type SearchStatus, useSearch } from '../../Search';
import { t3 } from '../../t3';

// The search box in each state. Stories render at the window's width, so view them at a phone's
// and at a desktop's.

function AllStates() {
  const states: [string, SearchStatus, number, string?][] = [
    ['', 'idle', 0],
    ['sip phone num', 'typing', 0],
    ['sip phone numbers', 'searching', 0],
    ['sip phone numbers', 'done', 3],
    ['slack', 'done', 1],
    ['sip phone numbers', 'failed', 0, 'TypeSafe answered 429 Too Many Requests'],
  ];
  return (
    <View style={styles.page}>
      {states.map(([value, status, matched, error], i) => (
        <View key={i} style={styles.row}>
          <SearchBox value={value} onChange={() => {}} status={status} matched={matched} error={error} />
        </View>
      ))}
    </View>
  );
}

// Made-up PRs, searched by whether their title contains the query.
const titles = [
  'Post merge queue failures to Slack',
  'Retry Slack webhooks once',
  'Pin kubectl in CI',
  'Fail over to the backup region',
];

const post = async (_path: string, body: JsonValue): Promise<JsonValue> => {
  const query = ((body as { query: string }).query ?? '').toLowerCase();
  await new Promise((resolve) => setTimeout(resolve, 1500));
  return { pullRequestUrls: titles.filter((title) => title.toLowerCase().includes(query)) };
};

// The search box over a list of PRs, searching as you type.
function Live() {
  const search = useSearch(post);
  const shown = titles.filter((title) => !search.matches || search.matches.has(title));
  return (
    <View style={styles.page}>
      <View style={styles.row}>
        <SearchBox
          value={search.query}
          onChange={search.setQuery}
          status={search.status}
          matched={search.matches?.size ?? 0}
          error={search.error}
        />
      </View>
      {shown.map((title) => (
        <Text key={title} style={styles.title}>
          {title}
        </Text>
      ))}
    </View>
  );
}

const meta: Meta = { title: 'Search' };
export default meta;

export const States: StoryObj = { render: () => <AllStates /> };
export const Typing: StoryObj = { render: () => <Live /> };

const styles = StyleSheet.create({
  page: {
    gap: 12,
    padding: 10,
  },
  row: {
    flexDirection: 'row',
  },
  title: {
    color: t3.foreground,
    fontSize: 13,
  },
});
