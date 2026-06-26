
import { Database, RefreshCw, Trash2, WandSparkles, Plus, Upload, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { CollectionSummary, FieldDraft, ImportDocument, SchemaConstraintDraft } from "../../types";
import { ConstraintEditor } from "../editors/ConstraintEditor";

import { defaultFieldDraft, defaultIdFieldDraft } from "../../lib/schema-utils";
import { SCHEMA_TYPE_OPTIONS } from "../../lib/constants";

import { useState } from "react"; // Local state for forms

type DatabaseContextViewProps = {
    dbName: string;
    collections: CollectionSummary[];
    onRefresh: () => Promise<void>;
    onSeedDemo: () => Promise<void>;
    onDeleteDb: () => Promise<void>;
    onOpenCollection: (db: string, col: string) => void;
    // Create Collection logic
    onCreateCollection: (name: string, fields: FieldDraft[], constraints: SchemaConstraintDraft[]) => Promise<void>;
    onRebuildIndexes: () => Promise<void>;
    // Import logic
    onImport: (
        schema: string,
        fields: FieldDraft[],
        constraints: SchemaConstraintDraft[],
        docs: ImportDocument[],
        strategy: "merge" | "replace"
    ) => Promise<void>;
};

export function DatabaseContextView({
    dbName,
    collections,
    onRefresh,
    onSeedDemo,
    onDeleteDb,
    onOpenCollection,
    onCreateCollection,
    onRebuildIndexes,
    onImport,
}: DatabaseContextViewProps) {
    // Local state for forms which doesn't need to be global
    const [newCollectionName, setNewCollectionName] = useState("");
    const [newCollectionFields, setNewCollectionFields] = useState<FieldDraft[]>([defaultIdFieldDraft()]);
    const [newCollectionConstraints, setNewCollectionConstraints] = useState<SchemaConstraintDraft[]>([]);

    const [importSchemaName, setImportSchemaName] = useState("");
    const [importFieldsRaw, setImportFieldsRaw] = useState('[{"name":"id","type":"String"}]');
    const [importConstraints, setImportConstraints] = useState<SchemaConstraintDraft[]>([]);
    const [importDocumentsRaw, setImportDocumentsRaw] = useState('[{"id":"d1"}]');
    const [importStrategy, setImportStrategy] = useState<"merge" | "replace">("merge");

    function updateFieldRow(
        setRows: React.Dispatch<React.SetStateAction<FieldDraft[]>>,
        index: number,
        patch: Partial<FieldDraft>,
    ) {
        setRows((prev) => prev.map((row, i) => (i === index ? { ...row, ...patch } : row)));
    }

    return (
        <Card>
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Database Context</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
                <div className="flex items-center gap-2">
                    <Database className="h-4 w-4" />
                    <span className="font-semibold">{dbName}</span>
                    <Badge variant="subtle">{collections.length} collections</Badge>
                    <div className="ml-auto flex items-center gap-2">
                        <Button variant="secondary" size="sm" onClick={() => void onRefresh()} title="Refresh database">
                            <RefreshCw className="h-4 w-4" />
                        </Button>
                        <Button variant="secondary" size="sm" onClick={() => void onSeedDemo()} title="Seed demo data">
                            <WandSparkles className="h-4 w-4" />
                        </Button>
                        <Button variant="secondary" size="sm" onClick={() => void onRebuildIndexes()} title="Rebuild indexes">
                            <Wrench className="h-4 w-4" />
                        </Button>
                        <Button variant="secondary" size="sm" onClick={() => void onDeleteDb()} title="Delete database">
                            <Trash2 className="h-4 w-4" />
                        </Button>
                    </div>
                </div>

                <div className="overflow-hidden rounded-md border border-[#d6c8b9]">
                    <div className="grid grid-cols-[1fr_120px_100px] bg-[#f2e8db] px-3 py-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                        <div>Collection</div>
                        <div>Documents</div>
                        <div>Open</div>
                    </div>
                    <div className="divide-y divide-[#e3d7c8] bg-[#fffaf4]">
                        {collections.map((collection) => (
                            <div key={collection.name} className="grid grid-cols-[1fr_120px_100px] items-center px-3 py-2 text-sm">
                                <div>{collection.name}</div>
                                <div>{collection.document_count}</div>
                                <div>
                                    <Button variant="secondary" size="sm" onClick={() => onOpenCollection(dbName, collection.name)}>
                                        Open
                                    </Button>
                                </div>
                            </div>
                        ))}
                        {!collections.length && (
                            <div className="px-3 py-4 text-sm text-muted-foreground">No collections yet.</div>
                        )}
                    </div>
                </div>

                <Card className="border-[#d6c8b9] bg-[#fbf5eb]">
                    <CardHeader>
                        <CardTitle className="text-sm tracking-wide">Create Collection</CardTitle>
                    </CardHeader>
                    <CardContent className="space-y-2">
                        <Input
                            placeholder="collection name"
                            value={newCollectionName}
                            onChange={(e) => setNewCollectionName(e.target.value)}
                        />
                        {newCollectionFields.map((row, index) => (
                            <div key={`new-row-${index}`} className="grid grid-cols-[1fr_1fr_auto] gap-2">
                                <Input
                                    placeholder="field"
                                    value={row.name}
                                    onChange={(e) => updateFieldRow(setNewCollectionFields, index, { name: e.target.value })}
                                />
                                <Select
                                    value={row.type}
                                    onValueChange={(value) => updateFieldRow(setNewCollectionFields, index, { type: value })}
                                >
                                    <SelectTrigger>
                                        <SelectValue placeholder="type" />
                                    </SelectTrigger>
                                    <SelectContent>
                                        {SCHEMA_TYPE_OPTIONS.map((option) => (
                                            <SelectItem key={option} value={option}>
                                                {option}
                                            </SelectItem>
                                        ))}
                                    </SelectContent>
                                </Select>
                                <Button
                                    variant="secondary"
                                    onClick={() => setNewCollectionFields((prev) => prev.filter((_, i) => i !== index))}
                                    disabled={newCollectionFields.length <= 1}
                                >
                                    Remove
                                </Button>
                            </div>
                        ))}
                        <ConstraintEditor
                            constraints={newCollectionConstraints}
                            onChange={setNewCollectionConstraints}
                        />
                        <div className="flex gap-2">
                            <Button variant="secondary" onClick={() => setNewCollectionFields((prev) => [...prev, defaultFieldDraft()])}>
                                <Plus className="mr-2 h-4 w-4" />Add field
                            </Button>
                            <Button onClick={() => {
                                void onCreateCollection(newCollectionName, newCollectionFields, newCollectionConstraints);
                                setNewCollectionName("");
                                setNewCollectionFields([defaultIdFieldDraft()]);
                                setNewCollectionConstraints([]);
                            }}>Create collection</Button>
                        </div>
                    </CardContent>
                </Card>

                <Card className="border-[#d6c8b9] bg-[#fbf5eb]">
                    <CardHeader>
                        <CardTitle className="text-sm tracking-wide">Import Data + Schema</CardTitle>
                    </CardHeader>
                    <CardContent className="space-y-2">
                        <Input
                            placeholder="schema name"
                            value={importSchemaName}
                            onChange={(e) => setImportSchemaName(e.target.value)}
                        />
                        <Textarea
                            className="mono min-h-[100px]"
                            value={importFieldsRaw}
                            onChange={(e) => setImportFieldsRaw(e.target.value)}
                            placeholder='[{"name":"id","type":"String"},{"name":"profile","type":"Object"}]'
                        />
                        <ConstraintEditor
                            constraints={importConstraints}
                            onChange={setImportConstraints}
                        />
                        <Textarea
                            className="mono min-h-[120px]"
                            value={importDocumentsRaw}
                            onChange={(e) => setImportDocumentsRaw(e.target.value)}
                            placeholder='[{"id":"u1","profile":{"city":"Lagos"}}]'
                        />
                        <div className="flex items-center gap-2">
                            <Select
                                value={importStrategy}
                                onValueChange={(v) => setImportStrategy(v === "replace" ? "replace" : "merge")}
                            >
                                <SelectTrigger className="w-[180px]">
                                    <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                    <SelectItem value="merge">merge</SelectItem>
                                    <SelectItem value="replace">replace</SelectItem>
                                </SelectContent>
                            </Select>
                            <Button onClick={() => {
                                try {
                                    const fields = JSON.parse(importFieldsRaw) as FieldDraft[];
                                    const docs = JSON.parse(importDocumentsRaw) as ImportDocument[];
                                    void onImport(importSchemaName, fields, importConstraints, docs, importStrategy);
                                } catch (e) {
                                    console.error("Parse error", e);
                                }
                            }}>
                                <Upload className="mr-2 h-4 w-4" />Import
                            </Button>
                        </div>
                    </CardContent>
                </Card>
            </CardContent>
        </Card>
    );
}
