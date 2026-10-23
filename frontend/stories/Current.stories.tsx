import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { ScrollView, View } from 'react-native';

import { ProposalView } from '../ProposalView';
import { items } from './fixtures';

function Current() {
  return (
    <View style={{ width: 400, height: 900, backgroundColor: '#19212A' }}>
      <ScrollView>
        {items.map(({ pr, proposal }) => (
          <ProposalView
            key={proposal.id}
            pr={pr}
            proposal={proposal}
            busy={false}
            disabled={false}
            onApprove={() => {}}
            onDismiss={() => {}}
          />
        ))}
      </ScrollView>
    </View>
  );
}

const meta: Meta<typeof Current> = { title: 'Proposals/0 Current', component: Current };
export default meta;

export const Sidebar: StoryObj<typeof Current> = {};
