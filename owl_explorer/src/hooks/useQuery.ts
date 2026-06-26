
import { useState } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { OwlDocument, QueryResponse } from "../types";

export function useQuery(
    selectedDb: string,
    onStatusChange: (status: string) => void
) {
    const [query, setQuery] = useState("");
    const [queryError, setQueryError] = useState("");
    const [queryDocs, setQueryDocs] = useState<OwlDocument[]>([]);

    const [queryMeta, setQueryMeta] = useState<QueryResponse["meta"] | undefined>(undefined);
    const [queryFilter, setQueryFilter] = useState("");
    const [recentQueries, setRecentQueries] = useState<string[]>(JSON.parse(localStorage.getItem("owl.queryHistory") ?? "[]"));

    async function runQuery(raw?: string) {
        if (!selectedDb) {
            onStatusChange("Select a database first");
            return;
        }
        const payload = (raw ?? query).trim();
        if (!payload) {
            onStatusChange("Enter a query");
            return;
        }
        try {
            onStatusChange("Running query...");
            setQueryError("");
            const res = await api<QueryResponse>("/api/query", { database: selectedDb, query: payload });
            const docs = res.mode === "single" ? (res.documents ?? []) : (res.last_query_documents ?? []);
            setQueryDocs(docs);
            setQueryMeta(res.meta);
            onStatusChange(`Query complete: ${docs.length} document(s)`);

            const nextHistory = [payload, ...recentQueries.filter((q) => q !== payload)].slice(0, 20);
            setRecentQueries(nextHistory);
            localStorage.setItem("owl.queryHistory", JSON.stringify(nextHistory));

            return true; // Indicate success to switch tab
        } catch (e) {
            const message = presentErrorMessage((e as Error).message);
            setQueryError(message);
            onStatusChange(message);
            return false;
        }
    }

    function resetToInitialDataset() {
        setQuery("");
        setQueryError("");
        setQueryDocs([]);

        setQueryMeta(undefined);

        setQueryFilter("");
        onStatusChange("Reset to initial dataset view");
    }

    function updateQueryDocument(updated: OwlDocument) {
        setQueryDocs((prev) =>
            prev.map((doc) =>
                doc.id === updated.id && doc.schema === updated.schema ? updated : doc
            )
        );
    }

    function removeQueryDocument(schema: string, id: string) {
        setQueryDocs((prev) =>
            prev.filter((doc) => !(doc.schema === schema && doc.id === id))
        );
    }


    function clearQueryDocuments() {
        setQueryDocs([]);
        setQueryMeta(undefined);
    }



    const visibleQueryDocs = queryDocs.filter((doc) => {
        if (!queryFilter) return true;
        const json = JSON.stringify(doc).toLowerCase();
        return json.includes(queryFilter.toLowerCase());
    });

    return {
        query,
        setQuery,
        queryError,
        queryDocs,
        queryMeta,
        queryFilter,
        setQueryFilter,
        visibleQueryDocs,
        recentQueries,
        runQuery,
        resetToInitialDataset,
        updateQueryDocument,
        removeQueryDocument,
        clearQueryDocuments,
    };

}
