
import React, { useCallback } from 'react';
import {
    ReactFlow,
    MiniMap,
    Controls,
    Background,
    useNodesState,
    useEdgesState,
    addEdge,
    Connection,
    Edge,
    Node,
    BackgroundVariant,
} from '@xyflow/react';

import '@xyflow/react/dist/style.css';

interface GraphViewProps {
    documents: any[];
}

// Helper to convert documents to nodes
const createGraphData = (docs: any[]) => {
    const nodes: Node[] = [];
    const edges: Edge[] = [];

    // Grid layout parameters
    const startX = 50;
    const startY = 50;
    const gapX = 250;
    const gapY = 150;
    const cols = 3;

    docs.forEach((doc, index) => {
        const row = Math.floor(index / cols);
        const col = index % cols;

        const nodeId = doc._id || doc.id || `node-${index}`;
        const label = doc.name || doc.title || doc._id || `Doc ${index}`;

        nodes.push({
            id: nodeId,
            position: { x: startX + col * gapX, y: startY + row * gapY },
            data: { label: label, content: doc },
            type: 'default', // or custom
            style: {
                border: '1px solid #777',
                padding: '10px',
                borderRadius: '5px',
                background: 'var(--card)',
                color: 'var(--card-foreground)',
                minWidth: '150px'
            },
        });

        // Try to find relationships (basic example: look for fields ending in _id)
        Object.keys(doc).forEach(key => {
            if (key.endsWith('_id') && doc[key]) {
                const targetId = doc[key];
                // Add edge if target exists in our current set (simplified)
                // In a real app we'd need to know if that ID exists or fetch it.
                // For now, we only link if we see the logic.
                edges.push({
                    id: `${nodeId}-${targetId}`,
                    source: nodeId,
                    target: String(targetId),
                    animated: true,
                    style: { stroke: 'var(--primary)' }
                });
            }
        });
    });

    return { nodes, edges };
};

export function GraphView({ documents }: GraphViewProps) {
    const { nodes: initialNodes, edges: initialEdges } = createGraphData(documents);

    const [nodes, setNodes, onNodesChange] = useNodesState(initialNodes);
    const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);

    // Update graph when documents change
    React.useEffect(() => {
        const { nodes: newNodes, edges: newEdges } = createGraphData(documents);
        setNodes(newNodes);
        setEdges(newEdges);
    }, [documents, setNodes, setEdges]);

    const onConnect = useCallback(
        (params: Connection) => setEdges((eds) => addEdge(params, eds)),
        [setEdges],
    );

    return (
        <div style={{ width: '100%', height: '100%' }}>
            <ReactFlow
                nodes={nodes}
                edges={edges}
                onNodesChange={onNodesChange}
                onEdgesChange={onEdgesChange}
                onConnect={onConnect}
                fitView
            >
                <Controls />

                <MiniMap nodeStrokeColor="#666" nodeColor="#aaa" />

                <Background variant={BackgroundVariant.Dots} gap={12} size={1} />
            </ReactFlow>
        </div>
    );
}
