import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { View } from 'react-native';

import { bySubject, ProposalGroup } from '../ProposalView';
import { t3 } from '../t3';
import { checkProposals, permalinkProposals, proposals } from './fixtures';

const meta: Meta = { title: 'Proposals', parameters: { layout: 'fullscreen' } };
export default meta;

export const Digest: StoryObj = {
  name: 'Digest',
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, gap: 28, backgroundColor: t3.background }}>
      {bySubject(proposals).map((group) => (
        <ProposalGroup key={group[0].id} proposals={group} onApprove={() => {}} onDismiss={() => {}} />
      ))}
    </View>
  ),
};

export const Checks: StoryObj = {
  name: 'Checks',
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, gap: 28, backgroundColor: t3.background }}>
      {bySubject(checkProposals).map((group) => (
        <ProposalGroup
          key={group[0].id}
          proposals={group}
          onApprove={() => {}}
          onDismiss={() => {}}
          onInvestigate={() => {}}
        />
      ))}
    </View>
  ),
};

export const OneApproving: StoryObj = {
  name: 'One approving',
  render: () => {
    const groups = bySubject(proposals);
    const busy = new Set([groups[0][0].id]);
    return (
      <View style={{ minHeight: '100%', padding: 24, gap: 28, backgroundColor: t3.background }}>
        {groups.map((group) => (
          <ProposalGroup key={group[0].id} proposals={group} busy={busy} onApprove={() => {}} onDismiss={() => {}} />
        ))}
      </View>
    );
  },
};

export const Permalink: StoryObj = {
  name: 'Permalink',
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <ProposalGroup proposals={permalinkProposals} onApprove={() => {}} onDismiss={() => {}} />
    </View>
  ),
};
