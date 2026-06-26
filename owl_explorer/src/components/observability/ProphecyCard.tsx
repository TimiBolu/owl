
import { useEffect, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { Sparkles, TrendingUp, TrendingDown, AlertTriangle } from "lucide-react";
import { useTimeStore } from "@/lib/time/store";
import { generateProphecies, MetricForecast, projectMetric } from "@/lib/oracle/engine";
import { cn } from "@/lib/utils";

export function useOracle() {
    const history = useTimeStore(s => s.history);
    const [prophecies, setProphecies] = useState<MetricForecast[]>([]);

    useEffect(() => {
        if (history.length > 5) {
            const forecasts = generateProphecies(history);
            setProphecies(forecasts);
        }
    }, [history.length]); // Re-calc when history grows

    return { prophecies };
}

export function ProphecyCard() {
    const { prophecies } = useOracle();

    if (prophecies.length === 0) {
        return (
            <Card className="border-purple-500/20 bg-purple-500/5">
                <CardHeader className="pb-2">
                    <CardTitle className="flex items-center gap-2 text-purple-400">
                        <Sparkles className="h-4 w-4" />
                        The Oracle
                    </CardTitle>
                </CardHeader>
                <CardContent>
                    <p className="text-xs text-muted-foreground">Gathering temporal data for prediction...</p>
                </CardContent>
            </Card>
        );
    }

    const primaryProphecy = prophecies[0];

    return (
        <Card className={cn(
            "border-l-4 transition-all duration-500",
            primaryProphecy.severity === 'critical' ? "border-l-red-500 bg-red-500/5" :
                primaryProphecy.severity === 'warning' ? "border-l-yellow-500 bg-yellow-500/5" :
                    "border-l-purple-500 bg-purple-500/5"
        )}>
            <CardHeader className="pb-2">
                <CardTitle className="flex items-center justify-between text-base">
                    <div className="flex items-center gap-2">
                        <Sparkles className={cn("h-4 w-4",
                            primaryProphecy.severity === 'critical' ? "text-red-500 animate-pulse" : "text-purple-500"
                        )} />
                        <span className={cn(
                            primaryProphecy.severity === 'critical' ? "text-red-500" : "text-purple-500"
                        )}>Oracle Prophecy</span>
                    </div>
                    <span className="text-[10px] uppercase tracking-wider font-mono text-muted-foreground">
                        +1 Hour Projection
                    </span>
                </CardTitle>
            </CardHeader>
            <CardContent>
                <div className="space-y-4">
                    {prophecies.slice(0, 2).map((p, i) => (
                        <div key={i} className="flex items-center justify-between">
                            <div className="flex flex-col">
                                <span className="text-sm font-medium flex items-center gap-2">
                                    {p.label}
                                    {p.trend === 'up' ? <TrendingUp className="h-3 w-3 text-red-400" /> : <TrendingDown className="h-3 w-3 text-green-400" />}
                                </span>
                                <span className="text-xs text-muted-foreground">
                                    Current: {formatValue(p.metric, p.currentValue)}
                                </span>
                            </div>

                            <div className="text-right">
                                <span className="block text-lg font-mono font-bold">
                                    {formatValue(p.metric, p.predictedValue)}
                                </span>
                                <span className="text-[10px] text-muted-foreground">Predicted</span>
                            </div>
                        </div>
                    ))}

                    {/* Simple Visualization Bar */}
                    <div className="pt-2 border-t border-dashed border-purple-500/20">
                        <p className="text-xs italic text-muted-foreground flex items-center gap-2">
                            {primaryProphecy.severity === 'critical' ? (
                                <>
                                    <AlertTriangle className="h-3 w-3 text-red-500" />
                                    Warning: Critical threshold approach.
                                </>
                            ) : (
                                "System trajectory is within normal parameters."
                            )}
                        </p>
                    </div>
                </div>
            </CardContent>
        </Card>
    );
}

function formatValue(key: string, val: number) {
    if (key.includes('bytes')) {
        return (val / (1024 * 1024)).toFixed(1) + ' MB';
    }
    if (key.includes('seconds')) {
        return val.toFixed(1) + ' s';
    }
    return val.toFixed(1);
}
