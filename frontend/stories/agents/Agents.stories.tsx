import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { View } from 'react-native';

import { AgentView } from '../../AgentView';
import { AgentRow, SectionTitle } from '../../ClusterRow';
import { t3 } from '../../t3';
import { agents } from '../fixtures';

const meta: Meta = { title: 'Agents', parameters: { layout: 'fullscreen' } };
export default meta;

const ago = (date: Date) => `${Math.round((Date.now() - date.getTime()) / 60_000)} minutes ago`;

export const Sidebar: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', width: 375, padding: 8, backgroundColor: '#1D2630' }}>
      <SectionTitle>Agents</SectionTitle>
      {agents.map((agent, i) => (
        <AgentRow key={agent.id} agent={agent} selected={i === 1} onPress={() => {}} />
      ))}
    </View>
  ),
};

export const Answered: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <AgentView
        agent={agents[2]}
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
      <AgentView
        agent={agents[1]}
        ago={ago}
        onApprove={() => {}}
        onDismiss={() => {}}
        onMessage={() => Promise.resolve()}
        onClose={() => {}}
      />
    </View>
  ),
};

export const FollowUp: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <AgentView
        agent={agents[3]}
        ago={ago}
        onApprove={() => {}}
        onDismiss={() => {}}
        onMessage={() => Promise.resolve()}
        onClose={() => {}}
      />
    </View>
  ),
};

export const Question: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <AgentView
        agent={agents[4]}
        ago={ago}
        onApprove={() => {}}
        onDismiss={() => {}}
        onMessage={() => Promise.resolve()}
        onClose={() => {}}
      />
    </View>
  ),
};

// Two proposals answering the same failed checks: one card, with arrows between them.
export const Alternatives: StoryObj = {
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <AgentView
        agent={agents[0]}
        ago={ago}
        onApprove={() => {}}
        onDismiss={() => {}}
        onMessage={() => Promise.resolve()}
        onClose={() => {}}
      />
    </View>
  ),
};
