import { useState, useEffect } from "react";
import { api } from "../lib/api";
import { useTimeStore } from "../lib/time/store";

export type SystemMetrics = {
    timestamp: string;
    active_connections: number;
    queries_per_second: number;
    avg_query_time_ms: number;
    storage_size_bytes: number;
    memory_usage_bytes: number;
    cache_hit_rate: number;
    error_rate: number;
    uptime_seconds: number;
};

export type QueryStats = {
    total_queries: number;
    successful_queries: number;
    avg_execution_time_ms: number;
    success_rate: number;
    cache_hit_rate: number;
    queries_per_second: number;
};

export type Alert = {
    id: string;
    severity: "Info" | "Warning" | "Error" | "Critical";
    title: string;
    message: string;
    timestamp: string;
    resolved: boolean;
};

export type DashboardData = {
    system_metrics: SystemMetrics;
    query_stats: QueryStats;
    active_alerts: Alert[];
};

type MetricsEnvelope = {
    data: DashboardData;
};

const EMPTY_DASHBOARD: DashboardData = {
    system_metrics: {
        timestamp: new Date().toISOString(),
        active_connections: 0,
        queries_per_second: 0,
        avg_query_time_ms: 0,
        storage_size_bytes: 0,
        memory_usage_bytes: 0,
        cache_hit_rate: 0,
        error_rate: 0,
        uptime_seconds: 0,
    },
    query_stats: {
        total_queries: 0,
        successful_queries: 0,
        avg_execution_time_ms: 0,
        success_rate: 1,
        cache_hit_rate: 0,
        queries_per_second: 0,
    },
    active_alerts: [],
};

export function useMetrics(selectedDb?: string) {
    const [data, setData] = useState<DashboardData>(EMPTY_DASHBOARD);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const addSnapshot = useTimeStore((s) => s.addSnapshot);
    const currentTime = useTimeStore((s) => s.currentTime);
    const history = useTimeStore((s) => s.history);
    const isPaused = useTimeStore((s) => s.isPaused);

    const fetchMetrics = async () => {
        if (isPaused) return;
        try {
            setLoading(true);
            const res = await api<MetricsEnvelope>("/api/metrics", { database: selectedDb ?? null });
            setData(res.data ?? EMPTY_DASHBOARD);
            addSnapshot(res.data ?? EMPTY_DASHBOARD);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : "Failed to fetch metrics");
        } finally {
            setLoading(false);
        }
    };

    useEffect(() => {
        void fetchMetrics();
        const interval = setInterval(() => {
            void fetchMetrics();
        }, 2000);
        return () => clearInterval(interval);
    }, [isPaused, selectedDb]);

    if (currentTime !== null && history.length > 0) {
        const closest = history.reduce((prev, curr) => {
            return Math.abs(curr.timestamp - currentTime) < Math.abs(prev.timestamp - currentTime) ? curr : prev;
        });
        return {
            data: (closest.metrics as DashboardData) ?? EMPTY_DASHBOARD,
            loading: false,
            error,
            refresh: fetchMetrics,
            isHistory: true,
        };
    }

    return { data, loading, error, refresh: fetchMetrics, isHistory: false };
}
