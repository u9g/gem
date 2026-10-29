import type { ThemedToken } from '@pierre/highlights';
import { useEffect, useState } from 'react';
import { Text } from 'react-native';

// T3 Code's highlighter: Pierre's WASM lexers, in T3 Code's dark theme. Both load on first use, so
// they stay out of the app's first download.
let loading: Promise<{ highlight: (code: string, language: string) => ThemedToken[][] }> | undefined;
function load() {
  loading ??= Promise.all([import('@pierre/highlights'), import('@pierre/highlights/themes/pierre-dark')]).then(
    ([{ codeToTokens, isSupportedLanguage }, { default: theme }]) => ({
      // A language the lexers don't know comes out as plain text.
      highlight: (code: string, language: string) =>
        codeToTokens(code, { lang: isSupportedLanguage(language) ? language : 'text', theme }).tokens,
    }),
  );
  return loading;
}

// The language of the file at `path`: the lexers know languages by their files' extensions, and
// by name for files like Makefile.
export function languageOf(path: string): string {
  const name = path.split('/').pop()?.toLowerCase() ?? '';
  return name.includes('.') ? name.split('.').pop()! : name;
}

// `code`'s lines as colored tokens, once the highlighter has loaded.
export function useTokens(code: string, language: string): ThemedToken[][] | undefined {
  const [result, setResult] = useState<{ code: string; language: string; tokens: ThemedToken[][] }>();
  useEffect(() => {
    let current = true;
    load().then(
      ({ highlight }) => current && setResult({ code, language, tokens: highlight(code, language) }),
      () => {},
    );
    return () => {
      current = false;
    };
  }, [code, language]);
  return result?.code === code && result.language === language ? result.tokens : undefined;
}

// A line's tokens in their colors, or `text` in the surrounding color until they are ready.
export function Tokens({ tokens, text }: { tokens?: ThemedToken[]; text: string }) {
  if (!tokens) return text;
  return tokens.map((token, i) => (
    <Text
      key={i}
      style={{
        color: token.color,
        // Shiki's FontStyle flags.
        fontStyle: token.fontStyle && token.fontStyle & 1 ? 'italic' : undefined,
        fontWeight: token.fontStyle && token.fontStyle & 2 ? 'bold' : undefined,
      }}
    >
      {token.content}
    </Text>
  ));
}
