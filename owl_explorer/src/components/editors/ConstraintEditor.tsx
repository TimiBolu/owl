import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SchemaConstraintDraft } from "../../types";

type ConstraintEditorProps = {
    constraints: SchemaConstraintDraft[];
    onChange: (constraints: SchemaConstraintDraft[]) => void;
};

type ConstraintKind = "unique" | "check" | "ref";

type ConstraintRow = {
    kind: ConstraintKind;
    name: string;
    fieldsCsv: string;
    field: string;
    op: "eq" | "ne" | "gt" | "gte" | "lt" | "lte";
    value: string;
    targetSchema: string;
    targetField: string;
    required: boolean;
};

function toRows(input: SchemaConstraintDraft[]): ConstraintRow[] {
    return input.map((item) => {
        if (item.kind === "unique") {
            return {
                kind: "unique",
                name: item.name ?? "",
                fieldsCsv: (item.fields ?? []).join(", "),
                field: "",
                op: "eq",
                value: "",
                targetSchema: "",
                targetField: "",
                required: true,
            };
        }
        if (item.kind === "check") {
            return {
                kind: "check",
                name: item.name ?? "",
                fieldsCsv: "",
                field: item.field,
                op: item.op,
                value: JSON.stringify(item.value),
                targetSchema: "",
                targetField: "",
                required: true,
            };
        }
        return {
            kind: "ref",
            name: item.name ?? "",
            fieldsCsv: "",
            field: item.field,
            op: "eq",
            value: "",
            targetSchema: item.target_schema,
            targetField: item.target_field ?? "",
            required: item.required ?? true,
        };
    });
}

function parseMaybeJson(raw: string): unknown {
    const trimmed = raw.trim();
    if (!trimmed) return "";
    try {
        return JSON.parse(trimmed);
    } catch {
        return trimmed;
    }
}

function fromRows(rows: ConstraintRow[]): SchemaConstraintDraft[] {
    const out: SchemaConstraintDraft[] = [];
    for (const row of rows) {
        const name = row.name.trim() || undefined;
        if (row.kind === "unique") {
            const fields = row.fieldsCsv
                .split(",")
                .map((f) => f.trim())
                .filter(Boolean);
            if (fields.length) out.push({ kind: "unique", name, fields });
            continue;
        }
        if (row.kind === "check") {
            const field = row.field.trim();
            if (!field) continue;
            out.push({
                kind: "check",
                name,
                field,
                op: row.op,
                value: parseMaybeJson(row.value),
            });
            continue;
        }
        const field = row.field.trim();
        const targetSchema = row.targetSchema.trim();
        if (!field || !targetSchema) continue;
        out.push({
            kind: "ref",
            name,
            field,
            target_schema: targetSchema,
            target_field: row.targetField.trim() || undefined,
            required: row.required,
        });
    }
    return out;
}

function blankRow(kind: ConstraintKind = "unique"): ConstraintRow {
    return {
        kind,
        name: "",
        fieldsCsv: "",
        field: "",
        op: "eq",
        value: "",
        targetSchema: "",
        targetField: "",
        required: true,
    };
}

