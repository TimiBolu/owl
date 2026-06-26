import { useEffect, useState } from "react";
import { api } from "../lib/api";

export type ClusterNodeMetrics = {
    cpu: number;
    ram: number;
};

export type ClusterNode = {
    id: string;
    label: string;
    role: string;
    status: "healthy" | "warning" | "error";
    shardCount: number;
    metrics: ClusterNodeMetrics;
};

export type ClusterEdge = {
    id: string;
    source: string;
    target: string;
};

type ClusterTopologyResponse = {
    data: {
        nodes: ClusterNode[];
        edges: ClusterEdge[];
    };
};

export function useClusterTopology(selectedDb?: string) {
    const [nodes, setNodes] = useState<ClusterNode[]>([]);
    const [edges, setEdges] = useState<ClusterEdge[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    async function refresh() {
        try {
            setLoading(true);
            const res = await api<ClusterTopologyResponse>("/api/cluster/topology", { database: selectedDb ?? null });
            setNodes(res.data?.nodes ?? []);
            setEdges(res.data?.edges ?? []);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : "Failed to load cluster topology");
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        void refresh();
        const interval = setInterval(() => void refresh(), 2500);
        return () => clearInterval(interval);
    }, [selectedDb]);

    return { nodes, edges, loading, error, refresh };
}
