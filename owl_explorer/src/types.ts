
export type DatabaseEntry = {
    name: string;
    path: string;
    size_mb: number;
};

export type SchemaField = { name: string; type: string };
export type CheckOperatorDraft = "eq" | "ne" | "gt" | "gte" | "lt" | "lte";
export type SchemaConstraintDraft =
    | {
          kind: "unique";
          name?: string;
          fields: string[];
      }
    | {
          kind: "check";
          name?: string;
          field: string;
          op: CheckOperatorDraft;
          value: unknown;
      }
    | {
          kind: "ref";
          name?: string;
          field: string;
          target_schema: string;
          target_field?: string;
          required?: boolean;
      };

export type SchemaDetail = {
    name: string;
    fields: SchemaField[];
    constraints?: SchemaConstraintDraft[];
};

export type OwlDocument = {
    id: string;
    schema: string;
    version: number;
    created_at: string;
    updated_at: string;
    fields: Record<string, unknown>;
};

export type DocumentsPage = {
    schema: string;
    page: number;
    page_size: number;
    total: number;
    total_pages: number;
    documents: OwlDocument[];
};


export type QueryResponse = {
    mode: "single" | "script";
    count?: number;
    documents?: OwlDocument[];
    last_query_documents?: OwlDocument[];
    meta?: {
        execution_time_ms: number;
        documents_scanned: number;
        documents_returned: number;
    };
};


export type CollectionSummary = {
    name: string;
    document_count: number;
};

export type DocumentViewMode = "json" | "list" | "table";

export type WorkspaceTab = {
    id: string;
    type: "database" | "collection";
    label: string;
    database: string;
    schema?: string;
};

export type FieldDraft = {
    name: string;
    type: string;
};

export type ImportDocument = Record<string, unknown>;

export type PendingDeleteAction =
    | { kind: "one"; schema: string; id: string }
    | { kind: "all"; schema: string; count: number };
