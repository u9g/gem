import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { View } from 'react-native';

import { InvestigationRow, SectionTitle } from '../../ClusterRow';
import { InvestigationView } from '../../InvestigationView';
import { t3 } from '../../t3';
import { investigations } from '../fixtures';

const meta: Meta = { title: 'Investigations', parameters: { layout: 'fullscreen' } };
export default meta;

const ago = (date: Date) => `${Math.round((Date.now() - date.getTime()) / 60_000)} minutes ago`;

export const Sidebar: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', width: 375, padding: 8, backgroundColor: '#1D2630' }}>
      <SectionTitle>Investigations</SectionTitle>
      {investigations.map((investigation, i) => (
        <InvestigationRow key={investigation.id} investigation={investigation} selected={i === 1} onPress={() => {}} />
      ))}
    </View>
  ),
};

export const Answered: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <InvestigationView
        investigation={investigations[1]}
        ago={ago}
        onApprove={() => {}}
        onDismiss={() => {}}
        onMessage={() => Promise.resolve()}
        onClose={() => {}}
      />
    </View>
  ),
};

export const Working: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <InvestigationView
        investigation={investigations[0]}
        ago={ago}
        onApprove={() => {}}
        onDismiss={() => {}}
        onMessage={() => Promise.resolve()}
        onClose={() => {}}
      />
    </View>
  ),
};
