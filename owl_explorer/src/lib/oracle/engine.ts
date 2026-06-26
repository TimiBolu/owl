
import { SystemMetrics } from "@/hooks/useMetrics";
import { TimeSeriesPoint } from "../time/store";

export interface Prediction {
    timestamp: number;
    value: number;
    confidence: number; // 0-1
}

export interface MetricForecast {
    metric: keyof SystemMetrics;
    label: string;
    currentValue: number;
    predictedValue: number; // Value in 1 hour
    trend: 'up' | 'down' | 'flat';
    timeToThreshold?: number; // Minutes until critical threshold (e.g. 100% RAM)
    severity: 'info' | 'warning' | 'critical';
}

// Simple Linear Regression to project future values
export function projectMetric(
    history: TimeSeriesPoint[],
    metricKey: keyof SystemMetrics,
    minutesAhead: number = 60
): Prediction[] {
    if (history.length < 10) return [];

    // Extract X (time) and Y (value)
    const points = history.map(p => ({
        x: p.timestamp,
        y: (p.metrics.system_metrics as any)[metricKey] as number
    }));

    const n = points.length;
    let sumX = 0, sumY = 0, sumXY = 0, sumXX = 0;

    for (const p of points) {
        sumX += p.x;
        sumY += p.y;
        sumXY += p.x * p.y;
        sumXX += p.x * p.x;
    }

    const slope = (n * sumXY - sumX * sumY) / (n * sumXX - sumX * sumX);
    const intercept = (sumY - slope * sumX) / n;

    // Generate future points
    const lastTime = points[points.length - 1].x;
    const predictions: Prediction[] = [];

    for (let i = 1; i <= minutesAhead; i += 5) { // Every 5 mins
        const futureTime = lastTime + i * 60 * 1000;
        const predictedVal = slope * futureTime + intercept;
        predictions.push({
            timestamp: futureTime,
            value: predictedVal,
            confidence: Math.max(0, 1 - (i * 0.01)) // Confidence drops over time
        });
    }

    return predictions;
}

export function generateProphecies(history: TimeSeriesPoint[]): MetricForecast[] {
    if (history.length < 10) return [];

    const prophecies: MetricForecast[] = [];
    const latest = history[history.length - 1].metrics.system_metrics;

    // 1. Analyze Memory
    const memProjection = projectMetric(history, 'memory_usage_bytes');
    if (memProjection.length > 0) {
        const currentEx = latest.memory_usage_bytes;
        const futureEx = memProjection[memProjection.length - 1].value;
        const trend = futureEx > currentEx * 1.05 ? 'up' : futureEx < currentEx * 0.95 ? 'down' : 'flat';

        // Threshold: 1GB (simulated max)
        const MAX_MEM = 1024 * 1024 * 1024;

        let severity: MetricForecast['severity'] = 'info';
        if (futureEx > MAX_MEM * 0.9) severity = 'critical';
        else if (futureEx > MAX_MEM * 0.7) severity = 'warning';

        prophecies.push({
            metric: 'memory_usage_bytes',
            label: 'Memory Usage',
            currentValue: currentEx,
            predictedValue: futureEx,
            trend,
            severity
        });
    }

    // 2. Analyze Storage
    const storageProjection = projectMetric(history, 'storage_size_bytes');
    if (storageProjection.length > 0) {
        const current = latest.storage_size_bytes;
        const future = storageProjection[storageProjection.length - 1].value;
        const trend = future > current * 1.01 ? 'up' : 'flat';

        prophecies.push({
            metric: 'storage_size_bytes',
            label: 'Disk Usage',
            currentValue: current,
            predictedValue: future,
            trend,
            severity: 'info'
        });
    }

    // 3. Analyze QPS
    const qpsProjection = projectMetric(history, 'queries_per_second');
    if (qpsProjection.length > 0) {
        const current = latest.queries_per_second;
        const future = qpsProjection[qpsProjection.length - 1].value;
        // If traffic is exploding
        if (future > current * 2) {
            prophecies.push({
                metric: 'queries_per_second',
                label: 'Traffic Load',
                currentValue: current,
                predictedValue: future,
                trend: 'up',
                severity: 'warning'
            });
        }
    }

    return prophecies.sort((a, b) => (a.severity === 'critical' ? -1 : 1));
}
