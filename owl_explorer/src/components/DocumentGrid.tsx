
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Input } from "@/components/ui/input";
import { OwlDocument, DocumentViewMode } from "../types";
import { text } from "../lib/utils";
import { FileEdit, Trash2, Copy, Save, X, Repeat } from "lucide-react";


type DocumentGridProps = {
    documents: OwlDocument[];
    mode?: DocumentViewMode;
    viewMode?: string; // Add this prop to satisfy TypeScript
    onCopy?: (doc: OwlDocument) => void;
    // Editing props (optional)
    editingDocId?: string | null;
    editingDocSchema?: string;
    editValues?: Record<string, string>;
    editJsonDraft?: string;
    onStartEdit?: (doc: OwlDocument, mode: "fields" | "json") => void;
    onEditFieldChange?: (field: string, value: string) => void;
    onEditJsonChange?: (value: string) => void;
    onSaveEdit?: (doc: OwlDocument, mode: "fields" | "json") => void;
    onCancelEdit?: () => void;

    onRequestDelete?: (doc: OwlDocument) => void | Promise<void>;

    onUpdateDocument?: (doc: OwlDocument) => Promise<void>;
    onDeleteDocument?: (doc: OwlDocument) => Promise<void>;
    onDuplicate?: (doc: OwlDocument) => void;
};


