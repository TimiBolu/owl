
import { X } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { WorkspaceTab } from "../../types";

type WorkspaceTabsProps = {
    tabs: WorkspaceTab[];
    activeTabId: string;
    onSelectTab: (tabId: string) => void;
    onCloseTab: (tab: WorkspaceTab) => void;
};

export function WorkspaceTabs({
    tabs,
    activeTabId,
    onSelectTab,
    onCloseTab,
}: WorkspaceTabsProps) {
    return (
        <Card>
            <CardContent className="pt-4">
                <div className="flex flex-wrap items-center gap-2">
                    {tabs.map((tab) => {
                        const active = tab.id === activeTabId;
                        return (
                            <div key={tab.id} className={`flex items-center gap-1 rounded-md border px-2 py-1 ${active ? "border-[#7a5a44] bg-[#ebdfd2]" : "border-border bg-card"}`}>
                                <button className="text-sm font-medium" onClick={() => onSelectTab(tab.id)}>
                                    {tab.label}
                                </button>
                                {tab.type === "collection" && (
                                    <button className="rounded p-0.5 hover:bg-[#d9c6b2]" onClick={() => onCloseTab(tab)}>
                                        <X className="h-3.5 w-3.5" />
                                    </button>
                                )}
                            </div>
                        );
                    })}
                </div>
            </CardContent>
        </Card>
    );
}
