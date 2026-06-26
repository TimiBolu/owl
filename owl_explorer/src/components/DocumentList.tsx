
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { OwlDocument, DocumentsPage, DocumentViewMode } from "../types";
import { DocumentGrid } from "./DocumentGrid";

type DocumentListProps = {
    docsPage: DocumentsPage | null;
    loading: boolean;
    page: number;
    onPageChange: (p: number) => void;
    pageSize: number;
    onPageSizeChange: (s: number) => void;
    filter: string;
    onFilterChange: (v: string) => void;
    visibleDocs: OwlDocument[];
    onStatusChange: (status: string) => void;
    // Optional view mode
    viewMode?: DocumentViewMode;
    onViewModeChange?: (mode: DocumentViewMode) => void;

    // Editing Props
    editingDocId?: string | null;
    editingDocSchema?: string;
    editValues?: Record<string, string>;
    editJsonDraft?: string;
    onStartEdit?: (doc: OwlDocument, mode: "fields" | "json") => void;
    onEditFieldChange?: (field: string, value: string) => void;
    onEditJsonChange?: (value: string) => void;
    onSaveEdit?: (doc: OwlDocument, mode: "fields" | "json") => void;
    onCancelEdit?: () => void;
    onRequestDelete?: (doc: OwlDocument) => void;
    onDuplicate?: (doc: OwlDocument) => void;
    onRequestDeleteAll?: () => void;
};

export function DocumentList({
    docsPage,
    loading,
    page,
    onPageChange,
    pageSize,
    onPageSizeChange,
    filter,
    onFilterChange,
    visibleDocs,
    onStatusChange,
    viewMode = "json",
    onViewModeChange,
    editingDocId,
    editingDocSchema,
    editValues,
    editJsonDraft,
    onStartEdit,
    onEditFieldChange,
    onEditJsonChange,
    onSaveEdit,
    onCancelEdit,
    onRequestDelete,
    onDuplicate,
    onRequestDeleteAll
}: DocumentListProps) {
    return (
        <Card>
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Documents</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
                <div className="flex flex-wrap items-center gap-2">
                    <Badge variant="subtle">{docsPage?.total ?? 0} docs</Badge>
                    <Badge variant="subtle">page {docsPage?.page ?? 1}/{docsPage?.total_pages ?? 1}</Badge>
                    <Input className="ml-auto max-w-sm" placeholder="Filter current page..." value={filter} onChange={(e) => onFilterChange(e.target.value)} />
                    <select
                        className="h-10 rounded-md border border-input bg-card px-3 text-sm"
                        value={pageSize}
                        onChange={(e) => onPageSizeChange(Number(e.target.value))}
                    >
                        <option value={10}>10</option>
                        <option value={20}>20</option>
                        <option value={50}>50</option>
                    </select>
                    {onViewModeChange && (
                        <div className="flex items-center gap-1 rounded-md border border-input bg-card p-1">
                            <Button size="sm" variant={viewMode === "json" ? "default" : "secondary"} onClick={() => onViewModeChange("json")}>JSON</Button>
                            <Button size="sm" variant={viewMode === "list" ? "default" : "secondary"} onClick={() => onViewModeChange("list")}>List</Button>
                            <Button size="sm" variant={viewMode === "table" ? "default" : "secondary"} onClick={() => onViewModeChange("table")}>Table</Button>
                        </div>
                    )}
                    {onRequestDeleteAll && (
                        <Button size="sm" variant="secondary" onClick={onRequestDeleteAll}>Delete All</Button>
                    )}
                </div>

                {loading ? (
                    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                        <Skeleton className="h-52" />
                        <Skeleton className="h-52" />
                        <Skeleton className="h-52" />
                    </div>
                ) : (
                    <DocumentGrid
                        documents={visibleDocs}
                        mode={viewMode}
                        onCopy={(doc) => {
                            navigator.clipboard.writeText(JSON.stringify(doc, null, 2)).catch(() => null);
                            onStatusChange("Copied document JSON");
                        }}
                        editingDocId={editingDocId}
                        editingDocSchema={editingDocSchema}
                        editValues={editValues}
                        editJsonDraft={editJsonDraft}
                        onStartEdit={onStartEdit}
                        onEditFieldChange={onEditFieldChange}
                        onEditJsonChange={onEditJsonChange}
                        onSaveEdit={onSaveEdit}
                        onCancelEdit={onCancelEdit}
                        onRequestDelete={onRequestDelete}
                        onDuplicate={onDuplicate}
                    />
                )}

                <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="text-sm text-muted-foreground">
                        {docsPage?.total ?? 0} docs · page {docsPage?.page ?? 1}/{docsPage?.total_pages ?? 1}
                    </div>
                    <div className="flex gap-2">
                        <Button variant="secondary" size="sm" disabled={page <= 1} onClick={() => onPageChange(1)}>First</Button>
                        <Button variant="secondary" size="sm" disabled={page <= 1} onClick={() => onPageChange(Math.max(1, page - 1))}>Prev</Button>
                        <Button
                            variant="secondary"
                            size="sm"
                            disabled={page >= (docsPage?.total_pages ?? 1)}
                            onClick={() => onPageChange(Math.min(docsPage?.total_pages ?? 1, page + 1))}
                        >
                            Next
                        </Button>
                        <Button
                            variant="secondary"
                            size="sm"
                            disabled={page >= (docsPage?.total_pages ?? 1)}
                            onClick={() => onPageChange(docsPage?.total_pages ?? 1)}
                        >
                            Last
                        </Button>
                    </div>
                </div>
            </CardContent>
        </Card>
    );
}