export function DocumentGrid({
    documents,
    mode = "json",
    onCopy,
    editingDocId,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    editingDocSchema, // unused locally but good for prop parity
    editValues,
    editJsonDraft,
    onStartEdit,
    onEditFieldChange,
    onEditJsonChange,
    onSaveEdit,
    onCancelEdit,
    onRequestDelete,
    onDuplicate
}: DocumentGridProps) {
    if (mode === "list") {
        return (
            <div className="space-y-2">
                {documents.map(doc => (
                    <Card key={doc.id} className="border-[#d9cec2] bg-[#fffaf4]">
                        <CardContent className="flex items-center justify-between p-3">
                            <div className="font-mono text-sm">{doc.id}</div>
                            <div className="text-xs text-muted-foreground">{JSON.stringify(doc.fields).slice(0, 100)}...</div>

                            <div className="flex gap-2">
                                <Button size="sm" variant="ghost" onClick={() => onCopy && onCopy(doc)} disabled={!onCopy}><Copy className="h-3 w-3" /></Button>
                            </div>

                        </CardContent>
                    </Card>
                ))}
            </div>
        )
    }

    if (mode === "table") {
        // Simple table view implementation
        if (!documents.length) return <div>No documents</div>;
        const keys = Array.from(new Set(documents.flatMap(d => Object.keys(d.fields))));
        return (
            <div className="overflow-auto border rounded-md">
                <table className="w-full text-sm">
                    <thead className="bg-muted">
                        <tr>
                            <th className="p-2 text-left">ID</th>
                            {keys.map(k => <th key={k} className="p-2 text-left">{k}</th>)}
                            <th className="p-2">Actions</th>
                        </tr>
                    </thead>
                    <tbody>
                        {documents.map(doc => (
                            <tr key={doc.id} className="border-t">
                                <td className="p-2 font-mono">{doc.id}</td>
                                {keys.map(k => (
                                    <td key={k} className="p-2 max-w-[200px] truncate">{text(doc.fields[k])}</td>
                                ))}

                                <td className="p-2">
                                    <Button size="sm" variant="ghost" onClick={() => onCopy && onCopy(doc)} disabled={!onCopy}><Copy className="h-3 w-3" /></Button>
                                </td>

                            </tr>
                        ))}
                    </tbody>
                </table>
            </div>
        )
    }

    // JSON Cards view (Default)
    return (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
            {documents.map((doc) => {
                const isEditing = editingDocId === doc.id;

                if (isEditing) {
                    return (
                        <Card key={doc.id} className="border-blue-300 bg-blue-50/50">
                            <CardContent className="space-y-3 pt-4">
                                <div className="flex items-center justify-between">
                                    <div className="font-semibold">{doc.id}</div>
                                    <div className="flex gap-1">
                                        {/* If we have editJsonDraft, we are in JSON mode */}
                                        {editJsonDraft && onEditJsonChange ? (
                                            <Button size="sm" onClick={() => onSaveEdit?.(doc, "json")}><Save className="h-3 w-3" /></Button>
                                        ) : (
                                            <Button size="sm" onClick={() => onSaveEdit?.(doc, "fields")}><Save className="h-3 w-3" /></Button>
                                        )}
                                        <Button size="sm" variant="ghost" onClick={onCancelEdit}><X className="h-3 w-3" /></Button>
                                    </div>
                                </div>

                                {editJsonDraft && onEditJsonChange ? (
                                    <Textarea
                                        className="min-h-[200px] font-mono text-sm"
                                        value={editJsonDraft}
                                        onChange={(e) => onEditJsonChange(e.target.value)}
                                    />
                                ) : (
                                    <div className="space-y-2">
                                        {Object.entries(doc.fields).map(([k]) => (
                                            <div key={k} className="grid grid-cols-[1fr_2fr] gap-2 items-center">
                                                <div className="text-xs text-muted-foreground text-right">{k}</div>
                                                <Input
                                                    className="h-7 text-xs"
                                                    value={editValues?.[k] || ""}
                                                    onChange={(e) => onEditFieldChange?.(k, e.target.value)}
                                                />
                                            </div>
                                        ))}
                                    </div>
                                )}
                            </CardContent>
                        </Card>
                    )
                }

                return (
                    <Card key={doc.id} className="border-[#d9cec2] bg-[#fffaf4]">
                        <CardContent className="space-y-2 pt-4">
                            <div className="flex items-start justify-between gap-2">
                                <div className="line-clamp-1 font-semibold">{doc.id}</div>
                                <div className="flex gap-1">
                                    {onDuplicate && <Button size="sm" variant="ghost" className="h-6 w-6 p-0" title="Duplicate" onClick={() => onDuplicate(doc)}><Repeat className="h-3 w-3" /></Button>}
                                    {onStartEdit && <Button size="sm" variant="ghost" className="h-6 w-6 p-0" title="Edit fields" onClick={() => onStartEdit(doc, "fields")}><FileEdit className="h-3 w-3" /></Button>}
                                    {onRequestDelete && <Button size="sm" variant="ghost" className="h-6 w-6 p-0 hover:text-red-600" title="Delete" onClick={() => onRequestDelete(doc)}><Trash2 className="h-3 w-3" /></Button>}
                                </div>
                            </div>
                            <div className="text-xs text-muted-foreground">schema={doc.schema} · v{doc.version}</div>
                            <div className="grid gap-1 border-t border-dashed border-[#d6c9bb] pt-2">
                                {Object.entries(doc.fields)
                                    .slice(0, 5)
                                    .map(([k, v]) => (
                                        <div key={k} className="flex items-start justify-between gap-2 text-sm">
                                            <span className="text-muted-foreground">{k}</span>
                                            <button
                                                className="mono max-w-[65%] truncate text-right hover:bg-black/5 px-1 rounded cursor-text"
                                                onClick={() => {
                                                    if (onCopy) onCopy(doc); // Or copy value
                                                }}
                                            >
                                                {text(v)}
                                            </button>
                                        </div>
                                    ))}
                            </div>

                            <div className="flex justify-end pt-2">
                                <button
                                    className="text-xs font-semibold text-[#6b4f3a] underline disabled:opacity-50"
                                    onClick={() => onCopy && onCopy(doc)}
                                    disabled={!onCopy}
                                >
                                    copy JSON
                                </button>
                            </div>

                        </CardContent>
                    </Card>
                );
            })}
        </div >
    );
}
