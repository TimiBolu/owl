
import { useState, useEffect, useMemo } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { DatabaseEntry } from "../types";

export function useDatabases(onStatusChange: (status: string) => void) {
    const [databases, setDatabases] = useState<DatabaseEntry[]>([]);
    const [selectedDb, setSelectedDb] = useState<string>(localStorage.getItem("owl.lastDb") ?? "");
    const [dbFilter, setDbFilter] = useState("");
    const [pinnedDbs, setPinnedDbs] = useState<string[]>(JSON.parse(localStorage.getItem("owl.pinnedDbs") ?? "[]"));

    // Create DB state
    const [dbName, setDbName] = useState("");
    const [dbPath, setDbPath] = useState("");
    const [dbSize, setDbSize] = useState("128");

    useEffect(() => {
        void loadDatabases();
    }, []);

    useEffect(() => {
        /* If selectedDb changes, store it in local storage */
        if (!selectedDb) return;
        localStorage.setItem("owl.lastDb", selectedDb);
    }, [selectedDb]);

    async function loadDatabases() {
        try {
            const res = await api<{ databases: DatabaseEntry[] }>("/api/databases");
            setDatabases(res.databases ?? []);
            if (!selectedDb && res.databases?.length) {
                setSelectedDb(res.databases[0].name);
            }
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    async function createDatabase() {
        if (!dbName.trim() || !dbPath.trim()) {
            onStatusChange("Name and path are required");
            return;
        }
        try {
            onStatusChange("Creating database...");
            await api("/api/databases", { name: dbName.trim(), path: dbPath.trim(), size_mb: Number(dbSize || "128") });
            setDbName("");
            setDbPath("");
            setDbSize("128");
            await loadDatabases();
            setSelectedDb(dbName.trim());
            onStatusChange("Database created");
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    /* ADDED: deleteDatabase */
    async function deleteDatabase(name: string) {
        try {
            onStatusChange(`Deleting database '${name}'...`);
            // Note: Assuming /api/databases/delete exists based on context from App.tsx
            await api("/api/databases/delete", { name });
            await loadDatabases();
            if (selectedDb === name) {
                setSelectedDb("");
            }
            onStatusChange(`Database '${name}' deleted`);
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    function togglePin(name: string) {
        const next = pinnedDbs.includes(name) ? pinnedDbs.filter((v) => v !== name) : [...pinnedDbs, name];
        setPinnedDbs(next);
        localStorage.setItem("owl.pinnedDbs", JSON.stringify(next));
    }

    const filteredDatabases = useMemo(() => {
        const f = dbFilter.toLowerCase();
        return databases
            .filter((db) => db.name.toLowerCase().includes(f) || db.path.toLowerCase().includes(f))
            .sort((a, b) => {
                const ap = pinnedDbs.includes(a.name) ? 1 : 0;
                const bp = pinnedDbs.includes(b.name) ? 1 : 0;
                if (ap !== bp) return bp - ap;
                return a.name.localeCompare(b.name);
            });
    }, [databases, dbFilter, pinnedDbs]);

    return {
        databases,
        selectedDb,
        setSelectedDb,
        dbFilter,
        setDbFilter,
        pinnedDbs,
        togglePin,
        filteredDatabases,
        refreshDatabases: loadDatabases,
        deleteDatabase, /* Exported */
        createDatabaseState: {
            dbName, setDbName,
            dbPath, setDbPath,
            dbSize, setDbSize,
            createDatabase
        }
    };
}
