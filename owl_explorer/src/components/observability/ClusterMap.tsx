import { useCallback, useEffect, useMemo } from "react";
import {
    ReactFlow,
    Background,
    Controls,
    useNodesState,
    useEdgesState,
    addEdge,
    Connection,
    Edge,
    MarkerType,
    Node,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { ClusterNode } from "./ClusterNode";
import { useClusterTopology } from "@/hooks/useClusterTopology";

const nodeTypes = {
    clusterNode: ClusterNode,
};

export default function ClusterMap() {
    const { nodes: topologyNodes, edges: topologyEdges, loading, error } = useClusterTopology();

    const graph = useMemo(() => {
        const nodes: Node[] = topologyNodes.map((n, i) => ({
            id: n.id,
            type: "clusterNode",
            position: { x: 220 + i * 240, y: 140 },
            data: {
                label: n.label,
                role: n.role,
                status: n.status,
                metrics: n.metrics,
                shardCount: n.shardCount,
            },
        }));
        const edges: Edge[] = topologyEdges.map((e) => ({
            id: e.id,
            source: e.source,
            target: e.target,
            animated: true,
            style: { stroke: "var(--primary)", strokeWidth: 2 },
            markerEnd: { type: MarkerType.ArrowClosed },
        }));
        return { nodes, edges };
    }, [topologyNodes, topologyEdges]);

    const [nodes, setNodes, onNodesChange] = useNodesState(graph.nodes);
    const [edges, setEdges, onEdgesChange] = useEdgesState(graph.edges);

    useEffect(() => {
        setNodes(graph.nodes);
        setEdges(graph.edges);
    }, [graph, setNodes, setEdges]);

    const onConnect = useCallback(
        (params: Connection) => setEdges((eds) => addEdge(params, eds)),
        [setEdges],
    );

    if (error) {
        return <div className="p-4 text-sm text-destructive">Failed to load topology: {error}</div>;
    }

    if (loading && nodes.length === 0) {
        return <div className="p-4 text-sm text-muted-foreground">Loading cluster topology...</div>;
    }

    return (
        <div style={{ width: "100%", height: "100%" }} className="bg-muted/10">
            <ReactFlow
                nodes={nodes}
                edges={edges}
                onNodesChange={onNodesChange}
                onEdgesChange={onEdgesChange}
                onConnect={onConnect}
                nodeTypes={nodeTypes}
                fitView
                fitViewOptions={{ padding: 0.2 }}
            >
                <Background color="#888" gap={16} size={1} />
                <Controls showInteractive={false} />
            </ReactFlow>
        </div>
    );
}
