import { timestampDate } from '@bufbuild/protobuf/wkt';
import { FileCode, MessagesSquare } from 'lucide-react';
import { Fragment, useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import { type ClusterView, MergeBanner, timeAgo } from './ClusterView';
import { MergeBy, type Proposal, type PullRequest } from './gen/gem_pb';
import { Composer } from './InvestigationView';
import { bySubject, ProposalGroup } from './ProposalView';
import { ChecksIcon, StatusLine } from './PullRequestList';
import { parseDiff } from './surfaces';
import { Button, DiffStat, diffFiles, Icon, t3 } from './t3';

// The conversations of one PR about one file, by the line each is on. `path` is '' for those on no
// file: top-level comments and failed checks.
export type Section = { pr: PullRequest; path: string; groups: Proposal[][] };

// A review comment's file, else the first file its proposal changes.
function pathOf(proposal: Proposal): string {
  return proposal.comment?.path || diffFiles(proposal.patch?.diff ?? '')[0]?.path || '';
}

export function sectionsOf(pr: PullRequest): Section[] {
  const byPath = new Map<string, Proposal[][]>();
  for (const group of bySubject(pr.proposals)) {
    const path = pathOf(group[0]);
    byPath.set(path, [...(byPath.get(path) ?? []), group]);
  }
  return [...byPath]
    .sort(([a], [b]) => (a === '') !== (b === '') ? (a === '' ? 1 : -1) : a.localeCompare(b))
    .map(([path, groups]) => ({
      pr,
      path,
      groups: groups.sort((a, b) => (a[0].comment?.line ?? 0) - (b[0].comment?.line ?? 0)),
    }));
}

export const anchor = (section: Section) => `${section.pr.url}:${section.path}`;

function label(path: string): string {
  return path ? path.split('/').slice(-2).join('/') : 'Conversation';
}

const MAIN = 'cluster-review-file';

type Props = Parameters<typeof ClusterView>[0];

// The open cluster on a wide screen, as a code review: a sidebar of the cluster's PRs and the files
// their conversations are on, and the chosen file's diff with each conversation inside it, after
// the line its review comment is on.
export function ClusterReview(props: Props) {
  const { cluster, busy, onRestart, onRebase, onMerge, onMessage } = props;
  const sections = cluster.pullRequests.flatMap(sectionsOf);
  const [chosen, setChosen] = useState<string>();
  // Approving or dismissing the last conversation on a file removes its section.
  const section = sections.find((s) => anchor(s) === chosen) ?? sections[0];
  const restarting = busy?.has(cluster.pullRequests[0].url);
  const merging = busy?.has(`merge ${cluster.pullRequests[0].url}`);
  return (
    <View style={styles.page}>
      <View style={styles.sidebar}>
        <Text
          style={[
            styles.title,
            cluster.mergeBy === MergeBy.APPROVAL && { color: '#4ADE80' },
            cluster.mergeBy === MergeBy.ADMIN && { color: t3.warning },
          ]}
        >
          {cluster.name}
        </Text>
        <View style={styles.row}>
          <Text style={[styles.meta, { flex: 1 }]}>
            {cluster.updatedAt && timeAgo(timestampDate(cluster.updatedAt))}
          </Text>
          <Button
            label={restarting ? 'Restarting…' : 'Restart investigation'}
            disabled={restarting || merging}
            onPress={() => onRestart(cluster)}
          />
        </View>
        <MergeBanner cluster={cluster} busy={busy} onMerge={onMerge} stacked />
        <View style={styles.tree}>
          {cluster.pullRequests.map((pr) => (
            <View key={pr.url}>
              <Pressable style={styles.pr} onPress={() => Linking.openURL(pr.url)}>
                <ChecksIcon pr={pr} size={12} />
                <Text numberOfLines={1} style={styles.ref}>
                  {pr.repository?.name}#{pr.number}
                </Text>
                {pr.checks && pr.checks.failed > 0 && <Text style={styles.failed}>{pr.checks.failed} failed</Text>}
                {(pr.behind || pr.rebase) && (
                  <Button
                    label="Rebase"
                    disabled={pr.rebase !== undefined || busy?.has(pr.url)}
                    onPress={() => onRebase(pr.url)}
                  />
                )}
                {pr.proposals.length > 0 && <Text style={styles.count}>{pr.proposals.length}</Text>}
              </Pressable>
              <StatusLine pr={pr} />
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
          ))}
        </View>
        {onMessage && <Composer placeholder="Ask an agent about this cluster…" onSend={onMessage} />}
      </View>
      <View nativeID={MAIN} style={styles.main}>
        {/* Every file stays mounted, so switching files keeps the drafts in each. */}
        {sections.map((s) => (
          <View key={anchor(s)} style={anchor(s) !== anchor(section) && styles.hidden}>
            <FileView section={s} {...props} />
          </View>
        ))}
        {!section && <Text style={styles.meta}>Nothing waits on you.</Text>}
      </View>
    </View>
  );
}

// One file's diff on its PR with the section's conversations inside it, each after the line its
// comment's own hunk ends on. Conversations on no line of the diff, or on a deleted one, come before
// it; without a diff, they are all there is.
function FileView({ section, busy, onApprove, onDismiss, onInvestigate }: { section: Section } & Props) {
  const patch = section.pr.files.find((f) => f.path === section.path)?.patch ?? '';
  // GitHub's hunks for the file come without a file header.
  const lines = parseDiff(`--- a/${section.path}\n+++ b/${section.path}\n${patch}`)[0]?.lines ?? [];
  const placed = new Map<number, Proposal[][]>();
  const unplaced: Proposal[][] = [];
  for (const group of section.groups) {
    const comment = group[0].comment;
    const last = comment?.diffHunk.split('\n').at(-1) ?? '';
    const at = lines.findIndex((l) => l.kind !== 'del' && l.number === comment?.line && l.text === last.slice(1));
    if (comment?.line && !last.startsWith('-') && at >= 0) placed.set(at, [...(placed.get(at) ?? []), group]);
    else unplaced.push(group);
  }
  const thread = (group: Proposal[], inline: boolean) => (
    <ProposalGroup
      key={group[0].id}
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
        <Text style={styles.barRef}>
          {section.pr.repository?.name}#{section.pr.number}
        </Text>
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
  hidden: {
    display: 'none',
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
