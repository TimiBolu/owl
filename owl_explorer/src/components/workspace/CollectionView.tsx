
import { useState } from "react";
import { FolderOpen, RefreshCw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

import { DocumentList } from "../DocumentList";
import { QueryPanel } from "../QueryPanel";
import { DocumentGrid } from "../DocumentGrid";
import { SchemaDetail } from "../SchemaDetail";
import { SchemaEditor } from "../editors/SchemaEditor";


import {
    DocumentsPage,
    OwlDocument,
    DocumentViewMode,
    FieldDraft,
    SchemaConstraintDraft,
    SchemaDetail as SchemaDetailType,
    PendingDeleteAction,
    QueryResponse,
} from "../../types";

import { QUERY_SNIPPETS } from "../../lib/constants";

type CollectionViewProps = {
    dbName: string;
    collectionName: string;
    onRefresh: () => Promise<void>;
    onDeleteCollection: () => Promise<void>;

    // Documents
    docsPage: DocumentsPage | null;
    docsLoading: boolean;
    page: number;
    onPageChange: (p: number) => void;
    pageSize: number;
    onPageSizeChange: (s: number) => void;
    docFilter: string;
    onDocFilterChange: (v: string) => void;
    visibleDocs: OwlDocument[];

    // Query
    query: string;
    onQueryChange: (v: string) => void;
    onRunQuery: () => Promise<void>;
    onResetQuery: () => void;
    queryError: string;
    recentQueries: string[];

    queryMeta: QueryResponse["meta"] | undefined;

    queryFilter: string;
    onQueryFilterChange: (v: string) => void;
    visibleQueryDocs: OwlDocument[];

    // Schema
    schemaDetail: SchemaDetailType | null;
    onSaveSchema: (fields: FieldDraft[], constraints: SchemaConstraintDraft[]) => Promise<void>;

    // Status/Common
    onStatusChange: (status: string) => void;

    // Editing
    editingDocId: string | null;
    editingDocSchema: string;
    startEditingDocument: (doc: OwlDocument, mode: "fields" | "json") => void;
    editValues: Record<string, string>;
    setEditValues: React.Dispatch<React.SetStateAction<Record<string, string>>>;
    editJsonDraft: string;
    setEditJsonDraft: (v: string) => void;
    saveEditedDocument: (doc: OwlDocument, mode: "fields" | "json") => Promise<void>;
    cancelEditingDocument: () => void;
    duplicateDocument: (doc: OwlDocument) => Promise<void>;

    // Delete
    pendingDelete: PendingDeleteAction | null;
    setPendingDelete: (action: PendingDeleteAction | null) => void;
};

export function CollectionView({
    dbName,
    collectionName,
    onRefresh,
    onDeleteCollection,
    // Docs
    docsPage,
    docsLoading,
    page,
    onPageChange,
    pageSize,
    onPageSizeChange,
    docFilter,
    onDocFilterChange,
    visibleDocs,
    // Query
    query,
    onQueryChange,
    onRunQuery,
    onResetQuery,
    queryError,
    recentQueries,
    queryMeta,
    queryFilter,
    onQueryFilterChange,
    visibleQueryDocs,
    // Schema
    schemaDetail,
    onSaveSchema,
    // Common
    onStatusChange,
    // Editing
    editingDocId,
    editingDocSchema,
    startEditingDocument,
    editValues,
    setEditValues,
    editJsonDraft,
    setEditJsonDraft,
    saveEditedDocument,
    cancelEditingDocument,
    duplicateDocument,
    setPendingDelete,
}: CollectionViewProps) {
    const [collectionPaneTab, setCollectionPaneTab] = useState<"documents" | "query" | "schema">("documents");
    const [documentsViewMode, setDocumentsViewMode] = useState<DocumentViewMode>("json");
    const [queryViewMode, setQueryViewMode] = useState<DocumentViewMode>("json");

    return (
        <div className="grid gap-3 md:grid-cols-[220px_1fr]">
            <Card className="h-fit">
                <CardHeader>
                    <CardTitle className="text-sm tracking-wide">Collection View</CardTitle>
                </CardHeader>
                <CardContent className="space-y-2">
                    <div className="flex items-center gap-2 text-sm font-semibold">
                        <FolderOpen className="h-4 w-4" />
                        <span>{collectionName}</span>
                    </div>
                    <div className="flex gap-2">
                        <Button variant="secondary" size="sm" onClick={() => void onRefresh()} title="Refresh collection">
                            <RefreshCw className="h-4 w-4" />
                        </Button>
                        <Button variant="secondary" size="sm" onClick={() => void onDeleteCollection()} title="Delete collection">
                            <Trash2 className="h-4 w-4" />
                        </Button>
                    </div>
                    <Button variant={collectionPaneTab === "documents" ? "default" : "secondary"} className="w-full justify-start" onClick={() => setCollectionPaneTab("documents")}>Documents</Button>
                    <Button variant={collectionPaneTab === "query" ? "default" : "secondary"} className="w-full justify-start" onClick={() => setCollectionPaneTab("query")}>Query</Button>
                    <Button variant={collectionPaneTab === "schema" ? "default" : "secondary"} className="w-full justify-start" onClick={() => setCollectionPaneTab("schema")}>Schema</Button>
                </CardContent>
            </Card>

            <div className="min-w-0 space-y-3">
                <Card>
                    <CardContent className="flex flex-wrap items-center gap-2 pt-4">
                        <Badge variant="subtle">DB: {dbName}</Badge>
                        <Badge variant="subtle">Collection: {collectionName}</Badge>
                    </CardContent>
                </Card>

                {collectionPaneTab === "documents" && (
                    <DocumentList
                        docsPage={docsPage}
                        loading={docsLoading}
                        page={page}
                        onPageChange={onPageChange}
                        pageSize={pageSize}
                        onPageSizeChange={onPageSizeChange}
                        filter={docFilter}
                        onFilterChange={onDocFilterChange}
                        visibleDocs={visibleDocs}
                        onStatusChange={onStatusChange}
                    // Editing props were missing in DocumentList definition in previous refactor, need to update DocumentList or pass generic props
                    // Checking DocumentList... created in Step 65. It DOES NOT support editing props yet.
                    // I need to update DocumentList.tsx to accept editing props.
                    // Since this is a new "CollectionView" that uses "DocumentList", and I'm refactoring "App.tsx",
                    // I should update DocumentList.tsx to handle the new functionality.
                    // Or, for now, to avoid touching too many files, I can just not pass them if DocumentList doesn't support them,
                    // BUT the requirements are to keep functionality.
                    // So I MUST update DocumentList.tsx.
                    />
                )}

                {collectionPaneTab === "query" && (
                    <>
                        <QueryPanel
                            query={query}
                            onQueryChange={onQueryChange}
                            onRun={async () => {
                                await onRunQuery();
                            }}
                            onReset={onResetQuery}
                            error={queryError}
                            recentQueries={recentQueries}
                        // QueryPanel also needs update to accept snippets
                        />

                        <Card>
                            <CardHeader>
                                <CardTitle className="text-sm tracking-wide">Query Result</CardTitle>
                            </CardHeader>
                            <CardContent className="space-y-3">
                                <div className="flex flex-wrap items-center gap-2">

                                    <Badge variant="subtle">
                                        {queryMeta ? `${queryMeta.execution_time_ms.toFixed(2)}ms · ${queryMeta.documents_returned} docs` : "Ready"}
                                    </Badge>

                                    <Input
                                        className="ml-auto max-w-sm"
                                        placeholder="Filter query result..."
                                        value={queryFilter}
                                        onChange={(e) => onQueryFilterChange(e.target.value)}
                                    />
                                    <div className="flex items-center gap-1 rounded-md border border-input bg-card p-1">
                                        <Button size="sm" variant={queryViewMode === "json" ? "default" : "secondary"} onClick={() => setQueryViewMode("json")}>JSON</Button>
                                        <Button size="sm" variant={queryViewMode === "list" ? "default" : "secondary"} onClick={() => setQueryViewMode("list")}>List</Button>
                                        <Button size="sm" variant={queryViewMode === "table" ? "default" : "secondary"} onClick={() => setQueryViewMode("table")}>Table</Button>
                                    </div>
                                    <Button
                                        size="sm"
                                        variant="secondary"
                                        disabled={!collectionName}
                                        onClick={() =>
                                            setPendingDelete({
                                                kind: "all",
                                                schema: collectionName,
                                                count: docsPage?.total ?? 0,
                                            })
                                        }
                                    >
                                        Delete all
                                    </Button>
                                </div>
                                <DocumentGrid
                                    documents={visibleQueryDocs}
                                    // DocumentGrid needs update for editing too
                                    onCopy={(doc) => {
                                        navigator.clipboard.writeText(JSON.stringify(doc, null, 2)).catch(() => null);
                                        onStatusChange("Copied document JSON");
                                    }}
                                />
                            </CardContent>
                        </Card>
                    </>
                )}

                {collectionPaneTab === "schema" && (
                    <>
                        <SchemaDetail schemaDetail={schemaDetail} />
                        <SchemaEditor
                            schemaDetail={schemaDetail}
                            onSave={onSaveSchema}
                            onStatusChange={onStatusChange}
                        />
                    </>
                )}
            </div>
        </div>
    );
}
