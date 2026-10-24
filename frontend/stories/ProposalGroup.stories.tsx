import type { Meta, StoryObj } from '@storybook/react-native-web-vite';
import { View } from 'react-native';

import { byComment, ProposalGroup } from '../ProposalView';
import { t3 } from '../t3';
import { proposals } from './fixtures';

const meta: Meta = { title: 'Proposals', parameters: { layout: 'fullscreen' } };
export default meta;

export const SideBySide: StoryObj = {
  name: 'Side by side',
  render: () => (
    <View style={{ minHeight: '100%', padding: 24, gap: 28, backgroundColor: t3.background }}>
      {byComment(proposals).map((group) => (
        <ProposalGroup key={group[0].id} proposals={group} onApprove={() => {}} onDismiss={() => {}} />
      ))}
    </View>
  ),
};
