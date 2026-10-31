import {
  Bot,
  CheckCheck,
  CircleX,
  ExternalLink,
  FileCode,
  GitPullRequestClosed,
  GitPullRequestCreate,
  Hammer,
  RotateCcw,
  Undo2,
} from 'lucide-react';
import { createContext, type CSSProperties, type ReactNode, useContext, useEffect, useMemo, useState } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';

import 'github-markdown-css/github-markdown-dark.css';

import type { CheckFailure, Comment, NewPullRequest, Proposal } from './shapes';
import { languageOf, Tokens, useTokens } from './highlight';
import { editHunk, newSides, splitFiles } from './patch';
import { DiffStat, diffFiles, Icon, t3 } from './t3';

// T3 Code's surfaces for what a proposal does, rebuilt in React Native.

// GitHub login of the user the server runs as.
export const Login = createContext('');

// Markdown as GitHub renders it in a comment on the pull request at `pr`. Without it, the user's
// rewrites show as plain text.
export const RenderMarkdown = createContext<((pr: string, markdown: string) => Promise<string>) | undefined>(undefined);

// `number` is the line's number in the new file, `old` in the old one.
export type DiffLine = { kind: 'add' | 'del' | 'ctx' | 'hunk'; text: string; number?: number; old?: number };

export function parseDiff(diff: string): { path: string; lines: DiffLine[] }[] {
  const files: { path: string; lines: DiffLine[] }[] = [];
  let [old, next] = [0, 0];
  // Lines left in the current hunk on each side, from its header; within them, every line is code.
  let [oldLeft, newLeft] = [0, 0];
  const lines = diff.split('\n');
  for (const [i, line] of lines.entries()) {
    const inHunk = oldLeft > 0 || newLeft > 0;
    // A file's header is a `--- ` line then a `+++ ` line, between hunks.
    if (!inHunk && line.startsWith('--- ') && lines[i + 1]?.startsWith('+++ ')) continue;
    if (!inHunk && line.startsWith('+++ ') && lines[i - 1]?.startsWith('--- ')) {
      // A deleted file's new side is /dev/null; its path is on the old side.
      files.push({ path: (line === '+++ /dev/null' ? lines[i - 1] : line).slice(6), lines: [] });
    }
    else if (!inHunk && line.startsWith('@@')) {
      const [, from, fromCount, to, toCount] = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))?/.exec(line) ?? [];
      [old, next] = [Number(from ?? 0), Number(to ?? 0)];
      [oldLeft, newLeft] = [Number(fromCount ?? 1), Number(toCount ?? 1)];
      files[files.length - 1].lines.push({ kind: 'hunk', text: line.replace(/^@@[^@]*@@ ?/, '') });
    // Between hunks every other line is header; inside one, an empty line is an empty context line
    // whose leading space was stripped.
    } else if (!files.length || !inHunk || line.startsWith('\\')) continue;
    else if (line.startsWith('+')) {
      newLeft--;
      files[files.length - 1].lines.push({ kind: 'add', text: line.slice(1), number: next++ });
    } else if (line.startsWith('-')) {
      oldLeft--;
      files[files.length - 1].lines.push({ kind: 'del', text: line.slice(1), old: old++ });
    } else {
      [oldLeft, newLeft] = [oldLeft - 1, newLeft - 1];
      files[files.length - 1].lines.push({ kind: 'ctx', text: line.slice(1), number: next++, old: old++ });
    }
  }
  return files;
}

// T3 Code's diff surface: code background, tinted add/delete lines, a new-line-number gutter.
// `header` replaces each file header's stats, e.g. with an error mark. Given `onChange`, each file's
// Diff / Edit switch flips it to the code the patch leaves, to rewrite; `diff` is then the user's
// rewrite of `suggested`, which has the same files.
export function DiffSurface({
  diff,
  header,
  suggested,
  onChange,
}: {
  diff: string;
  header?: ReactNode;
  suggested?: string;
  onChange?: (diff: string) => void;
}) {
  const files = splitFiles(diff);
  const theirs = suggested === undefined ? [] : splitFiles(suggested);
  return (
    <View style={{ gap: 8 }}>
      {files.map((file, i) => (
        <DiffFile
          key={i}
          file={file}
          header={header}
          suggested={theirs[i]}
          onChange={onChange && ((edited) => onChange(files.map((f, j) => (j === i ? edited : f)).join('\n')))}
        />
      ))}
    </View>
  );
}

