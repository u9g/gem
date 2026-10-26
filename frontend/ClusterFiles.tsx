import { FileCode, MessagesSquare } from 'lucide-react';
import type { ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { Proposal, PullRequest, PullRequestCluster } from './gen/gem_pb';
import { bySubject, ProposalGroup } from './ProposalView';
import { ChecksIcon } from './PullRequestList';
import { diffFiles, Icon, t3 } from './t3';

// The conversations of one PR about one file, by the line each is on. `path` is '' for those on no
// file: top-level comments and failed checks.
type Section = { pr: PullRequest; path: string; groups: Proposal[][] };

// A review comment's file, else the first file its proposal changes.
function pathOf(proposal: Proposal): string {
  return proposal.comment?.path || diffFiles(proposal.patch?.diff ?? '')[0]?.path || '';
}

function sectionsOf(pr: PullRequest): Section[] {
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

const anchor = (section: Section) => `${section.pr.url}:${section.path}`;

function label(path: string): string {
  return path ? path.split('/').slice(-2).join('/') : 'Conversation';
}

// The cluster's PRs, each with the files its conversations are on; a file scrolls to its section.
function FileTree({ cluster }: { cluster: PullRequestCluster }) {
  return (
    <View style={styles.tree}>
      {cluster.pullRequests.map((pr) => (
        <View key={pr.url}>
          <View style={styles.pr}>
            <ChecksIcon pr={pr} size={12} />
            <Text numberOfLines={1} style={styles.ref}>
              {pr.repository?.name}#{pr.number}
            </Text>
            {pr.proposals.length > 0 && <Text style={styles.count}>{pr.proposals.length}</Text>}
          </View>
          {sectionsOf(pr).map((section) => (
            <Pressable
              key={section.path}
              style={styles.file}
              onPress={() => document.getElementById(anchor(section))?.scrollIntoView({ behavior: 'smooth', block: 'start' })}
            >
              <Icon icon={section.path ? FileCode : MessagesSquare} size={12} />
              <Text numberOfLines={1} style={styles.path}>
                {label(section.path)}
              </Text>
              <Text style={styles.meta}>{section.groups.length}</Text>
            </Pressable>
          ))}
        </View>
      ))}
    </View>
  );
}

// `children`, the open cluster, beside its file tree, which stays in view while it scrolls.
export function WithFiles({ cluster, children }: { cluster: PullRequestCluster; children: ReactNode }) {
  return (
    <View style={styles.split}>
      <View style={styles.sidebar}>
        <FileTree cluster={cluster} />
      </View>
      {children}
    </View>
  );
}

// Every conversation waiting on the user under the PR and file it is on, each beside its code.
export function FileSections({
  cluster,
  busy,
  onApprove,
  onDismiss,
  onInvestigate,
}: {
  cluster: PullRequestCluster;
  busy?: ReadonlySet<string>;
  onApprove: (id: string, reply: string, followUp: string, diff: string) => void;
  onDismiss: (id: string) => void;
  onInvestigate: (id: string) => void;
}) {
  return cluster.pullRequests.flatMap(sectionsOf).map((section) => (
    <View key={anchor(section)} nativeID={anchor(section)} style={styles.section}>
      <View style={styles.sectionHeader}>
        <Text style={styles.sectionRef}>
          {section.pr.repository?.name}#{section.pr.number}
        </Text>
        <Text numberOfLines={1} style={styles.sectionPath}>
          {section.path || 'Conversation'}
        </Text>
      </View>
      {section.groups.map((group) => (
        <ProposalGroup
          key={group[0].id}
          anchored
          proposals={group}
          busy={busy}
          onApprove={onApprove}
          onDismiss={onDismiss}
          onInvestigate={onInvestigate}
        />
      ))}
    </View>
  ));
}

const styles = StyleSheet.create({
  split: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 28,
  },
  sidebar: {
    position: 'sticky',
    top: 0,
    width: 260,
    flexShrink: 0,
  } as object,
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
  count: {
    color: t3.warning,
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  file: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingVertical: 3,
    paddingLeft: 18,
    borderRadius: 6,
  },
  path: {
    flex: 1,
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  meta: {
    color: t3.faint,
    fontSize: 11,
  },
  section: {
    gap: 10,
    scrollMarginTop: 16,
  } as object,
  sectionHeader: {
    flexDirection: 'row',
    alignItems: 'baseline',
    gap: 8,
    marginTop: 8,
  },
  sectionRef: {
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  sectionPath: {
    flex: 1,
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 13,
    fontWeight: '600',
  },
});
