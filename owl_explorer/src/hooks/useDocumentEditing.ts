
import { useState } from "react";
import { api, presentErrorMessage } from "../lib/api";
import { OwlDocument, PendingDeleteAction } from "../types";

export function useDocumentEditing(
    selectedDb: string,
    onStatusChange: (status: string) => void,
    onDocumentsChanged: () => Promise<void>
) {
    const [editingDocId, setEditingDocId] = useState<string | null>(null);
    const [editingDocSchema, setEditingDocSchema] = useState<string>("");
    const [editValues, setEditValues] = useState<Record<string, string>>({});
    const [editJsonDraft, setEditJsonDraft] = useState<string>("");
    const [pendingDelete, setPendingDelete] = useState<PendingDeleteAction | null>(null);

    function normalizeFieldInput(raw: string): unknown {
        const trimmed = raw.trim();
        if (!trimmed) return "";
        try {
            return JSON.parse(trimmed);
        } catch {
            return raw;
        }
    }

    function startEditingDocument(doc: OwlDocument, mode: "fields" | "json") {
        const nextValues: Record<string, string> = {};
        for (const [key, value] of Object.entries(doc.fields)) {
            if (typeof value === "string") {
                nextValues[key] = value;
            } else if (value === null) {
                nextValues[key] = "null";
            } else {
                nextValues[key] = JSON.stringify(value);
            }
        }
        setEditingDocId(doc.id);
        setEditingDocSchema(doc.schema);
        setEditValues(nextValues);
        setEditJsonDraft(JSON.stringify(doc.fields, null, 2));
        if (mode === "json") {
            onStatusChange("Editing JSON fields");
        }
    }

    function cancelEditingDocument() {
        setEditingDocId(null);
        setEditingDocSchema("");
        setEditValues({});
        setEditJsonDraft("");
    }

    async function saveEditedDocument(doc: OwlDocument, mode: "fields" | "json") {
        if (!selectedDb) return;
        try {
            const payloadFields: Record<string, unknown> = {};
            if (mode === "json") {
                const parsed = JSON.parse(editJsonDraft) as unknown;
                if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
                    onStatusChange("JSON editor must contain an object");
                    return;
                }
                for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
                    payloadFields[key] = value;
                }
            } else {
                for (const [key, value] of Object.entries(editValues)) {
                    payloadFields[key] = normalizeFieldInput(value);
                }
            }
            onStatusChange("Saving document...");
            await api<{ document: OwlDocument }>("/api/documents/update", {
                database: selectedDb,
                schema: doc.schema,
                id: doc.id,
                fields: payloadFields,
                replace: mode === "json",
            });
            cancelEditingDocument();
            await onDocumentsChanged();
            onStatusChange("Document updated");
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    async function duplicateDocument(doc: OwlDocument) {
        if (!selectedDb) return;
        try {
            onStatusChange("Duplicating document...");
            await api<{ document: OwlDocument }>("/api/documents/duplicate", {
                database: selectedDb,
                schema: doc.schema,
                id: doc.id,
            });
            await onDocumentsChanged();
            onStatusChange("Document duplicated");
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    async function confirmDeleteAction() {
        if (!selectedDb || !pendingDelete) return;
        try {
            if (pendingDelete.kind === "one") {
                onStatusChange("Deleting document...");
                await api("/api/documents/delete", {
                    database: selectedDb,
                    schema: pendingDelete.schema,
                    id: pendingDelete.id,
                });
            } else {
                onStatusChange("Deleting all documents...");
                await api("/api/documents/delete_all", {
                    database: selectedDb,
                    schema: pendingDelete.schema,
                });
            }
            setPendingDelete(null);
            cancelEditingDocument();
            await onDocumentsChanged();
            onStatusChange("Delete completed");
        } catch (e) {
            onStatusChange(presentErrorMessage((e as Error).message));
        }
    }

    return {
        editingDocId,
        editingDocSchema,
        editValues,
        setEditValues,
        editJsonDraft,
        setEditJsonDraft,
        pendingDelete,
        setPendingDelete,
        startEditingDocument,
        cancelEditingDocument,
        saveEditedDocument,
        duplicateDocument,
        confirmDeleteAction,
    };
}
