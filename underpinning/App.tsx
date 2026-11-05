import '@xyflow/react/dist/style.css';

import { Background, Controls, type Edge, type Node, ReactFlow } from '@xyflow/react';
import { StatusBar } from 'expo-status-bar';
import { StyleSheet, View } from 'react-native';

const nodes: Node[] = [
  { id: 'frontend', type: 'input', position: { x: 0, y: 0 }, data: { label: 'Frontend' } },
  { id: 'backend', type: 'output', position: { x: 0, y: 160 }, data: { label: 'Backend' } },
];

const edges: Edge[] = [{ id: 'frontend-backend', source: 'frontend', target: 'backend' }];

export default function App() {
  return (
    <View style={styles.container}>
      <ReactFlow defaultNodes={nodes} defaultEdges={edges} fitView>
        <Background />
        <Controls />
      </ReactFlow>
      <StatusBar style="auto" />
    </View>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1 },
});
