import '@xyflow/react/dist/style.css';

import { Background, Controls, type Edge, type Node, ReactFlow } from '@xyflow/react';
import { StatusBar } from 'expo-status-bar';
import { useMemo } from 'react';
import { StyleSheet, View } from 'react-native';

import { BackendContext, useBackend } from './backend';
import { Log } from './Log';
import { nodeTypes, statusColor } from './nodes';

const nodes: Node[] = [
  { id: 'frontend', type: 'frontend', position: { x: 0, y: 0 }, data: {} },
  { id: 'backend', type: 'backend', position: { x: 480, y: 0 }, data: {} },
];

export default function App() {
  const backend = useBackend();
  const { status } = backend;
  // The line between them shows whether the frontend is registered with the backend.
  const edges = useMemo<Edge[]>(
    () => [
      {
        id: 'frontend-backend',
        source: 'frontend',
        target: 'backend',
        animated: status === 'online',
        label: status,
        labelStyle: { fontFamily: 'system-ui, sans-serif', fontSize: 12 },
        style: { stroke: statusColor[status], strokeWidth: 2 },
      },
    ],
    [status],
  );

  return (
    <BackendContext.Provider value={backend}>
      <View style={styles.container}>
        <View style={styles.canvas}>
          <ReactFlow defaultNodes={nodes} edges={edges} nodeTypes={nodeTypes} fitView>
            <Background />
            <Controls />
          </ReactFlow>
        </View>
        <Log />
      </View>
      <StatusBar style="auto" />
    </BackendContext.Provider>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1, flexDirection: 'row' },
  canvas: { flex: 1 },
});
