
import { FieldDraft, SchemaConstraintDraft } from "../types";

export function defaultFieldDraft(): FieldDraft {
    return { name: "", type: "String" };
}

export function defaultIdFieldDraft(): FieldDraft {
    return { name: "id", type: "String" };
}

export function serializeSchemaFields(
    fields: FieldDraft[],
    constraints: SchemaConstraintDraft[] = [],
): string {
    return JSON.stringify(
        {
            fields: fields
                .map((field) => ({ name: field.name.trim(), type: field.type.trim() }))
                .filter((field) => field.name && field.type),
            constraints,
        },
        null,
        2,
    );
}

export function parseSchemaJsonDraft(raw: string): FieldDraft[] {
    return parseSchemaBundleJsonDraft(raw).fields;
}

export function parseSchemaBundleJsonDraft(raw: string): {
    fields: FieldDraft[];
    constraints: SchemaConstraintDraft[];
} {
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        throw new Error("JSON schema must be an object");
    }

    const root = parsed as Record<string, unknown>;
    const fieldsNode = root.fields;
    let rows: FieldDraft[];

    if (Array.isArray(fieldsNode)) {
        rows = fieldsNode.map((item, index) => {
            if (!item || typeof item !== "object") {
                throw new Error(`fields[${index}] must be an object`);
            }
            const name = (item as { name?: unknown }).name;
            const type = (item as { type?: unknown }).type;
            if (typeof name !== "string" || !name.trim()) {
                throw new Error(`fields[${index}].name must be a non-empty string`);
            }
            if (typeof type !== "string" || !type.trim()) {
                throw new Error(`fields[${index}].type must be a non-empty string`);
            }
            return { name: name.trim(), type: type.trim() };
        });
    } else if (fieldsNode && typeof fieldsNode === "object" && !Array.isArray(fieldsNode)) {
        rows = Object.entries(fieldsNode as Record<string, unknown>).map(([name, type]) => {
            if (typeof type !== "string" || !type.trim()) {
                throw new Error(`Type for field '${name}' must be a non-empty string`);
            }
            return { name: name.trim(), type: type.trim() };
        });
    } else {
        // try reading root as key-value pairs
        const entries = Object.entries(root).filter(([key]) => key !== "name");
        rows = entries.map(([name, type]) => {
            if (typeof type !== "string" || !type.trim()) {
                throw new Error(`Type for field '${name}' must be a non-empty string`);
            }
            return { name: name.trim(), type: type.trim() };
        });
    }

    const constraintsNode = root.constraints;
    let constraints: SchemaConstraintDraft[] = [];
    if (constraintsNode !== undefined) {
        if (!Array.isArray(constraintsNode)) {
            throw new Error("constraints must be an array");
        }
        constraints = constraintsNode as SchemaConstraintDraft[];
    }

    if (!rows.length) {
        throw new Error("Schema must include at least one field");
    }
    return { fields: rows, constraints };
}

export function parseConstraintsJsonDraft(raw: string): SchemaConstraintDraft[] {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) {
        throw new Error("Constraints JSON must be an array");
    }
    return parsed as SchemaConstraintDraft[];
}
