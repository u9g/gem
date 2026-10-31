import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { View } from 'react-native';

import { DbContext, usePending } from '../db';
import { MemoryDb } from '../memory';
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

// The first is being approved, as the view says, from this frontend or another.
const approving = new MemoryDb({ pending: [bySubject(proposals)[0][0].id] });

function Approving() {
  const busy = usePending();
  return (
    <View style={{ minHeight: '100%', padding: 24, gap: 28, backgroundColor: t3.background }}>
      {bySubject(proposals).map((group) => (
        <ProposalGroup key={group[0].id} proposals={group} busy={busy} onApprove={() => {}} onDismiss={() => {}} />
      ))}
    </View>
  );
}

export const OneApproving: StoryObj = {
  name: 'One approving',
  render: () => (
    <DbContext value={approving}>
      <Approving />
    </DbContext>
  ),
};

export const Permalink: StoryObj = {
  name: 'Permalink',
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, backgroundColor: t3.background }}>
      <ProposalGroup proposals={permalinkProposals} onApprove={() => {}} onDismiss={() => {}} />
    </View>
  ),
};
