import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { StyleSheet, View } from 'react-native';

import { SearchBox, type SearchStatus } from '../../Search';

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

const meta: Meta = { title: 'Search' };
export default meta;

export const States: StoryObj = { render: () => <AllStates /> };

const styles = StyleSheet.create({
  page: {
    gap: 12,
    padding: 10,
  },
  row: {
    flexDirection: 'row',
  },
});
