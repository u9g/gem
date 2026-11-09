import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import { timeAgo } from './ClusterView';
import { Icon, t3 } from './t3';
import type { PullRequest } from './view-types';

// The pieces of a GitHub-style timeline: a rail down the left with a badge per event, each event a
// line of text with what hangs under it beside the rail. ClusterTimeline and Activity are made of
// them.

export const ref = (pr: PullRequest) => `${pr.repository?.name}#${pr.number}`;

// The rail runs through the badges' centers, under them.
export function Timeline({ children }: { children: ReactNode }) {
  return (
    <View style={styles.timeline}>
      <View style={styles.rail} />
      {children}
    </View>
  );
}

// One line of the timeline: a badge on the rail, then what happened. `children` hang under it,
// beside the rail.
export function Event({
  icon,
  tone,
  children,
  trailing,
  body,
}: {
  icon: LucideIcon;
  tone?: string;
  children: ReactNode;
  trailing?: ReactNode;
  body?: ReactNode;
}) {
  return (
    <View style={styles.event}>
      <View style={styles.eventLine}>
        <Badge icon={icon} tone={tone} />
        <Text style={styles.eventText}>{children}</Text>
        {trailing}
      </View>
      {body && <View style={styles.eventBody}>{body}</View>}
    </View>
  );
}

export function Badge({ icon, tone }: { icon: LucideIcon; tone?: string }) {
  return (
    <View style={[styles.badge, tone !== undefined && { borderColor: tone }]}>
      <Icon icon={icon} size={14} color={tone ?? t3.muted} />
    </View>
  );
}

// A comment or reply under an event, as GitHub boxes one: who and what in its head, the body under.
export function Card({ head, children }: { head: ReactNode; children: ReactNode }) {
  return (
    <View style={styles.card}>
      <View style={styles.cardHead}>{head}</View>
      <View style={styles.cardBody}>{children}</View>
    </View>
  );
}

export const Strong = ({ children }: { children: ReactNode }) => <Text style={styles.strong}>{children}</Text>;
export const Mono = ({ children }: { children: ReactNode }) => <Text style={styles.mono}>{children}</Text>;
export const Dim = ({ children }: { children: ReactNode }) => <Text style={styles.when}>{children}</Text>;
export const When = ({ at }: { at?: number }) =>
  at === undefined ? null : <Text style={styles.when}> · {timeAgo(new Date(at))}</Text>;

export const BADGE = 28;

const styles = StyleSheet.create({
  timeline: {
    gap: 14,
  },
  rail: {
    position: 'absolute',
    top: 8,
    bottom: 8,
    left: BADGE / 2 - 1,
    width: 2,
    backgroundColor: t3.border,
  },
  event: {
    gap: 10,
  },
  eventLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    minHeight: BADGE,
  },
  eventText: {
    flex: 1,
    color: t3.muted,
    fontSize: 13,
    lineHeight: 19,
  },
  eventBody: {
    marginLeft: BADGE + 10,
    gap: 10,
  },
  badge: {
    width: BADGE,
    height: BADGE,
    borderRadius: BADGE / 2,
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: t3.border,
    backgroundColor: '#222C37',
  },
  strong: {
    color: t3.foreground,
    fontWeight: '600',
  },
  mono: {
    color: t3.foreground,
    fontFamily: t3.mono,
    fontSize: 12,
    paddingHorizontal: 4,
    borderRadius: 4,
    backgroundColor: t3.input,
  },
  when: {
    color: t3.faint,
  },
  card: {
    borderRadius: 12,
    borderWidth: 1,
    borderColor: t3.border,
    overflow: 'hidden',
  },
  cardHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderBottomWidth: 1,
    borderBottomColor: t3.border,
    backgroundColor: 'rgba(255, 255, 255, 0.03)',
  },
  cardBody: {
    padding: 12,
  },
});
