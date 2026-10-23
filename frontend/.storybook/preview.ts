import type { Preview } from '@storybook/react-native-web-vite';

const preview: Preview = {
  parameters: {
    layout: 'fullscreen',
    backgrounds: { options: { app: { name: 'App', value: '#19212A' } } },
  },
  initialGlobals: { backgrounds: { value: 'app' } },
};

export default preview;