export function ConstraintEditor({ constraints, onChange }: ConstraintEditorProps) {
    const rows = toRows(constraints);

    function updateRow(index: number, patch: Partial<ConstraintRow>) {
        const next = rows.map((row, i) => (i === index ? { ...row, ...patch } : row));
        onChange(fromRows(next));
    }

    function removeRow(index: number) {
        onChange(fromRows(rows.filter((_, i) => i !== index)));
    }

    function addRow(kind: ConstraintKind) {
        onChange(fromRows([...rows, blankRow(kind)]));
    }

    return (
        <div className="space-y-2 rounded-md border border-[#d6cabd] bg-[#f8f2e8] p-3">
            <div className="flex items-center justify-between gap-2">
                <div className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Constraints</div>
                <div className="flex items-center gap-1">
                    <Button size="sm" variant="secondary" onClick={() => addRow("unique")}>
                        <Plus className="mr-1 h-3 w-3" />
                        Unique
                    </Button>
                    <Button size="sm" variant="secondary" onClick={() => addRow("check")}>
                        <Plus className="mr-1 h-3 w-3" />
                        Check
                    </Button>
                    <Button size="sm" variant="secondary" onClick={() => addRow("ref")}>
                        <Plus className="mr-1 h-3 w-3" />
                        Ref
                    </Button>
                </div>
            </div>

            {!rows.length && <div className="text-xs text-muted-foreground">No constraints</div>}

            {rows.map((row, index) => (
                <div key={`constraint-row-${index}`} className="space-y-2 rounded-md border border-[#dfd3c5] bg-[#fffaf4] p-2">
                    <div className="grid grid-cols-[120px_1fr_auto] gap-2">
                        <Select
                            value={row.kind}
                            onValueChange={(value) => {
                                const nextKind = value as ConstraintKind;
                                updateRow(index, { ...blankRow(nextKind), kind: nextKind });
                            }}
                        >
                            <SelectTrigger>
                                <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                                <SelectItem value="unique">unique</SelectItem>
                                <SelectItem value="check">check</SelectItem>
                                <SelectItem value="ref">ref</SelectItem>
                            </SelectContent>
                        </Select>
                        <Input
                            placeholder="constraint name (optional)"
                            value={row.name}
                            onChange={(e) => updateRow(index, { name: e.target.value })}
                        />
                        <Button variant="secondary" onClick={() => removeRow(index)} title="Remove constraint">
                            <Trash2 className="h-4 w-4" />
                        </Button>
                    </div>

                    {row.kind === "unique" && (
                        <Input
                            placeholder="fields (comma-separated), e.g. email or org_id, email"
                            value={row.fieldsCsv}
                            onChange={(e) => updateRow(index, { fieldsCsv: e.target.value })}
                        />
                    )}

                    {row.kind === "check" && (
                        <div className="grid grid-cols-[1fr_130px_1fr] gap-2">
                            <Input
                                placeholder="field path, e.g. age"
                                value={row.field}
                                onChange={(e) => updateRow(index, { field: e.target.value })}
                            />
                            <Select value={row.op} onValueChange={(value) => updateRow(index, { op: value as ConstraintRow["op"] })}>
                                <SelectTrigger>
                                    <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                    <SelectItem value="eq">eq</SelectItem>
                                    <SelectItem value="ne">ne</SelectItem>
                                    <SelectItem value="gt">gt</SelectItem>
                                    <SelectItem value="gte">gte</SelectItem>
                                    <SelectItem value="lt">lt</SelectItem>
                                    <SelectItem value="lte">lte</SelectItem>
                                </SelectContent>
                            </Select>
                            <Input
                                placeholder='value (JSON), e.g. 18 or "paid"'
                                value={row.value}
                                onChange={(e) => updateRow(index, { value: e.target.value })}
                            />
                        </div>
                    )}

                    {row.kind === "ref" && (
                        <div className="grid grid-cols-2 gap-2">
                            <Input
                                placeholder="field, e.g. company_id"
                                value={row.field}
                                onChange={(e) => updateRow(index, { field: e.target.value })}
                            />
                            <Input
                                placeholder="target schema, e.g. companies"
                                value={row.targetSchema}
                                onChange={(e) => updateRow(index, { targetSchema: e.target.value })}
                            />
                            <Input
                                placeholder="target field (optional), default id"
                                value={row.targetField}
                                onChange={(e) => updateRow(index, { targetField: e.target.value })}
                            />
                            <Select
                                value={row.required ? "true" : "false"}
                                onValueChange={(value) => updateRow(index, { required: value === "true" })}
                            >
                                <SelectTrigger>
                                    <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                    <SelectItem value="true">required</SelectItem>
                                    <SelectItem value="false">optional</SelectItem>
                                </SelectContent>
                            </Select>
                        </div>
                    )}
                </div>
            ))}
        </div>
    );
}