// One file of a patch; renders nothing for a piece without a file.
function DiffFile({
  file: piece,
  header,
  suggested,
  onChange,
}: {
  file: string;
  header?: ReactNode;
  suggested?: string;
  onChange?: (file: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [file] = parseDiff(piece);
  // Each side of the file is highlighted on its own, so a line reads as it does in its version.
  const language = languageOf(file?.path ?? '');
  const side = (kind: DiffLine['kind']) => (file?.lines ?? []).filter((line) => line.kind === 'ctx' || line.kind === kind);
  const newTokens = useTokens(side('add').map((line) => line.text).join('\n'), language);
  const oldTokens = useTokens(side('del').map((line) => line.text).join('\n'), language);
  if (!file) return null;
  let [newLine, oldLine] = [0, 0];
  const tokens = file.lines.map((line) =>
    line.kind === 'add'
      ? newTokens?.[newLine++]
      : line.kind === 'del'
        ? oldTokens?.[oldLine++]
        : line.kind === 'ctx'
          ? (oldLine++, newTokens?.[newLine++])
          : undefined,
  );
  const [stat] = diffFiles(piece);
  const hunks = onChange && newSides(piece);
  const edited = suggested !== undefined && piece !== suggested;
  const gutter = gutterWidth(
    editing && hunks ? hunks.map((hunk) => hunk.start + hunk.lines.length - 1) : file.lines.map((line) => line.number ?? 0),
  );
  const lines =
    editing && hunks
      ? hunks.map((hunk, i) => (
          <View key={i}>
            <Text style={styles.hunk}>
              {'  ⋯  '}
              {hunk.context}
            </Text>
            <View style={styles.diffLine}>
              <View>
                {hunk.lines.map((_, n) => (
                  <Text key={n} numberOfLines={1} style={[styles.gutter, gutter]}>
                    {hunk.start + n}
                  </Text>
                ))}
              </View>
              <TextInput
                multiline
                autoFocus={i === 0}
                value={hunk.lines.join('\n')}
                onChangeText={(text) => onChange!(editHunk(piece, i, text === '' ? [] : text.split('\n')))}
                style={[styles.diffText, styles.codeInput]}
              />
            </View>
          </View>
        ))
      : file.lines.map((line, i) =>
          line.kind === 'hunk' ? (
            <Text key={i} style={styles.hunk}>
              {'  ⋯  '}
              {line.text}
            </Text>
          ) : (
            <View
              key={i}
              style={[
                styles.diffLine,
                line.kind === 'add' && { backgroundColor: 'rgba(16, 185, 129, 0.12)' },
                line.kind === 'del' && { backgroundColor: 'rgba(242, 109, 109, 0.12)' },
              ]}
            >
              <Text numberOfLines={1} style={[styles.gutter, gutter]}>
                {line.number ?? ''}
              </Text>
              <Text
                style={[
                  styles.diffText,
                  line.kind === 'add' && { color: '#6EE7B7' },
                  line.kind === 'del' && { color: '#FCA5A5' },
                ]}
              >
                {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ' '} <Tokens tokens={tokens[i]} text={line.text} />
              </Text>
            </View>
          ),
        );
  return (
    <CodeSurface
      path={file.path}
      header={
        <>
          {edited && <Text style={styles.edited}>Edited</Text>}
        {edited && (
          <Pressable accessibilityLabel="Revert to the suggestion" onPress={() => onChange!(suggested)} style={styles.revert}>
            <Icon icon={Undo2} size={12} />
          </Pressable>
        )}
          {header ?? <DiffStat added={stat.added} removed={stat.removed} />}
          {hunks && (
            <View style={[styles.segmented, { marginLeft: 4 }]}>
              {[false, true].map((edit) => (
                <Pressable key={String(edit)} onPress={() => setEditing(edit)} style={[styles.segment, editing === edit && styles.segmentOn]}>
                  <Text style={[styles.segmentText, editing === edit && { color: t3.foreground }]}>{edit ? 'Edit' : 'Diff'}</Text>
                </Pressable>
              ))}
            </View>
          )}
        </>
      }
    >
      {editing && hunks ? lines : <CodeLines>{lines}</CodeLines>}
    </CodeSurface>
  );
}

// T3 Code's code surface: the file's name over its code, with `header` after the name, or the code
// alone without a path. Given `href`, the name links to the file.
function CodeSurface({ path, href, header, children }: { path?: string; href?: string; header?: ReactNode; children: ReactNode }) {
  const name = (
    <Text numberOfLines={1} style={styles.diffPath}>
      {path?.split('/').slice(-2).join('/')}
    </Text>
  );
  return (
    <View style={styles.diffSurface}>
      {path !== undefined && (
        <View style={styles.diffHeader}>
          <Icon icon={FileCode} size={14} />
          {href ? (
            // Inline, as a link in .markdown-body would otherwise take the stylesheet's link color.
            <a href={href} style={{ display: 'flex', flex: 1, minWidth: 0, color: 'inherit', textDecoration: 'none' }}>
              {name}
            </a>
          ) : (
            name
          )}
          {header}
        </View>
      )}
      {children}
    </View>
  );
}

// Lines of code, scrolling sideways together.
function CodeLines({ children }: { children: ReactNode }) {
  return (
    <ScrollView horizontal contentContainerStyle={styles.diffLines}>
      <View style={styles.diffLines}>{children}</View>
    </ScrollView>
  );
}

// A gutter wide enough for the longest of `numbers`, so none wraps.
function gutterWidth(numbers: number[]) {
  return { width: String(Math.max(0, ...numbers)).length * 7 + 14 };
}

// Code in a body: the lines of a file GitHub embeds for a permalink to them, or a fenced block,
// which has no file or line numbers.
type Code = { path?: string; url?: string; language: string; lines: { number?: number; text: string }[] };

function CodeBlock({ code }: { code: Code }) {
  const tokens = useTokens(code.lines.map((line) => line.text).join('\n'), code.language);
  const gutter = code.path !== undefined && gutterWidth(code.lines.map((line) => line.number ?? 0));
  return (
    <View style={{ marginBottom: 16 }}>
      <CodeSurface path={code.path} href={code.url}>
        <CodeLines>
          <View style={!gutter && { paddingVertical: 8 }}>
            {code.lines.map((line, i) => (
              <View key={i} style={styles.diffLine}>
                {gutter && (
                  <Text numberOfLines={1} style={[styles.gutter, gutter]}>
                    {line.number}
                  </Text>
                )}
                <Text style={[styles.diffText, !gutter && { paddingLeft: 12 }]}>
                  <Tokens tokens={tokens?.[i]} text={line.text} />
                </Text>
              </View>
            ))}
          </View>
        </CodeLines>
      </CodeSurface>
    </View>
  );
}

// GitHub's rendering of a body cut into its HTML and the code at its top level: a fenced block is a
// <pre>, in a .highlight when it names a language; a permalink's lines are a Box holding a header
// that links to them and a table with a row per line.
function pieces(html: string): (string | Code)[] {
  const body = new DOMParser().parseFromString(html, 'text/html').body;
  const out: (string | Code)[] = [];
  const chunk = document.createElement('div');
  const flush = () => {
    if (chunk.childNodes.length) out.push(chunk.innerHTML);
    chunk.replaceChildren();
  };
  for (const node of [...body.childNodes]) {
    const pre = node instanceof Element && (node.matches('pre') ? node : node.querySelector(':scope.highlight > pre'));
    if (pre) {
      flush();
      // GitHub names the language by its grammar's scope, as in highlight-source-python or
      // highlight-text-html-basic. The last line ends in a newline.
      const scope = /\bhighlight-(?:source|text)-(\S+)/.exec((node as Element).className)?.[1];
      out.push({
        language: scope?.split('-')[0] ?? 'text',
        lines: (pre.textContent ?? '').trimEnd().split('\n').map((text) => ({ text })),
      });
      continue;
    }
    const code = node instanceof Element && node.querySelector(':scope > .blob-wrapper-embedded');
    const link = code && node.querySelector<HTMLAnchorElement>('.Box-header a');
    if (!code || !link) {
      chunk.append(node);
      continue;
    }
    flush();
    // /owner/repo/blob/sha/path
    const path = new URL(link.href).pathname.split('/').slice(5).join('/');
    out.push({
      path,
      url: link.href,
      language: languageOf(path),
      lines: [...code.querySelectorAll('tr')].map((row) => ({
        number: Number(row.querySelector('[data-line-number]')?.getAttribute('data-line-number')),
        // GitHub puts a space on either side of each line's code.
        text: row.querySelector('.blob-code')?.textContent?.replace(/^ /, '').trimEnd() ?? '',
      })),
    });
  }
  flush();
  return out;
}

// T3 Code's message bubble: rounded-2xl on the message surface, at most 80% wide. The user's messages
// sit on the right and everyone else's on the left, as in a chat.
export function Bubble({ children, side = 'right' }: { children: ReactNode; side?: 'left' | 'right' }) {
  return (
    <View style={{ alignItems: side === 'right' ? 'flex-end' : 'flex-start' }}>
      <View style={styles.bubble}>{children}</View>
    </View>
  );
}

// T3 Code's markdown blockquote: a 2px rule at 35% of the text color.
export function Quote({ children, lines }: { children: string; lines?: number }) {
  return (
    <View style={styles.quote}>
      <Text numberOfLines={lines} style={styles.quoteText}>
        {children}
      </Text>
    </View>
  );
}

// A comment in the conversation, as a message from its author: the user's on the right like the reply,
// everyone else's on the left. The body is GitHub's own rendering, styled with GitHub's stylesheet,
// cut to `max` pixels when given.
export function Theirs({ comment, trailing, max }: { comment: Comment; trailing?: ReactNode; max?: number }) {
  const author = comment.author;
  const side = author === useContext(Login) ? 'right' : 'left';
  return (
    <View style={{ gap: 4 }}>
      <View style={[styles.theirsHeader, side === 'right' && styles.theirsHeaderRight]}>
        <Text style={styles.meta}>{author}</Text>
        {trailing}
      </View>
      <Bubble side={side}>
        {max ? (
          <Clamp max={max}>
            <Html html={comment.bodyHtml} />
          </Clamp>
        ) : (
          <Html html={comment.bodyHtml} />
        )}
      </Bubble>
    </View>
  );
}

// Cuts its content to `max` pixels, fading out the last lines, with Show more under it when
// anything was cut.
// Each failed check with the log lines that show why it failed, opening its logs.
export function FailedChecks({ failure }: { failure: CheckFailure }) {
  return (
    <View style={{ gap: 8 }}>
      {failure.checks.map((check) => (
        <View key={check.id} style={styles.check}>
          <View style={styles.checkHeader}>
            <Icon icon={CircleX} size={12} color={t3.deletion} />
            <Text numberOfLines={1} style={styles.checkName}>
              {check.name}{' '}
              <Text style={styles.meta}>· {check.conclusion.toLowerCase().replace('_', ' ')}</Text>
            </Text>
            <Pressable accessibilityLabel="Open logs" onPress={() => Linking.openURL(check.url)}>
              <Icon icon={ExternalLink} size={12} />
            </Pressable>
          </View>
          {check.excerpt !== '' && <Text style={styles.excerpt}>{check.excerpt.trimEnd()}</Text>}
        </View>
      ))}
    </View>
  );
}

export function RerunCard({ failure }: { failure: CheckFailure }) {
  return (
    <View style={styles.rerun}>
      <Icon icon={RotateCcw} size={14} />
      <Text style={styles.checkName}>
        Reruns {failure.checks.map((c) => c.name).join(' and ')} on the same commit. No code changes.
      </Text>
    </View>
  );
}

export function Clamp({ max, children }: { max: number; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [height, setHeight] = useState(0);
  // Cutting less than a couple of lines saves nothing.
  const cut = height > max + 24;
  return (
    // Focus inside opens it, so nothing being typed in is cut off.
    <View onFocus={() => setOpen(true)}>
      <View style={[{ overflow: 'hidden' }, cut && !open && ({ maxHeight: max, ...fade } as object)]}>
        <View onLayout={(event) => setHeight(event.nativeEvent.layout.height)}>{children}</View>
      </View>
      {cut && (
        <Pressable onPress={() => setOpen(!open)} style={{ alignSelf: 'flex-start' }}>
          <Text style={styles.more}>{open ? 'Show less' : 'Show more'}</Text>
        </Pressable>
      )}
    </View>
  );
}

const fade = { maskImage: 'linear-gradient(to bottom, black 60%, transparent)' };

// GitHub's rendering of a body, styled with GitHub's stylesheet. Links leave the app for a new
// tab; a click anywhere else calls onPress.
export function Html({
  html,
  onPress,
  className,
  style,
}: {
  html: string;
  onPress?: () => void;
  className?: string;
  style?: CSSProperties;
}) {
  // Cut only when there is code, so everything else stays a direct child of .markdown-body.
  const parts = useMemo(() => (/<pre|blob-wrapper-embedded/.test(html) ? pieces(html) : undefined), [html]);
  return (
    <div
      className={className ? `markdown-body ${className}` : 'markdown-body'}
      style={{ ...markdownBody, ...style }}
      dangerouslySetInnerHTML={parts ? undefined : { __html: html }}
      onClick={(event) => {
        const link = (event.target as Element).closest('a');
        if (!link) return onPress?.();
        event.preventDefault();
        window.open(link.href, '_blank');
      }}
    >
      {parts?.map((part, i) =>
        typeof part === 'string' ? (
          <div key={i} dangerouslySetInnerHTML={{ __html: part }} />
        ) : (
          <CodeBlock key={i} code={part} />
        ),
      )}
    </div>
  );
}

const markdownBody = {
  backgroundColor: 'transparent',
  color: t3.foreground,
  fontSize: 13,
  lineHeight: 1.55,
  minWidth: 0,
};

// A reply as both versions: the agent's suggestion and the user's rewrite of it.
export type ReplyDraft = { yours: string; showing: 'suggested' | 'yours' };

// The reply's bubble, rendered as it will look on GitHub until pressed, then editable until left.
// Once the text differs from the suggestion, a Suggested / Yours switch under it keeps both
// versions one click away; editing always edits Yours.
export function EditableReply({
  pr,
  suggested,
  html,
  draft,
  onChange,
  max,
}: {
  pr: string;
  suggested: string;
  // The suggestion as GitHub renders it.
  html: string;
  draft: ReplyDraft;
  onChange: (draft: ReplyDraft) => void;
  // Cuts the rendered reply to `max` pixels when given.
  max?: number;
}) {
  const [editing, setEditing] = useState(false);
  const hasYours = draft.yours.trim() !== suggested.trim();
  return (
    <View>
      <View style={{ alignItems: 'flex-end' }}>
        <View style={[styles.bubble, styles.editableBubble, editing && styles.bubbleFocused]}>
          <EditableMarkdown
            pr={pr}
            suggested={suggested}
            html={html}
            draft={draft}
            onChange={onChange}
            editing={editing}
            onEditing={setEditing}
            max={max}
          />
        </View>
      </View>
      {hasYours && <Versions draft={draft} onChange={onChange} />}
    </View>
  );
}

// Switches between the agent's suggestion and the user's rewrite of it.
function Versions({ draft, onChange }: { draft: ReplyDraft; onChange: (draft: ReplyDraft) => void }) {
  return (
    <View style={styles.versions}>
      <View style={styles.segmented}>
        {(['suggested', 'yours'] as const).map((version) => (
          <Pressable
            key={version}
            onPress={() => onChange({ ...draft, showing: version })}
            style={[styles.segment, draft.showing === version && styles.segmentOn]}
          >
            <Text style={[styles.segmentText, draft.showing === version && { color: t3.foreground }]}>
              {version === 'suggested' ? 'Suggested' : 'Yours'}
            </Text>
          </Pressable>
        ))}
      </View>
    </View>
  );
}

// The version showing, as GitHub renders it until pressed, then its markdown until left. Yours
// renders only once left, and shows as plain text until it has. Either keeps at least a line to
// press, even when the markdown renders to nothing. The Clamp spans both, so leaving does not
// collapse what is under the pointer.
function EditableMarkdown({
  pr,
  suggested,
  html,
  draft,
  onChange,
  editing,
  onEditing,
  max,
}: {
  pr: string;
  suggested: string;
  html: string;
  draft: ReplyDraft;
  onChange: (draft: ReplyDraft) => void;
  editing: boolean;
  onEditing: (editing: boolean) => void;
  max?: number;
}) {
  const render = useContext(RenderMarkdown);
  const [rendered, setRendered] = useState({ markdown: '', html: '' });
  const markdown = draft.showing === 'yours' ? draft.yours : suggested.trim();
  const stale = draft.showing === 'yours' && rendered.markdown !== markdown;
  useEffect(() => {
    if (editing || !stale || !render) return;
    let current = true;
    render(pr, markdown).then(
      (html) => current && setRendered({ markdown, html }),
      () => {},
    );
    return () => {
      current = false;
    };
  }, [editing, stale, render, pr, markdown]);
  const shown = editing ? (
    <TextInput
      multiline
      autoFocus
      value={markdown}
      onChangeText={(yours) => onChange({ yours, showing: 'yours' })}
      onBlur={() => onEditing(false)}
      style={styles.input}
    />
  ) : stale ? (
    <Pressable onPress={() => onEditing(true)} style={{ minHeight: line }}>
      <Text style={styles.input}>{markdown}</Text>
    </Pressable>
  ) : (
    <Html
      html={draft.showing === 'yours' ? rendered.html : html}
      onPress={() => onEditing(true)}
      style={{ minHeight: line }}
    />
  );
  return max === undefined ? shown : <Clamp max={max}>{shown}</Clamp>;
}

// One line of `styles.input`.
const line = 20;

// The prompt approving hands to a new T3 Code agent, as GitHub would render it. Clicking it edits
// the prompt, with the same Suggested / Yours switch as replies.
export function FollowUpCard({
  pr,
  suggested,
  html,
  draft,
  onChange,
}: {
  pr: string;
  suggested: string;
  html: string;
  draft: ReplyDraft;
  onChange: (draft: ReplyDraft) => void;
}) {
  const [editing, setEditing] = useState(false);
  return (
    <View style={styles.followUp}>
      <View style={styles.followUpHeader}>
        <Icon icon={Bot} size={14} color={t3.primary} />
        <Text style={styles.meta}>
          <Text style={{ color: t3.foreground, fontWeight: '500' }}>Follow-up agent</Text>
          {'  '}starts in T3 Code when approved
        </Text>
      </View>
      <EditableMarkdown
        pr={pr}
        suggested={suggested}
        html={html}
        draft={draft}
        onChange={onChange}
        editing={editing}
        onEditing={setEditing}
        max={130}
      />
      {draft.yours.trim() !== suggested.trim() && <Versions draft={draft} onChange={onChange} />}
    </View>
  );
}

// The pull request approving opens: where it goes, its title and description as GitHub renders
// them, and the change it commits, until an approve that failed after pushing it.
export function NewPullRequestCard({ pullRequest }: { pullRequest: NewPullRequest }) {
  const patch = pullRequest.patch;
  const diff = patch?.diff ?? '';
  const files = diffFiles(diff);
  const branch = pullRequest.branch;
  return (
    <View style={styles.followUp}>
      <View style={styles.followUpHeader}>
        <Icon icon={GitPullRequestCreate} size={14} color={t3.primary} />
        <Text style={styles.meta}>
          <Text style={{ color: t3.foreground, fontWeight: '500' }}>New pull request</Text>
          {'  '}
          {pullRequest.repository} · {pullRequest.base} ← {branch}
        </Text>
      </View>
      <Text style={{ color: t3.foreground, fontWeight: '600' }}>{pullRequest.title}</Text>
      <Clamp max={130}>
        <Html html={pullRequest.bodyHtml} />
      </Clamp>
      {patch ? (
        <>
          <Text style={styles.meta}>
            {patch.commitMessage}
            {'  '}
            <DiffStat added={files.reduce((n, f) => n + f.added, 0)} removed={files.reduce((n, f) => n + f.removed, 0)} />
          </Text>
          <Clamp max={130}>
            <DiffSurface diff={diff} />
          </Clamp>
        </>
      ) : (
        <Text style={styles.meta}>Pushed {branch}; approving again opens the pull request.</Text>
      )}
    </View>
  );
}

export function ResolvesTag() {
  return (
    <View style={styles.inlineTag}>
      <Icon icon={CheckCheck} size={12} />
      <Text style={styles.inlineTagText}>resolves</Text>
    </View>
  );
}

export function ClosesTag() {
  return (
    <View style={styles.inlineTag}>
      <Icon icon={GitPullRequestClosed} size={12} />
      <Text style={styles.inlineTagText}>closes PR</Text>
    </View>
  );
}

// T3 Code's warning banner: amber label, the tool's name in mono, why it is needed below.
export function ToolRequestBanner({ proposal }: { proposal: Proposal }) {
  const [name] = proposal.summary.split(':');
  return (
    <View style={styles.warning}>
      <Icon icon={Hammer} color="#F59E0B" />
      <View style={{ flex: 1, gap: 4 }}>
        <Text style={styles.meta}>
          <Text style={{ color: '#F59E0B', fontWeight: '500' }}>Tool request</Text>
          {'  '}
          {proposal.comment?.author}
        </Text>
        <Text style={styles.mono}>{name}</Text>
        <Text style={styles.meta}>{proposal.requestedTool.trim()}</Text>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  more: {
    color: t3.muted,
    fontSize: 12,
    paddingTop: 4,
    textDecorationLine: 'underline',
  },
  meta: {
    color: t3.muted,
    fontSize: 11,
    lineHeight: 16,
  },
  mono: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  diffSurface: {
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
    overflow: 'hidden',
  },
  diffHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 6,
    height: 30,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  diffPath: {
    flex: 1,
    color: t3.foreground,
    fontSize: 12,
  },
  hunk: {
    color: t3.faint,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 20,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  diffLine: {
    flexDirection: 'row',
    minHeight: 18,
  },
  // Tinted lines reach the right edge even when the code is narrower than the surface.
  diffLines: {
    flexGrow: 1,
  },
  gutter: {
    paddingRight: 8,
    textAlign: 'right',
    color: t3.faint,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 18,
  },
  diffText: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 18,
    paddingRight: 12,
    whiteSpace: 'pre',
  } as object,
  // As tall as its code, and scrolls its own long lines: iOS scrolls a selection drag only within the
  // field it's in, never the field's scrolling ancestors.
  codeInput: {
    flex: 1,
    minWidth: 0,
    color: t3.foreground,
    padding: 0,
    outlineStyle: 'solid',
    outlineWidth: 0,
    fieldSizing: 'content',
    overflowX: 'auto',
    resize: 'none',
  } as object,
  edited: {
    color: t3.muted,
    fontSize: 11,
  },
  revert: {
    width: 22,
    height: 22,
    borderRadius: 6,
    alignItems: 'center',
    justifyContent: 'center',
  },
  bubble: {
    maxWidth: '80%',
    gap: 8,
    paddingHorizontal: 14,
    paddingVertical: 10,
    borderRadius: 16,
    backgroundColor: 'rgba(255, 255, 255, 0.06)',
  },
  editableBubble: {
    width: '80%',
    borderWidth: 1,
    borderColor: 'transparent',
  },
  bubbleFocused: {
    borderColor: 'rgba(55, 103, 232, 0.7)',
  },
  input: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
    padding: 0,
    outlineStyle: 'solid',
    outlineWidth: 0,
    fieldSizing: 'content',
    resize: 'none',
  } as object,
  versions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 6,
  },
  segmented: {
    flexDirection: 'row',
    padding: 2,
    borderRadius: 8,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  segment: {
    paddingHorizontal: 8,
    height: 20,
    justifyContent: 'center',
    borderRadius: 6,
  },
  segmentOn: {
    backgroundColor: 'rgba(255, 255, 255, 0.08)',
  },
  segmentText: {
    color: t3.muted,
    fontSize: 11,
  },
  check: {
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.input,
    overflow: 'hidden',
  },
  checkHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  excerpt: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 11,
    lineHeight: 16,
    paddingHorizontal: 10,
    paddingVertical: 8,
    backgroundColor: 'rgba(0, 0, 0, 0.25)',
  },
  rerun: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    padding: 10,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: t3.input,
  },
  checkName: {
    flex: 1,
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
  },
  theirsHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    paddingLeft: 14,
  },
  theirsHeaderRight: {
    justifyContent: 'flex-end',
    paddingLeft: 0,
    paddingRight: 14,
  },
  theirs: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 20,
  },
  quote: {
    borderLeftWidth: 2,
    borderLeftColor: 'rgba(208, 207, 212, 0.35)',
    paddingLeft: 12,
  },
  quoteText: {
    color: t3.foreground,
    fontSize: 13,
    lineHeight: 19,
  },
  inlineTag: {
    flexDirection: 'row',
    alignItems: 'center',
    alignSelf: 'flex-start',
    paddingRight: 6,
    borderRadius: 999,
    backgroundColor: 'rgba(255, 255, 255, 0.04)',
  },
  inlineTagText: {
    color: t3.muted,
    fontSize: 11,
  },
  followUp: {
    gap: 8,
    padding: 10,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: 'rgba(55, 103, 232, 0.35)',
    backgroundColor: 'rgba(55, 103, 232, 0.06)',
  },
  followUpHeader: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  warning: {
    flexDirection: 'row',
    gap: 6,
    padding: 10,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: 'rgba(245, 158, 11, 0.28)',
    backgroundColor: 'rgba(245, 158, 11, 0.08)',
  },
});
