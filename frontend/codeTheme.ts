import type { Theme } from '@pierre/highlights';

// gem's code theme, from its palette (t3.tsx): text in the UI's own color and accents from its blue
// and amber. Red and green are only for a diff's own + and - lines, as the diff surface tints added
// and removed lines with them. Each color reads at 4.5:1 or more on the code surface.
const text = '#D0CFD4';
const comment = '#8A93A0';
const punctuation = '#9BA3B2';
const keyword = '#8EA8F7'; // the primary blue, lightened
const fn = '#C29BF5'; // lavender, beside the text's lavender gray
const string = '#E6C384'; // the warning amber, softened
const constant = '#F2A57E'; // peach
const type = '#7FC6E6'; // sky

export const codeTheme: Theme = {
  name: 'gem',
  appearance: 'dark',
  style: {
    background: '#19212A',
    text,
    'editor.background': '#19212A',
    'editor.foreground': text,
    syntax: {
      comment: { color: comment },
      'comment.doc': { color: comment },
      keyword: { color: keyword },
      preproc: { color: keyword },
      tag: { color: keyword },
      title: { color: keyword, font_weight: 700 },
      function: { color: fn },
      string: { color: string },
      'string.escape': { color: constant },
      'string.regex': { color: constant },
      'string.special': { color: constant },
      'string.special.symbol': { color: constant },
      'text.literal': { color: string },
      link_text: { color: string },
      link_uri: { color: type },
      number: { color: constant },
      boolean: { color: constant },
      constant: { color: constant },
      'variable.builtin': { color: constant },
      'variable.special': { color: constant },
      type: { color: type },
      'type.builtin': { color: type },
      constructor: { color: type },
      enum: { color: type },
      variant: { color: constant },
      namespace: { color: type },
      attribute: { color: type },
      'property.json_key': { color: type },
      label: { color: type },
      selector: { color: keyword },
      'selector.class': { color: type },
      'selector.id': { color: constant },
      'selector.pseudo': { color: fn },
      embedded: { color: text },
      variable: { color: text },
      'variable.parameter': { color: text },
      property: { color: text },
      primary: { color: text },
      operator: { color: punctuation },
      'keyword.operator': { color: keyword },
      punctuation: { color: punctuation },
      'punctuation.bracket': { color: punctuation },
      'punctuation.delimiter': { color: punctuation },
      'punctuation.special': { color: keyword },
      'punctuation.list_marker': { color: keyword },
      emphasis: { font_style: 'italic' },
      'emphasis.strong': { font_weight: 700 },
      'diff.plus': { color: '#10B981' },
      'diff.minus': { color: '#F26D6D' },
    },
  },
};
