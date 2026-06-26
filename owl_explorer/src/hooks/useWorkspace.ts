
import { useState } from "react";
import { WorkspaceTab } from "../types";

export function useWorkspace() {
    const [activeWorkspaceTabId, setActiveWorkspaceTabId] = useState<string>("");
    const [workspaceTabs, setWorkspaceTabs] = useState<WorkspaceTab[]>([]);

    function tabIdForDatabase(database: string): string {
        return `db:${database}`;
    }

    function tabIdForCollection(database: string, schema: string): string {
        return `col:${database}:${schema}`;
    }

    function ensureDatabaseTab(database: string) {
        const dbTabId = tabIdForDatabase(database);
        setWorkspaceTabs((prev) => {
            if (prev.some((t) => t.id === dbTabId)) return prev;
            return [...prev, { id: dbTabId, type: "database", label: `DB: ${database}`, database }];
        });
    }

    function openDatabaseContext(database: string) {
        ensureDatabaseTab(database);
        setActiveWorkspaceTabId(tabIdForDatabase(database));
    }

    function openCollectionContext(database: string, schema: string) {
        const id = tabIdForCollection(database, schema);
        setWorkspaceTabs((prev) => {
            if (prev.some((t) => t.id === id)) return prev;
            return [...prev, { id, type: "collection", label: `${database}/${schema}`, database, schema }];
        });
        setActiveWorkspaceTabId(id);
    }

    function closeWorkspaceTab(tab: WorkspaceTab) {
        if (tab.type === "database") return;
        setWorkspaceTabs((prev) => {
            const next = prev.filter((t) => t.id !== tab.id);
            setActiveWorkspaceTabId((curr) => {
                if (curr !== tab.id) return curr;
                const dbTab = tabIdForDatabase(tab.database);
                return next.some((t) => t.id === dbTab) ? dbTab : next[0]?.id ?? "";
            });
            return next;
        });
    }

    return {
        activeWorkspaceTabId,
        setActiveWorkspaceTabId,
        workspaceTabs,
        setWorkspaceTabs,
        ensureDatabaseTab,
        openDatabaseContext,
        openCollectionContext,
        closeWorkspaceTab,
        tabIdForDatabase,
        tabIdForCollection,
    };
}
