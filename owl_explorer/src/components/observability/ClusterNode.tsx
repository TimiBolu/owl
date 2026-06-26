
import { Handle, Position } from '@xyflow/react';
import { Database, Server, Cpu, HardDrive, Skull, Zap, Flame } from 'lucide-react';
import { useChaosStore } from '@/lib/chaos/store';

export function ClusterNode({ id, data }: { id: string, data: any }) {
    const isPrimary = data.role === 'primary';

    // Check Chaos State
    const activeEvents = useChaosStore(s => s.activeEvents);
    const triggerChaos = useChaosStore(s => s.triggerChaos);
    const selectedWeapon = useChaosStore(s => s.selectedWeapon);

    const myChaos = activeEvents.find(e => e.targetNodeId === id);
    const isDead = myChaos?.type === 'meteor';
    const isOnFire = myChaos?.type === 'fire';

    const handleNodeClick = () => {
        if (selectedWeapon) {
            triggerChaos(id, selectedWeapon);
        }
    };

    if (isDead) {
        return (
            <div className="relative shadow-xl rounded-xl border-2 border-transparent bg-destructive/10 min-w-[200px] overflow-hidden grayscale opacity-80 scale-95 transition-all duration-300">
                <div className="absolute inset-0 bg-black/50 z-10 flex items-center justify-center flex-col text-destructive animate-pulse">
                    <Skull className="h-12 w-12 mb-2" />
                    <span className="font-bold text-lg">SYSTEM FAILURE</span>
                </div>
                {/* Content Ghost */}
                <div className="p-4 space-y-3 opacity-20 filter blur-sm">
                    <div className="h-4 bg-muted w-3/4 rounded" />
                    <div className="h-10 bg-muted w-full rounded" />
                </div>
                <Handle type="target" position={Position.Top} className="!bg-transparent" />
                <Handle type="source" position={Position.Bottom} className="!bg-transparent" />
            </div>
        );
    }

    const statusColor = data.status === 'healthy' ? 'bg-green-500' : data.status === 'warning' ? 'bg-yellow-500' : 'bg-red-500';

    return (
        <div
            onClick={handleNodeClick}
            className={`
                relative shadow-lg rounded-xl border-2 bg-card min-w-[200px] overflow-hidden transition-all duration-300 
                ${isPrimary ? 'border-primary shadow-primary/20' : 'border-border'}
                ${selectedWeapon ? 'cursor-crosshair hover:ring-4 hover:ring-destructive/50' : ''}
                ${isOnFire ? 'animate-bounce ring-4 ring-orange-500 border-orange-500' : ''}
            `}
        >
            {isOnFire && <div className="absolute -top-4 -right-2 z-20 text-orange-500 animate-bounce"><Flame className="h-8 w-8 fill-orange-500" /></div>}

            {/* Header */}
            <div className={`px-4 py-2 border-b flex items-center justify-between ${isPrimary ? 'bg-primary/10' : 'bg-muted/30'}`}>
                <div className="flex items-center gap-2">
                    <Server className={`h-4 w-4 ${isPrimary ? 'text-primary' : 'text-muted-foreground'}`} />
                    <span className="font-bold text-sm tracking-tight">{data.label}</span>
                </div>
                <div className={`h-2.5 w-2.5 rounded-full ${statusColor} animate-pulse`} />
            </div>

            {/* Content */}
            <div className="p-4 space-y-3">
                <div className="flex items-center justify-between text-xs">
                    <div className="flex items-center gap-1.5 text-muted-foreground">
                        <Cpu className="h-3.5 w-3.5" />
                        <span>CPU</span>
                    </div>
                    <div className="w-16 h-1.5 bg-muted rounded-full overflow-hidden">
                        <div
                            className={`h-full transition-all duration-1000 ${isOnFire ? 'bg-orange-500 w-full' : 'bg-blue-500'}`}
                            style={{ width: isOnFire ? '100%' : `${data.metrics?.cpu || 0}%` }}
                        />
                    </div>
                    <span className={`font-mono ${isOnFire ? 'text-orange-500 font-bold' : ''}`}>
                        {isOnFire ? '100%' : `${data.metrics?.cpu}%`}
                    </span>
                </div>

                <div className="flex items-center justify-between text-xs">
                    <div className="flex items-center gap-1.5 text-muted-foreground">
                        <HardDrive className="h-3.5 w-3.5" />
                        <span>RAM</span>
                    </div>
                    <div className="w-16 h-1.5 bg-muted rounded-full overflow-hidden">
                        <div
                            className="h-full bg-purple-500 transition-all duration-1000"
                            style={{ width: `${data.metrics?.ram || 0}%` }}
                        />
                    </div>
                    <span className="font-mono">{data.metrics?.ram}%</span>
                </div>

                <div className="flex items-center justify-between pt-2 border-t text-xs text-muted-foreground">
                    <span className="flex items-center gap-1">
                        <Database className="h-3 w-3" />
                        {data.role.toUpperCase()}
                    </span>
                    <span className="font-mono bg-muted px-1.5 py-0.5 rounded text-[10px]">
                        {data.shardCount} Shards
                    </span>
                </div>
            </div>

            <Handle type="target" position={Position.Top} className="!bg-muted-foreground" />
            <Handle type="source" position={Position.Bottom} className="!bg-primary" />
        </div>
    );
}
