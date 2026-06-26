
import { useState, useEffect, useMemo } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { SchemaDetail } from "../types";

export function useSchemas(
    selectedDb: string,
    onStatusChange: (status: string) => void
) {
    const [schemas, setSchemas] = useState<string[]>([]);
    const [selectedSchema, setSelectedSchema] = useState<string>("");
    const [schemaDetail, setSchemaDetail] = useState<SchemaDetail | null>(null);
    const [schemaFilter, setSchemaFilter] = useState("");

    useEffect(() => {
        if (!selectedDb) return;
        void loadSchemas(selectedDb);
    }, [selectedDb]);

    useEffect(() => {
        if (!selectedDb || !selectedSchema) return;
        // We don't need to load documents here, useDocuments hook will handle that
        // But we do need to load schema details
        localStorage.setItem(`owl.lastSchema.${selectedDb}`, selectedSchema);
        void loadSchemaDetail(selectedDb, selectedSchema);
    }, [selectedDb, selectedSchema]);

    async function loadSchemas(database: string) {
        try {
            const res = await api<{ schemas: string[] }>("/api/schemas", { database });
            setSchemas(res.schemas ?? []);
            const remembered = localStorage.getItem(`owl.lastSchema.${database}`) ?? "";
            const next = res.schemas.includes(remembered) ? remembered : (res.schemas[0] ?? "");
            setSelectedSchema(next);
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
            setSchemas([]);
            setSelectedSchema("");
        }
    }

    async function loadSchemaDetail(database: string, schema: string) {
        try {
            const res = await api<{ schema: SchemaDetail }>("/api/schema_detail", { database, schema });
            setSchemaDetail(res.schema);
        } catch {
            setSchemaDetail(null);
        }
    }

    const filteredSchemas = useMemo(() => {
        const f = schemaFilter.toLowerCase();
        return schemas.filter((s) => s.toLowerCase().includes(f));
    }, [schemas, schemaFilter]);

    return {
        schemas,
        selectedSchema,
        setSelectedSchema,
        schemaDetail,
        schemaFilter,
        setSchemaFilter,
        filteredSchemas,
        refreshSchemas: () => (selectedDb ? loadSchemas(selectedDb) : Promise.resolve()),
        refreshSchemaDetail: () =>
            selectedDb && selectedSchema
                ? loadSchemaDetail(selectedDb, selectedSchema)
                : Promise.resolve(),
    };
}
