

import { Activity, Database, HardDrive, Cpu, Zap, AlertTriangle, CheckCircle, Info, Maximize2 } from "lucide-react";
import { Button } from "@/components/ui/button";

import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";



import { useMetrics } from "@/hooks/useMetrics";

import ClusterMap from "../observability/ClusterMap";
import { ProphecyCard } from "../observability/ProphecyCard";
import { ChaosToolbar } from "../observability/ChaosToolbar";




// Helper for formatting bytes if not available
function formatBytesLocal(bytes: number, decimals = 2) {
    if (bytes === 0) return '0 Bytes';
    const k = 1024;
    const dm = decimals < 0 ? 0 : decimals;
    const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + ' ' + sizes[i];
}

export function Dashboard() {
    const { data, loading, error } = useMetrics();
    const metrics = data.system_metrics;

    if (error) {
        return <div className="p-6 text-red-500">Error loading metrics: {error}</div>;
    }

    return (
        <div className="p-6 space-y-6 overflow-auto h-full">
            <div className="flex items-center justify-between">
                <div>
                    <h1 className="text-2xl font-bold tracking-tight">System Overview</h1>
                    <p className="text-muted-foreground">Monitor cluster health and performance metrics.</p>
                </div>
                <div className="flex items-center gap-2 bg-muted/20 px-3 py-1 rounded-full border">
                    <span className={`flex h-2 w-2 rounded-full ${loading ? 'bg-yellow-400' : 'bg-green-500'} animate-pulse`} />
                    <span className="text-sm font-medium">{loading ? 'Updating...' : 'System Online'}</span>
                    <span className="text-xs text-muted-foreground ml-2">Uptime: {Math.floor(metrics.uptime_seconds / 3600)}h</span>
                </div>
            </div>

            {/* Metrics Grid */}
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
                <MetricCard
                    title="Queries / Sec"
                    value={metrics.queries_per_second.toFixed(1)}
                    icon={Zap}
                    description="Throughput"
                    trend={metrics.queries_per_second > 40 ? "+High" : "Normal"}
                    trendUp={metrics.queries_per_second > 40}
                />
                <MetricCard
                    title="Avg Latency"
                    value={`${metrics.avg_query_time_ms.toFixed(2)}ms`}
                    icon={Activity}
                    description="Query Execution Time"
                    trend={metrics.avg_query_time_ms < 5 ? "Fast" : "Slow"}
                    trendUp={metrics.avg_query_time_ms < 5}
                />
                <MetricCard
                    title="Memory Usage"
                    value={formatBytesLocal(metrics.memory_usage_bytes)}
                    icon={HardDrive}
                    description="Zig Arena Allocator"
                    trend={`${(metrics.memory_usage_bytes / (1024 * 1024 * 1024) * 100).toFixed(1)}% of 1GB`}
                    trendUp={metrics.memory_usage_bytes < 1024 * 1024 * 800}
                />
                <MetricCard
                    title="Cache Hit Rate"
                    value={`${(metrics.cache_hit_rate * 100).toFixed(1)}%`}
                    icon={Database}
                    description="Query Cache Efficiency"
                    trend={metrics.cache_hit_rate > 0.9 ? "Excellent" : "Good"}
                    trendUp={metrics.cache_hit_rate > 0.8}
                />
            </div>


            {/* Main Visuals Row 1 */}
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-7 h-[500px]">
                <Card className="col-span-4 border-primary/20 bg-card/50 h-full flex flex-col">


                    <CardHeader>
                        <CardTitle className="flex items-center justify-between gap-2">
                            <div className="flex items-center gap-2">
                                <Activity className="h-4 w-4 text-primary" />
                                Cluster Topology (Live)
                            </div>
                            <div className="flex items-center gap-2">
                                {/* CHAOS MODE TRIGGER */}
                                <ChaosToolbar />
                                <Button
                                    variant="outline"
                                    size="sm"
                                    className="h-7 text-xs gap-1"
                                    onClick={() => window.dispatchEvent(new CustomEvent("owl:navigate", { detail: "war_room" }))}
                                >
                                    <Maximize2 className="h-3 w-3" />
                                    Expand
                                </Button>
                            </div>
                        </CardTitle>
                        <CardDescription>
                            Real-time view of cluster nodes and replication traffic.
                        </CardDescription>
                    </CardHeader>


                    <CardContent className="flex-1 p-0 relative overflow-hidden">
                        <ClusterMap />
                    </CardContent>
                </Card>

                <Card className="col-span-3 border-primary/20 bg-card/50 flex flex-col">
                    <CardHeader>
                        <CardTitle className="flex items-center gap-2">
                            <HardDrive className="h-4 w-4 text-primary" />
                            Storage Usage
                        </CardTitle>
                        <CardDescription>
                            Total data size on disk.
                        </CardDescription>
                    </CardHeader>
                    <CardContent>
                        <div className="h-[200px] flex items-center justify-center text-muted-foreground border border-dashed rounded-md bg-muted/20 flex-col gap-2">
                            <h3 className="text-4xl font-mono font-bold">{formatBytesLocal(metrics.storage_size_bytes)}</h3>
                            <span className="text-xs">Total Storage</span>
                        </div>
                    </CardContent>
                </Card>

                {/* THE ORACLE */}
                <div className="col-span-4 lg:col-span-7">
                    <ProphecyCard />
                </div>

            </div>

            {/* Alerts & Logs */}
            <div className="grid gap-4 md:grid-cols-1">
                <Card className="border-primary/10">
                    <CardHeader>
                        <CardTitle className="flex items-center gap-2">
                            <AlertTriangle className="h-4 w-4 text-yellow-500" />
                            System Alerts & Notices
                        </CardTitle>
                    </CardHeader>
                    <CardContent>
                        <div className="space-y-4">
                            {data.active_alerts.length === 0 ? (
                                <div className="flex items-center gap-2 text-green-600 p-4 bg-green-50 rounded-md">
                                    <CheckCircle className="h-5 w-5" />
                                    <span>All systems operational. No active alerts.</span>
                                </div>
                            ) : (
                                data.active_alerts.map(alert => (
                                    <div key={alert.id} className="flex items-start gap-3 p-3 bg-muted/30 rounded-md border">
                                        <Info className="h-5 w-5 text-blue-500 mt-0.5" />
                                        <div>
                                            <h4 className="text-sm font-semibold">{alert.title}</h4>
                                            <p className="text-xs text-muted-foreground">{alert.message}</p>
                                            <span className="text-[10px] text-muted-foreground uppercase mt-1 block">
                                                {new Date(alert.timestamp).toLocaleTimeString()} · {alert.severity}
                                            </span>
                                        </div>
                                    </div>
                                ))
                            )}
                        </div>
                    </CardContent>
                </Card>
            </div>
        </div>
    );
}

function MetricCard({ title, value, icon: Icon, description, trend, trendUp }: any) {
    return (
        <Card className="border-l-4 border-l-primary/50 bg-card/50">
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">
                    {title}
                </CardTitle>
                <Icon className="h-4 w-4 text-muted-foreground" />
            </CardHeader>
            <CardContent>
                <div className="text-2xl font-bold">{value}</div>
                <div className="flex items-center text-xs text-muted-foreground">
                    {description}
                    {trend && (
                        <span className={`ml-2 font-medium ${trendUp ? "text-green-500" : "text-red-500"}`}>
                            {trend}
                        </span>
                    )}
                </div>
            </CardContent>
        </Card>
    );
}
