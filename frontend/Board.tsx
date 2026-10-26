import { X } from 'lucide-react';
import { type ReactNode, useEffect } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { InvestigationRow, PhoneClusterRow, section, sections } from './ClusterRow';
import type { Investigation, PullRequestCluster } from './gen/gem_pb';
import { Icon, t3 } from './t3';

const COLUMN_MIN_WIDTH = 220;
const DRAWER_WIDTH = 640;

// The wide layout: one column per place a phone lists a cluster, left to right by what each waits
// on, after a column of agents when any run. The open cluster or agent, `children`, slides over the
// right edge, or over the whole board when `wide`; clicking beside it, the wide drawer's close
// button or pressing Escape closes it.
export function Board({
  clusters,
  investigations,
  openUrl,
  openInvestigation,
  onOpenCluster,
  onOpenInvestigation,
  onClose,
  wide = false,
  children,
}: {
  clusters: PullRequestCluster[];
  investigations: Investigation[];
  // The oldest PR's URL of the open cluster.
  openUrl?: string;
  openInvestigation?: string;
  onOpenCluster: (cluster: PullRequestCluster) => void;
  onOpenInvestigation: (id: string) => void;
  onClose: () => void;
  wide?: boolean;
  children?: ReactNode;
}) {
  useEffect(() => {
    if (!children) return;
    const close = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    addEventListener('keydown', close);
    return () => removeEventListener('keydown', close);
  }, [children, onClose]);

  return (
    <View style={styles.board}>
      <View style={styles.columns}>
        {investigations.length > 0 && (
          <Column name="Agents" count={investigations.length}>
            {investigations.map((i) => (
              <InvestigationRow
                key={i.id}
                investigation={i}
                selected={i.id === openInvestigation}
                onPress={() => onOpenInvestigation(i.id)}
              />
            ))}
          </Column>
        )}
        {sections.map((name) => {
          const listed = clusters.filter((cluster) => section(cluster) === name);
          return (
            <Column key={name} name={name} count={listed.length} urgent={name === 'Needs you'}>
              {listed.map((cluster) => (
                <PhoneClusterRow
                  key={cluster.pullRequests[0].url}
                  cluster={cluster}
                  board
                  selected={cluster.pullRequests[0].url === openUrl}
                  onPress={() => onOpenCluster(cluster)}
                />
              ))}
            </Column>
          );
        })}
      </View>
      {children && (
        <>
          <Pressable accessibilityLabel="Close" style={styles.scrim} onPress={onClose} />
          <View style={[styles.drawer, wide && styles.wide]}>
            <ScrollView contentContainerStyle={styles.drawerContent}>{children}</ScrollView>
            {/* Only a wide drawer covers the space beside it that closes it. */}
            {wide && (
              <Pressable accessibilityLabel="Close" style={styles.close} onPress={onClose}>
                <Icon icon={X} size={16} />
              </Pressable>
            )}
          </View>
        </>
      )}
    </View>
  );
}

function Column({ name, count, urgent, children }: { name: string; count: number; urgent?: boolean; children: ReactNode }) {
  return (
    <View style={styles.column}>
      <View style={styles.columnHead}>
        <Text style={styles.columnTitle}>{name}</Text>
        <Text style={styles.columnCount}>{count}</Text>
      </View>
      <ScrollView style={[styles.columnBody, urgent && styles.urgent]} contentContainerStyle={styles.columnContent}>
        {count > 0 ? children : <Text style={styles.empty}>Nothing here</Text>}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  board: {
    flex: 1,
  },
  // Columns share the width until each is at its minimum; past that, the row scrolls sideways.
  columns: {
    flex: 1,
    flexDirection: 'row',
    gap: 12,
    padding: 14,
    overflowX: 'auto',
  } as object,
  column: {
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 0,
    minWidth: COLUMN_MIN_WIDTH,
  },
  columnHead: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: 6,
    paddingBottom: 8,
  },
  columnTitle: {
    color: t3.foreground,
    fontSize: 13,
    fontWeight: '600',
  },
  columnCount: {
    color: t3.faint,
    fontSize: 12,
    fontVariant: ['tabular-nums'],
  },
  columnBody: {
    flex: 1,
    borderRadius: 14,
    backgroundColor: 'rgba(255, 255, 255, 0.02)',
  },
  columnContent: {
    padding: 8,
  },
  urgent: {
    backgroundColor: 'rgba(242, 109, 109, 0.05)',
  },
  empty: {
    color: t3.faint,
    fontSize: 12,
    textAlign: 'center',
    paddingVertical: 16,
  },
  scrim: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    left: 0,
    right: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.35)',
    cursor: 'default',
  } as object,
  drawer: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    right: 0,
    width: DRAWER_WIDTH,
    maxWidth: '100%',
    borderLeftWidth: 1,
    borderLeftColor: t3.border,
    backgroundColor: '#1D2630',
    shadowColor: '#000',
    shadowOpacity: 0.5,
    shadowRadius: 32,
  },
  wide: {
    width: '100%',
    borderLeftWidth: 0,
  },
  drawerContent: {
    padding: 24,
  },
  close: {
    position: 'absolute',
    top: 6,
    right: 6,
    padding: 2,
  },
});
