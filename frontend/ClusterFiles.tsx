import { FileCode, MessagesSquare } from 'lucide-react';
import { Fragment, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { ClusterAgents, type ClusterView, MergeBanner, RebaseClusterButton, timeAgo } from './ClusterView';
import type { Node } from './db';
import { Composer } from './AgentView';
import { bySubject, ProposalGroup } from './ProposalView';
import { ChecksIcon, rebasable, ref, StatusLine } from './PullRequestList';
import { clusterKey, mergeBy, type Proposal, type PullRequest } from './shapes';
import { parseDiff } from './surfaces';
import { Button, DiffStat, diffFiles, Icon, t3 } from './t3';

// The conversations of one PR about one file, by the line each is on. `path` is '' for those on no
// file: top-level comments and failed checks.
export type Section = { pr: Node<PullRequest>; path: string; groups: Node<Proposal>[][] };

// A review comment's file, else the first file its proposal changes.
function pathOf(proposal: Node<Proposal>): string {
  return proposal.node('comment')?.str('path') || diffFiles(proposal.node('patch')?.str('diff') ?? '')[0]?.path || '';
}

// The line a proposal's review comment is on, or 0.
const lineOf = (proposal: Node<Proposal>) => proposal.node('comment')?.num('line') ?? 0;

export function sectionsOf(pr: Node<PullRequest>): Section[] {
  const byPath = new Map<string, Node<Proposal>[][]>();
  for (const group of bySubject(pr.list('proposals'))) {
    const path = pathOf(group[0]);
    byPath.set(path, [...(byPath.get(path) ?? []), group]);
  }
  return [...byPath]
    .sort(([a], [b]) => (a === '') !== (b === '') ? (a === '' ? 1 : -1) : a.localeCompare(b))
    .map(([path, groups]) => ({
      pr,
      path,
      groups: groups.sort((a, b) => lineOf(a[0]) - lineOf(b[0])),
    }));
}

export const anchor = (section: Section) => `${section.pr.str('url')}:${section.path}`;

function label(path: string): string {
  return path ? path.split('/').slice(-2).join('/') : 'Conversation';
}

const MAIN = 'cluster-review-file';

type Props = Parameters<typeof ClusterView>[0] & {
  // Starts an agent on the message, which reads the cluster through gem's MCP server. Resolves
  // once the agent started.
  onMessage?: (message: string) => Promise<unknown>;
};

// The open cluster on a wide screen, as a code review: a sidebar of the cluster's PRs and the files
// their conversations are on, and the chosen file's diff with each conversation inside it, after
// the line its review comment is on.
export function ClusterReview(props: Props) {
  const { cluster, onOpenAgent, busy, onRestart, onRebase, onRebaseCluster, onMerge, onMessage } = props;
  const pullRequests = cluster.list('pullRequests');
  const sections = pullRequests.flatMap(sectionsOf);
  const [chosen, setChosen] = useState<string>();
  // Approving or dismissing the last conversation on a file removes its section.
  const section = sections.find((s) => anchor(s) === chosen) ?? sections[0];
  const key = clusterKey(cluster);
  const restarting = busy?.has(key);
  const merging = busy?.has(`merge ${key}`);
  const updatedAt = cluster.time('updatedAt');
  return (
    <View style={styles.page}>
      <View style={styles.sidebar}>
        <Text
          style={[
            styles.title,
            mergeBy(cluster) === 'MERGE_BY_APPROVAL' && { color: '#4ADE80' },
            mergeBy(cluster) === 'MERGE_BY_ADMIN' && { color: t3.warning },
          ]}
        >
          {cluster.str('name')}
        </Text>
        <Text style={styles.meta}>
          {updatedAt && timeAgo(updatedAt)}
        </Text>
        <View style={styles.row}>
          <RebaseClusterButton cluster={cluster} busy={busy} onRebaseCluster={onRebaseCluster} />
          <Button
            label={restarting ? 'Restarting…' : 'Restart investigation'}
            disabled={restarting || merging}
            onPress={() => onRestart(cluster)}
          />
        </View>
        <MergeBanner cluster={cluster} busy={busy} onMerge={onMerge} stacked />
        <View style={styles.tree}>
          {pullRequests.map((pr) => {
            const url = pr.str('url');
            const failed = pr.node('checks')?.num('failed') ?? 0;
            const proposals = pr.list('proposals').length;
            return (
            <View key={url}>
              <Pressable style={styles.pr} onPress={() => Linking.openURL(url)}>
                <ChecksIcon pr={pr} size={12} />
                <Text numberOfLines={1} style={styles.ref}>
                  {ref(pr)}
                </Text>
                {failed > 0 && <Text style={styles.failed}>{failed} failed</Text>}
                {rebasable(pr) && (
                  <Button
                    label="Rebase"
                    disabled={pr.bool('rebasing') || pr.bool('rebased') || busy?.has(url)}
                    onPress={() => onRebase(url)}
                  />
                )}
                {proposals > 0 && <Text style={styles.count}>{proposals}</Text>}
              </Pressable>
              <StatusLine pr={pr} agents={cluster.list('agents')} />
              {sections.filter((s) => s.pr === pr).map((s) => (
                <Pressable
                  key={s.path}
                  style={[styles.file, section && anchor(s) === anchor(section) && styles.fileOn]}
                  onPress={() => {
                    setChosen(anchor(s));
                    // A file opens at its top, not where the last one was scrolled to.
                    document.getElementById(MAIN)?.scrollIntoView({ block: 'start' });
                  }}
                >
                  <Icon icon={s.path ? FileCode : MessagesSquare} size={12} />
                  <Text numberOfLines={1} style={styles.path}>
                    {label(s.path)}
                  </Text>
                  <Text style={styles.meta}>{s.groups.length}</Text>
                </Pressable>
              ))}
            </View>
            );
          })}
        </View>
        <ClusterAgents agents={cluster.list('agents')} onOpenAgent={onOpenAgent} />
        {onMessage && <Composer placeholder="Ask an agent about this cluster…" onSend={onMessage} />}
      </View>
      <View nativeID={MAIN} style={styles.main}>
        {section ? (
          <FileView key={anchor(section)} section={section} {...props} />
        ) : (
          <Text style={styles.meta}>Nothing waits on you.</Text>
        )}
      </View>
    </View>
  );
}

// One file's diff on its PR with the section's conversations inside it, each after the line its
// comment's own hunk ends on. Conversations on no line of the diff, or on a deleted one, come before
// it; without a diff, they are all there is.
function FileView({ section, busy, onApprove, onDismiss, onInvestigate }: { section: Section } & Props) {
  const file = section.pr.list('files').find((f) => f.str('path') === section.path);
  const patch = file?.str('patch') ?? '';
  // GitHub's hunks for the file come without a file header.
  const lines = parseDiff(`--- a/${section.path}\n+++ b/${section.path}\n${patch}`)[0]?.lines ?? [];
  const placed = new Map<number, Node<Proposal>[][]>();
  const unplaced: Node<Proposal>[][] = [];
  for (const group of section.groups) {
    const comment = group[0].node('comment');
    const line = comment?.num('line') ?? 0;
    // The commented line ends the hunk, after which only "\ No newline" markers can follow.
    const last = comment?.str('diffHunk').split('\n').filter((l) => !l.startsWith('\\')).at(-1) ?? '';
    const at = lines.findIndex((l) => l.kind !== 'del' && l.number === line && l.text === last.slice(1));
    if (line && !last.startsWith('-') && at >= 0) placed.set(at, [...(placed.get(at) ?? []), group]);
    else unplaced.push(group);
  }
  const thread = (group: Node<Proposal>[], inline: boolean) => (
    <ProposalGroup
      key={group[0].str('id')}
      proposals={group}
      inline={inline}
      anchored={!inline}
      busy={busy}
      onApprove={onApprove}
      onDismiss={onDismiss}
      onInvestigate={onInvestigate}
    />
  );
  return (
    <View style={styles.fileView}>
      <View style={styles.bar}>
        <Text style={styles.barRef}>{ref(section.pr)}</Text>
        <Text numberOfLines={1} style={styles.barPath}>
          {section.path || 'Conversation'}
        </Text>
        {lines.length > 0 && (
          <DiffStat
            added={lines.filter((l) => l.kind === 'add').length}
            removed={lines.filter((l) => l.kind === 'del').length}
          />
        )}
        <View style={{ flex: 1 }} />
        <Text style={styles.meta}>
          {section.groups.length} {section.groups.length === 1 ? 'thread' : 'threads'} waiting on you
        </Text>
      </View>
      {unplaced.map((group) => thread(group, false))}
      {lines.length > 0 && (
        <View style={styles.code}>
          {lines.map((line, i) => (
            <Fragment key={i}>
              {line.kind === 'hunk' ? (
                <Text style={styles.hunk}>
                  {'  ⋯  '}
                  {line.text}
                </Text>
              ) : (
                <View style={[styles.line, line.kind === 'add' && styles.add, line.kind === 'del' && styles.del]}>
                  <Text style={styles.number}>{line.old ?? ''}</Text>
                  <Text style={styles.number}>{line.number ?? ''}</Text>
                  <Text
                    style={[
                      styles.text,
                      line.kind === 'add' && { color: '#B7F5D8' },
                      line.kind === 'del' && { color: '#FFC9C9' },
                    ]}
                  >
                    {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ' '} {line.text}
                  </Text>
                </View>
              )}
              {placed.has(i) && (
                <View style={styles.inline}>{placed.get(i)!.map((group) => thread(group, true))}</View>
              )}
            </Fragment>
          ))}
        </View>
      )}
    </View>
  );
}

const GUTTER = 44;

const styles = StyleSheet.create({
  page: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 24,
  },
  // Stays in view while the file scrolls.
  sidebar: {
    position: 'sticky',
    top: 0,
    width: 290,
    flexShrink: 0,
    gap: 12,
  } as object,
  main: {
    flex: 1,
    minWidth: 0,
  },
  title: {
    color: t3.foreground,
    fontSize: 16,
    fontWeight: '600',
    lineHeight: 21,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  tree: {
    gap: 6,
  },
  pr: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingVertical: 4,
  },
  ref: {
    flex: 1,
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  failed: {
    color: t3.deletion,
    fontSize: 11,
  },
  count: {
    color: t3.warning,
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  file: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingVertical: 4,
    paddingLeft: 18,
    paddingRight: 6,
    borderRadius: 6,
  },
  fileOn: {
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
  },
  path: {
    flex: 1,
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  meta: {
    color: t3.faint,
    fontSize: 12,
  },
  fileView: {
    gap: 12,
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingBottom: 10,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
  },
  barRef: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  barPath: {
    flexShrink: 1,
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 14,
    fontWeight: '600',
  },
  code: {
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: '#0D1117',
    overflow: 'hidden',
    paddingBottom: 4,
  },
  hunk: {
    color: 'rgba(130, 170, 255, 0.7)',
    backgroundColor: 'rgba(55, 103, 232, 0.08)',
    fontFamily: t3.mono,
    fontSize: 12,
    lineHeight: 22,
    paddingHorizontal: 12,
  },
  line: {
    flexDirection: 'row',
  },
  add: {
    backgroundColor: 'rgba(16, 185, 129, 0.12)',
  },
  del: {
    backgroundColor: 'rgba(242, 109, 109, 0.12)',
  },
  number: {
    width: GUTTER,
    paddingRight: 10,
    textAlign: 'right',
    color: 'rgba(208, 207, 212, 0.28)',
    fontFamily: t3.mono,
    fontSize: 12,
    lineHeight: 20,
    userSelect: 'none',
  } as object,
  text: {
    flex: 1,
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
    lineHeight: 20,
    whiteSpace: 'pre-wrap',
  } as object,
  // Threads start past both gutters, under the code.
  inline: {
    gap: 8,
    marginVertical: 8,
    marginLeft: GUTTER * 2,
    marginRight: 12,
  },
});
