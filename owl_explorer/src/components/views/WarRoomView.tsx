

import { useState } from 'react';
import { ShieldAlert, Skull, AlertTriangle, Activity } from 'lucide-react';
import ClusterMap from '../observability/ClusterMap';
import ClusterHolodeck from '../observability/ClusterHolodeck';

import { ChaosToolbar } from '../observability/ChaosToolbar';
import { useChaosStore } from '@/lib/chaos/store';
import { useMetrics } from '@/hooks/useMetrics';


export function WarRoomView() {
    const { data } = useMetrics();
    const activeEvents = useChaosStore(s => s.activeEvents);
    const [is3D, setIs3D] = useState(false);


    return (
        <div className="h-full w-full flex flex-col bg-background relative overflow-hidden">
            {/* Header */}
            <div className="h-16 border-b bg-card/50 flex items-center justify-between px-6 z-10">
                <div className="flex items-center gap-3">
                    <div className="p-2 bg-destructive/10 rounded-md">
                        <ShieldAlert className="h-6 w-6 text-destructive" />
                    </div>
                    <div>
                        <h1 className="text-xl font-bold tracking-tight text-foreground">War Room</h1>
                        <p className="text-xs text-muted-foreground font-mono">CHAOS ENGINEERING</p>
                    </div>
                </div>

                {/* HUD Stats */}
                <div className="flex items-center gap-6">
                    <div className="text-right">
                        <span className="block text-xs text-muted-foreground uppercase tracking-wider">Active Faults</span>
                        <span className="text-2xl font-mono font-bold text-destructive">{activeEvents.length}</span>
                    </div>
                    <div className="text-right">
                        <span className="block text-xs text-muted-foreground uppercase tracking-wider">Cluster Health</span>
                        <span className="text-2xl font-mono font-bold text-primary">
                            {(data.system_metrics.uptime_seconds % 100).toFixed(1)}%
                        </span>
                    </div>
                </div>
            </div>


            {/* Main Battleground */}
            <div className="flex-1 relative bg-black/90">
                {is3D ? <ClusterHolodeck /> : <ClusterMap />}

                {/* View Toggle */}
                <div className="absolute top-4 left-4 z-50">
                    <div className="flex items-center bg-card/10 backdrop-blur-md rounded-lg p-1 border border-white/10">
                        <button
                            onClick={() => setIs3D(false)}
                            className={`px-3 py-1.5 text-xs font-medium rounded-md transition-all ${!is3D ? 'bg-primary text-primary-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}`}
                        >
                            Tactical 2D
                        </button>
                        <button
                            onClick={() => setIs3D(true)}
                            className={`px-3 py-1.5 text-xs font-medium rounded-md transition-all ${is3D ? 'bg-purple-600 text-white shadow-sm' : 'text-muted-foreground hover:text-foreground'}`}
                        >
                            Holodeck 3D
                        </button>
                    </div>
                </div>


                {/* Floating Toolbar */}
                <div className="absolute top-4 right-4 z-50">
                    <ChaosToolbar />
                </div>

                {/* Event Log Overlay */}
                <div className="absolute bottom-4 left-4 z-40 w-80 pointer-events-none">
                    <div className="bg-background/80 border rounded p-4 backdrop-blur-sm shadow-sm">
                        <h3 className="text-xs font-bold text-muted-foreground uppercase mb-2 flex items-center gap-2">
                            <Activity className="h-3 w-3" />
                            Operations Log
                        </h3>
                        <div className="space-y-2 max-h-[200px] overflow-hidden flex flex-col-reverse">
                            {activeEvents.map(e => (
                                <div key={e.id} className="text-[10px] font-mono text-destructive flex items-center gap-2 animate-in slide-in-from-left-2">
                                    <AlertTriangle className="h-3 w-3" />
                                    <span>
                                        FAILED: {e.targetNodeId} caused by {e.type.toUpperCase()}
                                    </span>
                                </div>
                            ))}
                            {activeEvents.length === 0 && (
                                <span className="text-[10px] text-muted-foreground italic">No active chaos events. System nominal.</span>
                            )}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}
