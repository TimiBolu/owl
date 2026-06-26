
import { useState } from "react";
import { ActivityBar } from "./ActivityBar";

import { SidePanel } from "./SidePanel";
import { StatusBar } from "./StatusBar";
import { TimelineScrubber } from "./TimelineScrubber";

import { ActivityMode } from "./types";
import { useJarvis } from "@/hooks/useJarvis";
import { Volume2, VolumeX } from "lucide-react";
import { Button } from "../ui/button";



interface ShellProps {
    sidebarContent: React.ReactNode;
    mainContent: React.ReactNode;
    status: string;
    activeMode: ActivityMode;
    onModeChange: (mode: ActivityMode) => void;
    sidebarTitle?: string;
}

export function Shell({
    sidebarContent,
    mainContent,
    status,
    activeMode,
    onModeChange,
    sidebarTitle
}: ShellProps) {

    const { isMuted, toggleMute, initAudio } = useJarvis();

    return (
        <div
            className="flex h-screen flex-col overflow-hidden bg-background text-foreground"
            onClick={initAudio} // Initialize audio context on first click
        >
            {/* Top Area: Activity + Sidebar + Main */}
            <div className="flex flex-1 overflow-hidden">
                <ActivityBar activeMode={activeMode} onModeChange={onModeChange} />

                {/* Side Panel (Context Aware) */}
                <SidePanel title={sidebarTitle}>
                    {sidebarContent}
                </SidePanel>

                {/* Main Content Area */}
                <main className="flex-1 flex flex-col overflow-hidden bg-muted/10 h-full relative">
                    {mainContent}
                </main>

            </div>

            {/* Global Timeline */}
            <TimelineScrubber />

            {/* Bottom Status Bar */}
            <div className="flex items-center justify-between border-t bg-card px-4 py-1">
                <StatusBar status={status} metrics={{ latency: "45ns", memory: "128MB" }} />

                {/* JARVIS CONTROLS */}
                <Button variant="ghost" size="sm" onClick={(e) => { e.stopPropagation(); toggleMute(); }} className="h-6 gap-2 text-xs text-muted-foreground hover:text-primary">
                    {isMuted ? <VolumeX className="h-3 w-3" /> : <Volume2 className="h-3 w-3 animate-pulse text-green-500" />}
                    <span>JARVIS {isMuted ? 'OFFLINE' : 'LISTENING'}</span>
                </Button>
            </div>
        </div>
    );
}

