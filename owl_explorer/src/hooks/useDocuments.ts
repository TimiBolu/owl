
import { useState, useEffect, useRef } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { DocumentsPage } from "../types";

export function useDocuments(
    selectedDb: string,
    selectedSchema: string,
    onStatusChange: (status: string) => void
) {
    const [page, setPage] = useState(1);
    const [pageSize, setPageSize] = useState(Number(localStorage.getItem("owl.pageSize") ?? "20"));
    const [docsPage, setDocsPage] = useState<DocumentsPage | null>(null);
    const [docsLoading, setDocsLoading] = useState(false);
    const [docFilter, setDocFilter] = useState("");
    const requestSeq = useRef(0);

    useEffect(() => {
        if (!selectedDb || !selectedSchema) return;
        void loadDocuments(selectedDb, selectedSchema, page, pageSize);
    }, [selectedDb, selectedSchema, page, pageSize]);

    // When database or schema changes, reset page to 1
    useEffect(() => {
        setPage(1);
    }, [selectedDb, selectedSchema]);


    async function loadDocuments(database: string, schema: string, pageIndex: number, size: number) {
        const requestId = ++requestSeq.current;
        setDocsLoading(true);
        try {
            const res = await api<{ page: DocumentsPage }>("/api/documents", {
                database,
                schema,
                page: pageIndex,
                page_size: size,
            });
            if (requestId !== requestSeq.current) {
                return;
            }
            setDocsPage(res.page);
        } catch (e) {
            if (requestId !== requestSeq.current) {
                return;
            }
            onStatusChange(presentErrorMessage((e as Error).message));
            setDocsPage(null);
        } finally {
            if (requestId === requestSeq.current) {
                setDocsLoading(false);
            }
        }
    }

    function handlePageSizeChange(size: number) {
        setPageSize(size);
        localStorage.setItem("owl.pageSize", String(size));
        setPage(1);
    }


    const visibleBrowsingDocs = (docsPage?.documents ?? []).filter((doc) => {
        if (!docFilter) return true;
        const json = JSON.stringify(doc).toLowerCase();
        return json.includes(docFilter.toLowerCase());
    });

    return {
        page,
        setPage,
        pageSize,
        setPageSize: handlePageSizeChange,
        docsPage,
        docsLoading,
        docFilter,
        setDocFilter,
        visibleBrowsingDocs,
        refreshDocuments: () => loadDocuments(selectedDb, selectedSchema, page, pageSize),
    };

}
