
import { useState, useMemo } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { CollectionSummary, FieldDraft, SchemaConstraintDraft } from "../types";

export function useCollections(
    selectedDb: string,
    onStatusChange: (status: string) => void
) {
    const [collectionFilter, setCollectionFilter] = useState("");
    const [collectionSummaries, setCollectionSummaries] = useState<CollectionSummary[]>([]);
    const [collectionLoading, setCollectionLoading] = useState(false);

    async function loadCollections(database: string) {
        setCollectionLoading(true);
        try {
            const res = await api<{ collections: CollectionSummary[] }>("/api/collections", { database });
            setCollectionSummaries(res.collections ?? []);
        } catch (e) {
            setCollectionSummaries([]);
            onStatusChange(presentErrorMessage((e as Error).message));
        } finally {
            setCollectionLoading(false);
        }
    }

    async function deleteCurrentCollection(
        selectedSchema: string,
        onSuccess: () => void
    ) {
        if (!selectedDb || !selectedSchema) {
            onStatusChange("Select a collection first");
            return;
        }
        if (!confirm(`Delete collection '${selectedSchema}' and all its documents?`)) return;
        try {
            onStatusChange("Deleting collection...");
            await api("/api/collections/delete", { database: selectedDb, name: selectedSchema });
            await onSuccess();
            onStatusChange("Collection deleted");
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    async function deleteDatabase(
        onSuccess: (deletedDbName: string) => void
    ) {
        if (!selectedDb) {
            onStatusChange("Select a database first");
            return;
        }
        if (!confirm(`Delete database '${selectedDb}' and its files?`)) return;
        try {
            const deleting = selectedDb;
            onStatusChange(`Deleting database '${deleting}'...`);
            await api("/api/databases/delete", { name: deleting });
            onSuccess(deleting);
            onStatusChange(`Database '${deleting}' deleted`);
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    async function upsertCollectionSchema(
        name: string,
        fields: FieldDraft[],
        constraints: SchemaConstraintDraft[],
        strategy: "replace" | "merge"
    ) {
        if (!selectedDb) {
            onStatusChange("Select a database first");
            return false;
        }

        const cleanedFields = fields
            .map((f) => ({ name: f.name.trim(), type: f.type.trim() }))
            .filter((f) => f.name && f.type);

        if (!name.trim()) {
            onStatusChange("Collection name is required");
            return false;
        }
        if (!cleanedFields.length) {
            onStatusChange("At least one valid field is required");
            return false;
        }

        try {
            onStatusChange(strategy === "merge" ? "Updating collection schema..." : "Saving collection schema...");
            await api("/api/collections/upsert", {
                database: selectedDb,
                name: name.trim(),
                fields: cleanedFields,
                constraints,
                strategy,
            });
            onStatusChange(`Collection '${name.trim()}' saved`);
            return true;
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
            return false;
        }
    }

    async function rebuildIndexes(backend: "memory" | "zig" = "memory") {
        if (!selectedDb) {
            onStatusChange("Select a database first");
            return false;
        }
        try {
            onStatusChange(`Rebuilding indexes (${backend})...`);
            await api("/api/indexes/rebuild", {
                database: selectedDb,
                backend,
            });
            onStatusChange("Index rebuild complete");
            return true;
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
            return false;
        }
    }

    const filteredCollections = useMemo(() => {
        const f = collectionFilter.toLowerCase();
        return collectionSummaries.filter((c) => c.name.toLowerCase().includes(f));
    }, [collectionSummaries, collectionFilter]);

    return {
        collectionFilter,
        setCollectionFilter,
        collectionSummaries,
        setCollectionSummaries,
        collectionLoading,
        loadCollections,
        deleteCurrentCollection,
        deleteDatabase,
        upsertCollectionSchema,
        rebuildIndexes,
        filteredCollections,
    };
}
