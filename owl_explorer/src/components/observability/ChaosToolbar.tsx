
import { cn } from "@/lib/utils";
import { useChaosStore, ChaosType } from "@/lib/chaos/store";
import { Button } from "@/components/ui/button";
import { Skull, Zap, Flame, ShieldAlert, Ban } from "lucide-react";
import { Card } from "@/components/ui/card";

export function ChaosToolbar() {
    const isChaosMode = useChaosStore(s => s.isChaosMode);
    const toggleChaosMode = useChaosStore(s => s.toggleChaosMode);
    const selectedWeapon = useChaosStore(s => s.selectedWeapon);
    const selectWeapon = useChaosStore(s => s.selectWeapon);
    const activeEvents = useChaosStore(s => s.activeEvents);
    const clearAll = useChaosStore(s => s.clearAll);

    if (!isChaosMode) {
        return (
            <Button
                variant="destructive"
                size="sm"
                onClick={toggleChaosMode}
                className="gap-2 font-bold animate-pulse shadow-lg hover:shadow-red-500/50 transition-all"
            >
                <ShieldAlert className="h-4 w-4" />
                ENTER WAR ROOM
            </Button>
        );
    }

    return (
        <Card className="flex flex-col gap-2 p-2 bg-destructive/10 border-destructive/50 shadow-2xl backdrop-blur-md min-w-[300px]">
            <div className="flex items-center justify-between border-b border-destructive/30 pb-2 mb-1">
                <span className="text-xs font-black text-destructive tracking-widest uppercase flex items-center gap-2">
                    <ShieldAlert className="h-3 w-3" />
                    Chaos Engineering
                </span>
                <Button
                    variant="ghost"
                    size="icon"
                    className="h-5 w-5 text-muted-foreground hover:text-foreground"
                    onClick={toggleChaosMode}
                >
                    <Ban className="h-3 w-3" />
                </Button>
            </div>

            <div className="grid grid-cols-3 gap-2">
                <WeaponButton
                    type="meteor"
                    icon={Skull}
                    label="METEOR"
                    active={selectedWeapon === 'meteor'}
                    onClick={() => selectWeapon('meteor')}
                />
                <WeaponButton
                    type="fire"
                    icon={Flame}
                    label="INFERNO"
                    active={selectedWeapon === 'fire'}
                    onClick={() => selectWeapon('fire')}
                />
                <WeaponButton
                    type="latency"
                    icon={Zap}
                    label="LAG SPIKE"
                    active={selectedWeapon === 'latency'}
                    onClick={() => selectWeapon('latency')}
                />
            </div>

            <div className="flex items-center justify-between pt-2 mt-1 border-t border-destructive/30 text-[10px] text-destructive/80 font-mono">
                <span>ACTIVE FAULTS: {activeEvents.length}</span>
                {activeEvents.length > 0 && (
                    <button onClick={clearAll} className="hover:text-destructive underline font-bold cursor-pointer">
                        RESOLVE ALL
                    </button>
                )}
            </div>

            <div className="bg-black/20 p-2 rounded text-[10px] text-muted-foreground italic text-center">
                {selectedWeapon ? "Select a node to target..." : "Select a weapon..."}
            </div>
        </Card>
    );
}

function WeaponButton({ type, icon: Icon, label, active, onClick }: { type: ChaosType, icon: any, label: string, active: boolean, onClick: () => void }) {
    return (
        <Button
            variant={active ? "destructive" : "outline"}
            className={cn(
                "h-16 flex flex-col gap-1 border-destructive/40 hover:bg-destructive/20 hover:text-destructive",
                active && "ring-2 ring-offset-1 ring-destructive"
            )}
            onClick={onClick}
        >
            <Icon className={cn("h-6 w-6", active && "animate-bounce")} />
            <span className="text-[9px] font-bold">{label}</span>
        </Button>
    )
}
