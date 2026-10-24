import { CircleCheck, CircleDashed, CircleX } from 'lucide-react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';

import type { PullRequest } from './gen/gem_pb';
import { Icon, t3 } from './t3';

// A cluster's PRs as a bordered list like T3 Code's changed-files panel: one row per PR with its
// checks, repo#number, title, review state and how many proposals wait on it. A row opens the PR.
export function PullRequestList({ pullRequests }: { pullRequests: PullRequest[] }) {
  return (
    <View style={styles.list}>
      {pullRequests.map((pr, i) => (
        <Pressable
          key={pr.url}
          style={[styles.row, i > 0 && styles.rowBorder]}
          onPress={() => Linking.openURL(pr.url)}
        >
          <ChecksIcon pr={pr} />
          <Text style={styles.ref}>
            {pr.repository?.name}#{pr.number}
          </Text>
          <Text numberOfLines={1} style={styles.title}>
            {pr.title}
          </Text>
          <Text style={[styles.review, pr.approved && { color: t3.addition }]}>
            {pr.approved ? 'Approved' : 'Review required'}
          </Text>
          <View style={styles.countSlot}>
            {pr.proposals.length > 0 && <Text style={styles.count}>{pr.proposals.length}</Text>}
          </View>
        </Pressable>
      ))}
    </View>
  );
}

// Checks on the head commit: failed, passed, or still running.
function ChecksIcon({ pr }: { pr: PullRequest }) {
  if (pr.checksFailing) return <Icon icon={CircleX} size={14} color={t3.deletion} />;
  if (pr.checksPassing) return <Icon icon={CircleCheck} size={14} color={t3.addition} />;
  return <Icon icon={CircleDashed} size={14} color={t3.warning} />;
}

const styles = StyleSheet.create({
  list: {
    borderRadius: 8,
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.025)',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    height: 36,
    paddingHorizontal: 6,
  },
  rowBorder: {
    borderTopWidth: 1,
    borderTopColor: t3.border,
  },
  ref: {
    width: 120,
    color: t3.muted,
    fontFamily: t3.mono,
    fontSize: 12,
  },
  title: {
    flex: 1,
    color: t3.foreground,
    fontSize: 13,
  },
  review: {
    color: t3.muted,
    fontSize: 12,
  },
  countSlot: {
    width: 24,
    alignItems: 'flex-end',
  },
  count: {
    minWidth: 18,
    paddingHorizontal: 5,
    borderRadius: 6,
    color: '#8FA8F5',
    backgroundColor: 'rgba(55, 103, 232, 0.16)',
    fontSize: 11,
    lineHeight: 18,
    textAlign: 'center',
    fontVariant: ['tabular-nums'],
  },
});
