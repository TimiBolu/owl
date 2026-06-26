
import { Wifi, Radio, Server } from "lucide-react";

interface StatusBarProps {
    status: string;
    metrics?: {
        latency?: string;
        memory?: string;
    };
}

export function StatusBar({ status, metrics }: StatusBarProps) {
    return (
        <div className="flex h-6 items-center justify-between border-t bg-primary px-3 text-[10px] text-primary-foreground">
            <div className="flex items-center gap-4">
                <div className="flex items-center gap-1.5">
                    <div className="h-2 w-2 rounded-full bg-green-400" />
                    <span>OWL DB CONNECTED</span>
                </div>
                <div className="flex items-center gap-1 text-primary-foreground/80">
                    <Server className="h-3 w-3" />
                    <span>Localhost:3000</span>
                </div>
            </div>

            <div className="flex items-center gap-4">
                <div className="flex items-center gap-1.5">
                    <Radio className="h-3 w-3" />
                    <span>{status}</span>
                </div>
                {metrics && (
                    <>
                        <div className="flex items-center gap-1">
                            <Wifi className="h-3 w-3" />
                            <span>{metrics.latency || "0ms"}</span>
                        </div>
                        <div>MEM: {metrics.memory || "24MB"}</div>
                    </>
                )}
                <div className="px-1 font-mono hover:bg-primary/20 cursor-pointer">
                    utf-8
                </div>
                <div className="px-1 font-mono hover:bg-primary/20 cursor-pointer">
                    Tsc
                </div>
            </div>
        </div>
    );
}
