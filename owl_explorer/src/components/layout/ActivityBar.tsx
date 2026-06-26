
import { ActivityMode, ActivityItem } from "./types";

import { Database, Activity, Code, Settings, ShieldAlert } from "lucide-react";

import { cn } from "@/lib/utils";

const ACTIVITIES: ActivityItem[] = [

    { id: "explorer", label: "Explorer", icon: Database },
    { id: "monitor", label: "Monitor", icon: Activity },
    { id: "studio", label: "Studio", icon: Code },
    { id: "war_room", label: "War Room", icon: ShieldAlert },
    { id: "settings", label: "Settings", icon: Settings },
];


interface ActivityBarProps {
    activeMode: ActivityMode;
    onModeChange: (mode: ActivityMode) => void;
}

export function ActivityBar({ activeMode, onModeChange }: ActivityBarProps) {
    return (
        <div className="flex w-[50px] flex-col items-center border-r bg-muted/40 py-4">
            {ACTIVITIES.map((item) => {
                const Icon = item.icon;
                const isActive = activeMode === item.id;
                return (
                    <button
                        key={item.id}
                        onClick={() => onModeChange(item.id)}
                        className={cn(
                            "mb-4 flex h-10 w-10 items-center justify-center rounded-md transition-colors hover:bg-muted",
                            isActive ? "bg-primary text-primary-foreground hover:bg-primary/90" : "text-muted-foreground"
                        )}
                        title={item.label}
                    >
                        <Icon className="h-5 w-5" />
                    </button>
                );
            })}
        </div>
    );
}
