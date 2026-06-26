
import { cn } from "@/lib/utils";
import { ReactNode } from "react";

interface SidePanelProps {
    children: ReactNode;
    title?: string;
    className?: string;
}

export function SidePanel({ children, title, className }: SidePanelProps) {
    return (
        <div className={cn("flex w-[300px] flex-col border-r bg-background", className)}>
            {title && (
                <div className="flex h-12 items-center border-b px-4 text-sm font-medium uppercase tracking-wider text-muted-foreground">
                    {title}
                </div>
            )}
            <div className="flex-1 overflow-auto">
                {children}
            </div>
        </div>
    );
}
