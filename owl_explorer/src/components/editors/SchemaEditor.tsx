
import { useState, useEffect } from "react";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { FieldDraft, SchemaConstraintDraft, SchemaDetail } from "../../types";
import { ConstraintEditor } from "./ConstraintEditor";

import {
    defaultFieldDraft,
    serializeSchemaFields,
    parseSchemaBundleJsonDraft,
} from "../../lib/schema-utils";
import { SCHEMA_TYPE_OPTIONS } from "../../lib/constants";


type SchemaEditorProps = {
    schemaDetail: SchemaDetail | null;
    onSave: (fields: FieldDraft[], constraints: SchemaConstraintDraft[]) => Promise<void>;
    onStatusChange: (status: string) => void;
};

export function SchemaEditor({ schemaDetail, onSave, onStatusChange }: SchemaEditorProps) {
    const [schemaEditorFields, setSchemaEditorFields] = useState<FieldDraft[]>([]);
    const [schemaEditorConstraints, setSchemaEditorConstraints] = useState<SchemaConstraintDraft[]>([]);
    const [schemaEditorMode, setSchemaEditorMode] = useState<"form" | "json">("form");
    const [schemaJsonDraft, setSchemaJsonDraft] = useState("{}");
    const [schemaJsonError, setSchemaJsonError] = useState("");

    useEffect(() => {
        if (!schemaDetail) {
            setSchemaEditorFields([]);
            setSchemaJsonDraft("{}");
            return;
        }
        const next = schemaDetail.fields.map((f) => ({ name: f.name, type: f.type }));
        const nextConstraints = schemaDetail.constraints ?? [];
        setSchemaEditorFields(next);
        setSchemaEditorConstraints(nextConstraints);
        setSchemaJsonDraft(serializeSchemaFields(next, nextConstraints));
        setSchemaJsonError("");
    }, [schemaDetail]);

    function updateFieldRow(
        setRows: React.Dispatch<React.SetStateAction<FieldDraft[]>>,
        index: number,
        patch: Partial<FieldDraft>,
    ) {
        setRows((prev) => prev.map((row, i) => (i === index ? { ...row, ...patch } : row)));
    }

    function applySchemaJsonToForm() {
        try {
            const parsed = parseSchemaBundleJsonDraft(schemaJsonDraft);
            setSchemaEditorFields(parsed.fields);
            setSchemaEditorConstraints(parsed.constraints ?? []);
            setSchemaJsonError("");
            onStatusChange(`Parsed ${parsed.fields.length} field(s) from JSON`);
        } catch (e) {
            const message = (e as Error).message;
            setSchemaJsonError(message);
            onStatusChange(message);
        }
    }

    return (
        <Card>
            <CardHeader>
                <div className="flex flex-wrap items-center justify-between gap-2">
                    <CardTitle className="text-sm tracking-wide">Update Schema</CardTitle>
                    <div className="flex gap-2">
                        <Button
                            size="sm"
                            variant={schemaEditorMode === "form" ? "default" : "secondary"}
                            onClick={() => setSchemaEditorMode("form")}
                        >
                            Form
                        </Button>
                        <Button
                            size="sm"
                            variant={schemaEditorMode === "json" ? "default" : "secondary"}
                            onClick={() => setSchemaEditorMode("json")}
                        >
                            JSON
                        </Button>
                    </div>
                </div>
            </CardHeader>
            <CardContent className="space-y-2">
                {schemaEditorMode === "form" ? (
                    <>
                        {schemaEditorFields.map((row, index) => (
                            <div key={`schema-row-${index}`} className="grid grid-cols-[1fr_1fr_auto] gap-2">
                                <Input
                                    placeholder="field"
                                    value={row.name}
                                    onChange={(e) => updateFieldRow(setSchemaEditorFields, index, { name: e.target.value })}
                                />
                                <Select
                                    value={row.type}
                                    onValueChange={(value) => updateFieldRow(setSchemaEditorFields, index, { type: value })}
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
                                    onClick={() => setSchemaEditorFields((prev) => prev.filter((_, i) => i !== index))}
                                    disabled={schemaEditorFields.length <= 1}
                                >
                                    Remove
                                </Button>
                            </div>
                        ))}
                        <div className="flex gap-2">
                            <Button variant="secondary" onClick={() => setSchemaEditorFields((prev) => [...prev, defaultFieldDraft()])}>
                                <Plus className="mr-2 h-4 w-4" />Add field
                            </Button>
                            <Button
                                variant="secondary"
                                onClick={() => {
                                    setSchemaJsonDraft(
                                        serializeSchemaFields(
                                            schemaEditorFields,
                                            schemaEditorConstraints,
                                        ),
                                    );
                                    setSchemaJsonError("");
                                }}
                            >
                                Sync to JSON
                            </Button>
                            <Button
                                onClick={async () => {
                                    try {
                                        await onSave(schemaEditorFields, schemaEditorConstraints);
                                    } catch (e) {
                                        const message = (e as Error).message;
                                        setSchemaJsonError(message);
                                        onStatusChange(message);
                                    }
                                }}
                            >
                                Save schema
                            </Button>
                        </div>
                        <ConstraintEditor
                            constraints={schemaEditorConstraints}
                            onChange={setSchemaEditorConstraints}
                        />
                    </>
                ) : (
                    <>
                        <p className="text-xs text-muted-foreground">
                            Edit as a field map JSON object or use {`{ \"fields\": [{\"name\":\"x\",\"type\":\"String\"}] }`}.
                        </p>
                        <Textarea
                            className="mono min-h-[240px]"
                            value={schemaJsonDraft}
                            onChange={(e) => setSchemaJsonDraft(e.target.value)}
                        />
                        {schemaJsonError && (
                            <div className="rounded-md border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800">
                                {schemaJsonError}
                            </div>
                        )}
                        <div className="flex gap-2">
                            <Button variant="secondary" onClick={applySchemaJsonToForm}>
                                Parse JSON
                            </Button>
                            <Button
                                variant="secondary"
                                onClick={() => {
                                    setSchemaJsonDraft(
                                        serializeSchemaFields(schemaEditorFields, schemaEditorConstraints),
                                    );
                                    setSchemaJsonError("");
                                }}
                            >
                                Reset from form
                            </Button>
                            <Button
                                onClick={async () => {
                                    let parsed: { fields: FieldDraft[]; constraints: SchemaConstraintDraft[] };
                                    try {
                                        parsed = parseSchemaBundleJsonDraft(schemaJsonDraft);
                                    } catch (e) {
                                        const message = (e as Error).message;
                                        setSchemaJsonError(message);
                                        onStatusChange(message);
                                        return;
                                    }
                                    setSchemaEditorFields(parsed.fields);
                                    setSchemaEditorConstraints(parsed.constraints ?? []);
                                    setSchemaJsonError("");
                                    await onSave(parsed.fields, parsed.constraints ?? []);
                                }}
                            >
                                Save schema
                            </Button>
                        </div>
                    </>
                )}
            </CardContent>
        </Card>
    );
}
