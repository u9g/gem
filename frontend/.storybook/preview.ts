import type { Preview } from '@storybook/react-native-web-vite';
import { createElement } from 'react';

import { Login, RenderMarkdown } from '../surfaces';

// GitHub's public markdown API, which the server calls through `gh`.
async function render(pr: string, markdown: string): Promise<string> {
  const context = pr.replace('https://github.com/', '').split('/').slice(0, 2).join('/');
  const response = await fetch('https://api.github.com/markdown', {
    method: 'POST',
    body: JSON.stringify({ text: markdown, mode: 'gfm', context }),
  });
  if (!response.ok) throw new Error(await response.text());
  return response.text();
}

const preview: Preview = {
  parameters: {
    layout: 'fullscreen',
    backgrounds: { options: { app: { name: 'App', value: '#19212A' } } },
  },
  initialGlobals: { backgrounds: { value: 'app' } },
  decorators: [
    (Story) =>
      createElement(Login, { value: 'u9g' }, createElement(RenderMarkdown, { value: render }, createElement(Story))),
  ],
};

export default preview;
