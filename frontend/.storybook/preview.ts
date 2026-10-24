import type { Preview } from '@storybook/react-native-web-vite';
import { createElement } from 'react';

import { Login } from '../surfaces';

const preview: Preview = {
  parameters: {
    layout: 'fullscreen',
    backgrounds: { options: { app: { name: 'App', value: '#19212A' } } },
  },
  initialGlobals: { backgrounds: { value: 'app' } },
  decorators: [(Story) => createElement(Login, { value: 'u9g' }, createElement(Story))],
};

export default preview;
