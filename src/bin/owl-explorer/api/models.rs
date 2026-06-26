use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseEntry {
    pub name: String,
    pub path: String,
    pub size_mb: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub databases: Vec<DatabaseEntry>,
}

impl Registry {
    pub fn load(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Registry::default(),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, raw).map_err(|e| e.to_string())
    }

    pub fn upsert(&mut self, entry: DatabaseEntry) {
        if let Some(existing) = self.databases.iter_mut().find(|db| db.name == entry.name) {
            *existing = entry;
        } else {
            self.databases.push(entry);
            self.databases.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    pub fn get(&self, name: &str) -> Option<&DatabaseEntry> {
        self.databases.iter().find(|db| db.name == name)
    }

    pub fn remove(&mut self, name: &str) -> Option<DatabaseEntry> {
        let idx = self.databases.iter().position(|db| db.name == name)?;
        Some(self.databases.remove(idx))
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateDbRequest {
    pub name: String,
    pub path: String,
    pub size_mb: u64,
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub database: String,
    pub query: String,
}

#[derive(Debug, Deserialize)]
pub struct ExplainQueryRequest {
    pub database: String,
    pub query: String,
}

#[derive(Debug, Deserialize)]
pub struct SchemasRequest {
    pub database: String,
}

#[derive(Debug, Deserialize)]
pub struct SchemaDetailRequest {
    pub database: String,
    pub schema: String,
}

#[derive(Debug, Deserialize)]
pub struct DocumentsRequest {
    pub database: String,
    pub schema: String,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDocumentRequest {
    pub database: String,
    pub schema: String,
    pub id: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
    pub replace: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteDocumentRequest {
    pub database: String,
    pub schema: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteAllDocumentsRequest {
    pub database: String,
    pub schema: String,
}

#[derive(Debug, Deserialize)]
pub struct DuplicateDocumentRequest {
    pub database: String,
    pub schema: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CollectionsRequest {
    pub database: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertCollectionField {
    pub name: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConstraintDef {
    #[serde(rename = "kind")]
    pub kind: String, // unique | check | ref
    pub name: Option<String>,
    pub fields: Option<Vec<String>>,
    pub field: Option<String>,
    pub op: Option<String>, // eq | ne | gt | gte | lt | lte
    pub value: Option<serde_json::Value>,
    pub target_schema: Option<String>,
    pub target_field: Option<String>,
    pub required: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertCollectionRequest {
    pub database: String,
    pub name: String,
    pub fields: Vec<UpsertCollectionField>,
    pub constraints: Option<Vec<ConstraintDef>>,
    pub strategy: Option<String>, // "replace" | "merge"
}

#[derive(Debug, Deserialize)]
pub struct DeleteDatabaseRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteCollectionRequest {
    pub database: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportSchemaField {
    pub name: String,
    pub r#type: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportDataRequest {
    pub database: String,
    pub schema: String,
    pub fields: Vec<ImportSchemaField>,
    pub constraints: Option<Vec<ConstraintDef>>,
    pub documents: Vec<serde_json::Map<String, serde_json::Value>>,
    pub strategy: Option<String>, // "replace" | "merge"
}

#[derive(Debug, Deserialize)]
pub struct SeedDemoRequest {
    pub database: String,
}

#[derive(Debug, Deserialize)]
pub struct MetricsRequest {
    pub database: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RebuildIndexesRequest {
    pub database: String,
    pub backend: Option<String>, // zig | memory
}

#[derive(Debug, Deserialize)]
pub struct ListVectorIndexesRequest {
    pub database: String,
    pub schema: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertVectorIndexRequest {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub field: String,
    pub dimension: u32,
    pub metric: String,     // cosine | l2 | dot
    pub index_type: String, // exact | hnsw
    pub quantization: Option<String>,
    pub state: Option<String>, // creating | ready | rebuilding | degraded | failed
}

#[derive(Debug, Deserialize)]
pub struct DeleteVectorIndexRequest {
    pub database: String,
    pub schema: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct VectorIndexStatusRequest {
    pub database: String,
    pub schema: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct BuildVectorIndexRequest {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub force: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AIChatRequest {
    pub messages: Vec<AIChatMessage>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AIChatMessage {
    pub id: Option<String>,
    pub role: String,
    pub content: String,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AIGenerateQueryRequest {
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
pub struct AIExplainQueryRequest {
    pub query: String,
}
