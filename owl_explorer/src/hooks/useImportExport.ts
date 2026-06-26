
import { useState } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { ImportDocument, SchemaConstraintDraft } from "../types";

export function useImportExport(
    selectedDb: string,
    onStatusChange: (status: string) => void,
    onSuccess: () => Promise<void>
) {
    const [importSchemaName, setImportSchemaName] = useState("");
    const [importFieldsRaw, setImportFieldsRaw] = useState('[{"name":"id","type":"String"}]');
    const [importDocumentsRaw, setImportDocumentsRaw] = useState('[{"id":"d1"}]');
    const [importStrategy, setImportStrategy] = useState<"merge" | "replace">("merge");


    async function importDocumentsWithSchema(
        schema: string,
        fields: { name: string; type: string }[],
        constraints: SchemaConstraintDraft[],
        documents: ImportDocument[],
        strategy: "merge" | "replace"
    ) {
        if (!selectedDb) {
            onStatusChange("Select a database first");
            return;
        }
        const schemaName = schema.trim();
        if (!schemaName) {
            onStatusChange("Import schema name is required");
            return;
        }
        try {
            if (!Array.isArray(fields) || !Array.isArray(documents)) {
                onStatusChange("Import fields/documents must be JSON arrays");
                return;
            }
            onStatusChange("Importing data...");
            const result = await api<{ result: { imported: number; errors: string[] } }>("/api/import", {
                database: selectedDb,
                schema: schemaName,
                fields,
                constraints,
                documents,
                strategy,
            });
            await onSuccess();
            const errorCount = result.result?.errors?.length ?? 0;
            onStatusChange(`Import complete: ${result.result?.imported ?? 0} imported${errorCount ? `, ${errorCount} errors` : ""}`);
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }


    async function seedFeatureDemo() {
        if (!selectedDb) {
            onStatusChange("Select a database first");
            return;
        }
        try {
            onStatusChange("Seeding OQL feature demo...");
            await api("/api/seed_demo", { database: selectedDb });
            await onSuccess();
            onStatusChange("Demo data seeded: users + orders");
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    return {
        importSchemaName,
        setImportSchemaName,
        importFieldsRaw,
        setImportFieldsRaw,
        importDocumentsRaw,
        setImportDocumentsRaw,
        importStrategy,
        setImportStrategy,
        importDocumentsWithSchema,
        seedFeatureDemo,
    };
}
