

export type ActivityMode = "explorer" | "monitor" | "studio" | "settings" | "war_room";


export interface ActivityItem {
    id: ActivityMode;
    label: string;

    icon: React.ComponentType<{ className?: string }>;

}
