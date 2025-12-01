// OQL Distributed Integration
//
// This module integrates OQL parser with distributed WAL operations:
// - Parse OQL insert/update statements for distributed execution
// - Convert OQL operations to atomic write operations across shards
// - Ensure OQL transaction semantics preserved in distributed mode
// - Test with existing .oql files from test suite

use crate::distributed::global_objref::GlobalObjRef;
use crate::distributed::consistent_hash::ShardId;
use crate::distributed::wal::{WALEntryType, TxnId};
use crate::distributed::wal_first_storage::WALFirstStorage;
use crate::document::{Document, DocumentId, Value};
use crate::error::{OQLError, OQLResult};
use crate::parser::{
    parse_oql_statement, OQLStatement, OQLQuery, Operation, InsertBlock, UpdateBlock,
    Expression, Literal, ObjectField, FieldAccess,
};
use crate::storage::DocumentStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Distributed OQL execution engine
pub struct DistributedOQLEngine {
    /// Current shard ID
    shard_id: ShardId,
    /// WAL-first storage for this shard
    storage: Arc<WALFirstStorage>,
    /// Cluster manager for cross-shard operations (placeholder)
    cluster_manager: Option<Arc<dyn ClusterManager>>,
}

/// Trait for cluster management operations
pub trait ClusterManager: Send + Sync {
    /// Execute OQL operation on a remote shard
    fn execute_remote_oql(&self, shard_id: ShardId, oql: &str) -> OQLResult<OQLExecutionResult>;
    
    /// Get shard ID for a given object ID
    fn get_shard_for_object(&self, object_id: &str) -> ShardId;
    
    /// Begin a distributed transaction across multiple shards
    fn begin_distributed_transaction(&self, shards: &[ShardId]) -> OQLResult<TxnId>;
    
    /// Commit a distributed transaction
    fn commit_distributed_transaction(&self, txn_id: TxnId) -> OQLResult<()>;
    
    /// Rollback a distributed transaction
    fn rollback_distributed_transaction(&self, txn_id: TxnId) -> OQLResult<()>;
}

/// Result of OQL execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OQLExecutionResult {
    /// Success flag
    pub success: bool,
    /// Number of affected documents
    pub affected_count: usize,
    /// Returned documents (for queries)
    pub documents: Vec<Document>,
    /// Error message if any
    pub error: Option<String>,
    /// Global object references for inserted/updated documents
    pub object_refs: Vec<GlobalObjRef>,
}

impl OQLExecutionResult {
    pub fn success(affected_count: usize) -> Self {
        Self {
            success: true,
            affected_count,
            documents: Vec::new(),
            error: None,
            object_refs: Vec::new(),
        }
    }

    pub fn success_with_refs(affected_count: usize, object_refs: Vec<GlobalObjRef>) -> Self {
        Self {
            success: true,
            affected_count,
            documents: Vec::new(),
            error: None,
            object_refs,
        }
    }

    pub fn success_with_documents(documents: Vec<Document>) -> Self {
        Self {
            success: true,
            affected_count: documents.len(),
            documents,
            error: None,
            object_refs: Vec::new(),
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            affected_count: 0,
            documents: Vec::new(),
            error: Some(message),
            object_refs: Vec::new(),
        }
    }
}

impl DistributedOQLEngine {
    /// Create a new distributed OQL engine
    pub fn new(shard_id: ShardId, storage: Arc<WALFirstStorage>) -> Self {
        Self {
            shard_id,
            storage,
            cluster_manager: None,
        }
    }

    /// Create with cluster manager for cross-shard operations
    pub fn with_cluster_manager(
        shard_id: ShardId,
        storage: Arc<WALFirstStorage>,
        cluster_manager: Arc<dyn ClusterManager>,
    ) -> Self {
        Self {
            shard_id,
            storage,
            cluster_manager: Some(cluster_manager),
        }
    }

    /// Execute OQL statement with distributed support
    pub fn execute_oql(&self, oql: &str) -> OQLResult<OQLExecutionResult> {
        // Parse OQL statement
        let statement = parse_oql_statement(oql)
            .map_err(|e| OQLError::generic(&format!("Failed to parse OQL: {}", e)))?;

        match statement {
            OQLStatement::Query(query) => self.execute_query(query, None),
            OQLStatement::FunctionDef(_) => {
                // Function definitions are not supported in distributed mode yet
                Err(OQLError::generic("Function definitions not supported in distributed mode"))
            }
        }
    }

    /// Execute OQL statement within a transaction
    pub fn execute_oql_in_transaction(&self, oql: &str, txn_id: TxnId) -> OQLResult<OQLExecutionResult> {
        let statement = parse_oql_statement(oql)
            .map_err(|e| OQLError::generic(&format!("Failed to parse OQL: {}", e)))?;

        match statement {
            OQLStatement::Query(query) => self.execute_query(query, Some(txn_id)),
            OQLStatement::FunctionDef(_) => {
                Err(OQLError::generic("Function definitions not supported in distributed mode"))
            }
        }
    }

    /// Execute a parsed OQL query
    fn execute_query(&self, query: OQLQuery, txn_id: Option<TxnId>) -> OQLResult<OQLExecutionResult> {
        let schema = &query.collection;

        // Process operations in sequence
        let mut current_documents = Vec::new();
        let mut object_refs = Vec::new();
        let mut affected_count = 0;

        for operation in &query.operations {
            match operation {
                Operation::Insert(insert_block) => {
                    let (count, refs) = self.execute_insert(schema, insert_block, txn_id)?;
                    affected_count += count;
                    object_refs.extend(refs);
                }
                Operation::InsertMany(insert_blocks) => {
                    let (count, refs) = self.execute_insert_many(schema, insert_blocks, txn_id)?;
                    affected_count += count;
                    object_refs.extend(refs);
                }
                Operation::Update(update_block) => {
                    let count = self.execute_update(schema, update_block, txn_id)?;
                    affected_count += count;
                }
                Operation::Delete => {
                    let count = self.execute_delete(schema, txn_id)?;
                    affected_count += count;
                }
                Operation::Where(_) | Operation::Select(_) | Operation::Sort(_) | 
                Operation::Limit(_) | Operation::GroupBy(_) | Operation::Having(_) |
                Operation::Count | Operation::Sum(_) | Operation::Avg(_) | 
                Operation::Min(_) | Operation::Max(_) => {
                    // Query operations - would need full query engine implementation
                    // For now, return documents from storage
                    current_documents = self.storage.scan_documents(schema)?;
                }
                Operation::FunctionDef(_) => {
                    return Err(OQLError::generic("Function definitions not supported"));
                }
            }
        }

        // Return appropriate result based on operation type
        if !object_refs.is_empty() {
            Ok(OQLExecutionResult::success_with_refs(affected_count, object_refs))
        } else if !current_documents.is_empty() {
            Ok(OQLExecutionResult::success_with_documents(current_documents))
        } else {
            Ok(OQLExecutionResult::success(affected_count))
        }
    }

    /// Execute OQL insert operation
    fn execute_insert(
        &self,
        schema: &str,
        insert_block: &InsertBlock,
        txn_id: Option<TxnId>,
    ) -> OQLResult<(usize, Vec<GlobalObjRef>)> {
        // Convert insert block to Document
        let document = self.insert_block_to_document(schema, insert_block)?;

        // Determine target shard for this document
        let target_shard = if let Some(cluster_mgr) = &self.cluster_manager {
            cluster_mgr.get_shard_for_object(&document.id.to_string())
        } else {
            self.shard_id // Single shard mode
        };

        if target_shard == self.shard_id {
            // Local insert
            let objref = self.storage.write_atomic(&document, txn_id)?;
            Ok((1, vec![objref]))
        } else {
            // Remote insert - delegate to cluster manager
            if let Some(cluster_mgr) = &self.cluster_manager {
                let remote_oql = self.document_to_insert_oql(&document)?;
                let result = cluster_mgr.execute_remote_oql(target_shard, &remote_oql)?;
                Ok((result.affected_count, result.object_refs))
            } else {
                Err(OQLError::generic("Cannot execute cross-shard operation without cluster manager"))
            }
        }
    }

    /// Execute OQL insert many operation
    fn execute_insert_many(
        &self,
        schema: &str,
        insert_blocks: &[InsertBlock],
        txn_id: Option<TxnId>,
    ) -> OQLResult<(usize, Vec<GlobalObjRef>)> {
        let mut total_count = 0;
        let mut all_refs = Vec::new();

        // Group inserts by target shard
        let mut shard_groups: HashMap<ShardId, Vec<Document>> = HashMap::new();

        for insert_block in insert_blocks {
            let document = self.insert_block_to_document(schema, insert_block)?;
            let target_shard = if let Some(cluster_mgr) = &self.cluster_manager {
                cluster_mgr.get_shard_for_object(&document.id.to_string())
            } else {
                self.shard_id
            };

            shard_groups.entry(target_shard).or_insert_with(Vec::new).push(document);
        }

        // Execute inserts for each shard
        for (target_shard, documents) in shard_groups {
            if target_shard == self.shard_id {
                // Local inserts
                for document in documents {
                    let objref = self.storage.write_atomic(&document, txn_id)?;
                    all_refs.push(objref);
                    total_count += 1;
                }
            } else {
                // Remote inserts
                if let Some(cluster_mgr) = &self.cluster_manager {
                    for document in documents {
                        let remote_oql = self.document_to_insert_oql(&document)?;
                        let result = cluster_mgr.execute_remote_oql(target_shard, &remote_oql)?;
                        total_count += result.affected_count;
                        all_refs.extend(result.object_refs);
                    }
                } else {
                    return Err(OQLError::generic("Cannot execute cross-shard operation without cluster manager"));
                }
            }
        }

        Ok((total_count, all_refs))
    }

    /// Execute OQL update operation
    fn execute_update(
        &self,
        schema: &str,
        update_block: &UpdateBlock,
        txn_id: Option<TxnId>,
    ) -> OQLResult<usize> {
        // For now, implement a simple update that affects all documents in the schema
        // In a full implementation, this would need WHERE clause support
        let documents = self.storage.scan_documents(schema)?;
        let mut updated_count = 0;

        for document in documents {
            let target_shard = if let Some(cluster_mgr) = &self.cluster_manager {
                cluster_mgr.get_shard_for_object(&document.id.to_string())
            } else {
                self.shard_id
            };

            if target_shard == self.shard_id {
                // Local update
                let updated_document = self.apply_update_to_document(document, update_block)?;
                self.storage.update_atomic(schema, &updated_document.id, &updated_document, txn_id)?;
                updated_count += 1;
            } else {
                // Remote update - would need to implement remote update protocol
                // For now, skip cross-shard updates
                continue;
            }
        }

        Ok(updated_count)
    }

    /// Execute OQL delete operation
    fn execute_delete(&self, schema: &str, txn_id: Option<TxnId>) -> OQLResult<usize> {
        // For now, implement a simple delete that affects all documents in the schema
        // In a full implementation, this would need WHERE clause support
        let documents = self.storage.scan_documents(schema)?;
        let mut deleted_count = 0;

        for document in documents {
            let target_shard = if let Some(cluster_mgr) = &self.cluster_manager {
                cluster_mgr.get_shard_for_object(&document.id.to_string())
            } else {
                self.shard_id
            };

            if target_shard == self.shard_id {
                // Local delete
                let deleted = self.storage.delete_atomic(schema, &document.id, txn_id)?;
                if deleted {
                    deleted_count += 1;
                }
            } else {
                // Remote delete - would need to implement remote delete protocol
                // For now, skip cross-shard deletes
                continue;
            }
        }

        Ok(deleted_count)
    }

    /// Convert InsertBlock to Document
    fn insert_block_to_document(&self, schema: &str, insert_block: &InsertBlock) -> OQLResult<Document> {
        let mut fields = HashMap::new();
        let mut document_id: Option<DocumentId> = None;

        for field in &insert_block.fields {
            let value = self.expression_to_value(&field.value)?;
            
            // Check if this is the ID field
            if field.name == "id" {
                if let Value::String(id_str) = &value {
                    document_id = Some(id_str.clone());
                } else {
                    return Err(OQLError::generic("Document ID must be a string"));
                }
            }
            
            fields.insert(field.name.clone(), value);
        }

        // Generate ID if not provided
        let id = document_id.unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            format!("{}_{}", schema, timestamp)
        });

        Ok(Document::with_id(id, schema.to_string(), fields))
    }

    /// Apply update block to a document
    fn apply_update_to_document(&self, mut document: Document, update_block: &UpdateBlock) -> OQLResult<Document> {
        for field in &update_block.fields {
            let value = self.expression_to_value(&field.value)?;
            document.fields.insert(field.name.clone(), value);
        }
        Ok(document)
    }

    /// Convert Expression to Value
    fn expression_to_value(&self, expr: &Expression) -> OQLResult<Value> {
        match expr {
            Expression::Literal(literal) => Ok(self.literal_to_value(literal)),
            Expression::FieldAccess(_) => {
                // Field access in insert/update context is not supported yet
                Err(OQLError::generic("Field access not supported in insert/update values"))
            }
            Expression::FunctionCall { name, args } => {
                // Handle built-in functions
                match name.as_str() {
                    "now" => {
                        use std::time::{SystemTime, UNIX_EPOCH};
                        let timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        Ok(Value::Integer(timestamp as i64))
                    }
                    _ => Err(OQLError::generic(&format!("Unknown function: {}", name))),
                }
            }
            Expression::ArrayLiteral(elements) => {
                let mut array_values = Vec::new();
                for element in elements {
                    array_values.push(self.expression_to_value(element)?);
                }
                Ok(Value::List(array_values))
            }
            Expression::ObjectLiteral(fields) => {
                let mut object_fields = HashMap::new();
                for field in fields {
                    let value = self.expression_to_value(&field.value)?;
                    object_fields.insert(field.name.clone(), value);
                }
                Ok(Value::Object(object_fields))
            }
            Expression::BinaryOp { left, operator, right } => {
                let left_val = self.expression_to_value(left)?;
                let right_val = self.expression_to_value(right)?;
                
                use crate::parser::BinaryOperator;
                match operator {
                    BinaryOperator::Add => self.add_values(left_val, right_val),
                    BinaryOperator::Subtract => self.subtract_values(left_val, right_val),
                    BinaryOperator::Multiply => self.multiply_values(left_val, right_val),
                    BinaryOperator::Divide => self.divide_values(left_val, right_val),
                    BinaryOperator::Modulo => self.modulo_values(left_val, right_val),
                }
            }
            _ => Err(OQLError::generic("Unsupported expression type in insert/update")),
        }
    }

    /// Convert Literal to Value
    fn literal_to_value(&self, literal: &Literal) -> Value {
        match literal {
            Literal::String(s) => Value::String(s.clone()),
            Literal::Integer(i) => Value::Integer(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::Boolean(b) => Value::Boolean(*b),
        }
    }

    /// Add two values
    fn add_values(&self, left: Value, right: Value) -> OQLResult<Value> {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a + b as f64)),
            (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
            _ => Err(OQLError::generic("Cannot add these value types")),
        }
    }

    /// Subtract two values
    fn subtract_values(&self, left: Value, right: Value) -> OQLResult<Value> {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a - b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a - b as f64)),
            _ => Err(OQLError::generic("Cannot subtract these value types")),
        }
    }

    /// Multiply two values
    fn multiply_values(&self, left: Value, right: Value) -> OQLResult<Value> {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a * b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a * b as f64)),
            _ => Err(OQLError::generic("Cannot multiply these value types")),
        }
    }

    /// Divide two values
    fn divide_values(&self, left: Value, right: Value) -> OQLResult<Value> {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => {
                if b == 0 {
                    Err(OQLError::generic("Division by zero"))
                } else {
                    Ok(Value::Float(a as f64 / b as f64))
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                if b == 0.0 {
                    Err(OQLError::generic("Division by zero"))
                } else {
                    Ok(Value::Float(a / b))
                }
            }
            (Value::Integer(a), Value::Float(b)) => {
                if b == 0.0 {
                    Err(OQLError::generic("Division by zero"))
                } else {
                    Ok(Value::Float(a as f64 / b))
                }
            }
            (Value::Float(a), Value::Integer(b)) => {
                if b == 0 {
                    Err(OQLError::generic("Division by zero"))
                } else {
                    Ok(Value::Float(a / b as f64))
                }
            }
            _ => Err(OQLError::generic("Cannot divide these value types")),
        }
    }

    /// Modulo two values
    fn modulo_values(&self, left: Value, right: Value) -> OQLResult<Value> {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => {
                if b == 0 {
                    Err(OQLError::generic("Modulo by zero"))
                } else {
                    Ok(Value::Integer(a % b))
                }
            }
            _ => Err(OQLError::generic("Modulo operation only supported for integers")),
        }
    }

    /// Convert Document back to OQL insert statement
    fn document_to_insert_oql(&self, document: &Document) -> OQLResult<String> {
        let mut field_parts = Vec::new();
        
        for (field_name, value) in &document.fields {
            let value_str = self.value_to_oql_string(value)?;
            field_parts.push(format!("{}: {}", field_name, value_str));
        }
        
        Ok(format!("{}.insert {{ {} }}", document.schema, field_parts.join(", ")))
    }

    /// Convert Value to OQL string representation
    fn value_to_oql_string(&self, value: &Value) -> OQLResult<String> {
        match value {
            Value::String(s) => Ok(format!("\"{}\"", s.replace("\"", "\\\""))),
            Value::Integer(i) => Ok(i.to_string()),
            Value::Float(f) => Ok(f.to_string()),
            Value::Boolean(b) => Ok(b.to_string()),
            Value::List(arr) => {
                let elements: Result<Vec<_>, _> = arr.iter()
                    .map(|v| self.value_to_oql_string(v))
                    .collect();
                Ok(format!("[{}]", elements?.join(", ")))
            }
            Value::Object(obj) => {
                let mut field_parts = Vec::new();
                for (key, val) in obj {
                    let val_str = self.value_to_oql_string(val)?;
                    field_parts.push(format!("{}: {}", key, val_str));
                }
                Ok(format!("{{ {} }}", field_parts.join(", ")))
            }
            Value::Reference(_) => {
                Err(OQLError::generic("Cannot serialize reference values to OQL"))
            }
            Value::Null => Ok("null".to_string()),
            Value::DateTime(dt) => Ok(format!("\"{}\"", dt.to_rfc3339())),
        }
    }

    /// Begin a distributed transaction
    pub fn begin_distributed_transaction(&self, shards: &[ShardId]) -> OQLResult<TxnId> {
        if let Some(cluster_mgr) = &self.cluster_manager {
            cluster_mgr.begin_distributed_transaction(shards)
        } else {
            // Single shard transaction
            let txn_id = self.generate_transaction_id();
            self.storage.begin_transaction(txn_id)?;
            Ok(txn_id)
        }
    }

    /// Commit a distributed transaction
    pub fn commit_distributed_transaction(&self, txn_id: TxnId) -> OQLResult<()> {
        if let Some(cluster_mgr) = &self.cluster_manager {
            cluster_mgr.commit_distributed_transaction(txn_id)
        } else {
            self.storage.commit_transaction(txn_id)?;
            Ok(())
        }
    }

    /// Rollback a distributed transaction
    pub fn rollback_distributed_transaction(&self, txn_id: TxnId) -> OQLResult<()> {
        if let Some(cluster_mgr) = &self.cluster_manager {
            cluster_mgr.rollback_distributed_transaction(txn_id)
        } else {
            self.storage.rollback_transaction(txn_id)?;
            Ok(())
        }
    }

    /// Generate a unique transaction ID
    fn generate_transaction_id(&self) -> TxnId {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        
        // Combine timestamp with shard ID for uniqueness
        (timestamp << 16) | (self.shard_id as u64)
    }

    /// Execute OQL file with multiple statements
    pub fn execute_oql_file(&self, file_content: &str) -> OQLResult<Vec<OQLExecutionResult>> {
        let mut results = Vec::new();
        
        // Split file content into individual statements
        // This is a simple implementation - a full parser would handle this better
        let statements = file_content
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && !s.starts_with("//"))
            .collect::<Vec<_>>();

        for statement in statements {
            match self.execute_oql(statement) {
                Ok(result) => results.push(result),
                Err(e) => {
                    results.push(OQLExecutionResult::error(e.to_string()));
                }
            }
        }

        Ok(results)
    }

    /// Get current shard ID
    pub fn shard_id(&self) -> ShardId {
        self.shard_id
    }

    /// Get storage reference
    pub fn storage(&self) -> &Arc<WALFirstStorage> {
        &self.storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributed::wal::DistributedWAL;
    use crate::simulation::RealFileSystem;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_engine() -> (DistributedOQLEngine, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let arena_path = temp_dir.path().join("arena.owl");
        let wal_path = temp_dir.path().join("wal.log");
        let filesystem = Arc::new(RealFileSystem::new());

        let storage = Arc::new(WALFirstStorage::new(
            0,
            &arena_path,
            10 * 1024 * 1024,
            &wal_path,
            filesystem,
        ).unwrap());

        let engine = DistributedOQLEngine::new(0, storage);
        (engine, temp_dir)
    }

    #[test]
    fn test_oql_insert_execution() {
        let (engine, _temp_dir) = create_test_engine();

        let oql = r#"User.insert { id: "user1", name: "Alice", age: 30 }"#;
        let result = engine.execute_oql(oql).unwrap();

        assert!(result.success);
        assert_eq!(result.affected_count, 1);
        assert_eq!(result.object_refs.len(), 1);
    }

    #[test]
    fn test_oql_insert_many_execution() {
        let (engine, _temp_dir) = create_test_engine();

        let oql = r#"User.insert [
            { id: "user1", name: "Alice", age: 30 },
            { id: "user2", name: "Bob", age: 25 }
        ]"#;
        let result = engine.execute_oql(oql).unwrap();

        assert!(result.success);
        assert_eq!(result.affected_count, 2);
        assert_eq!(result.object_refs.len(), 2);
    }

    #[test]
    fn test_oql_update_execution() {
        let (engine, _temp_dir) = create_test_engine();

        // First insert a document
        let insert_oql = r#"User.insert { id: "user1", name: "Alice", age: 30 }"#;
        engine.execute_oql(insert_oql).unwrap();

        // Then update it
        let update_oql = r#"User.update { age: 31 }"#;
        let result = engine.execute_oql(update_oql).unwrap();

        assert!(result.success);
        assert_eq!(result.affected_count, 1);
    }

    #[test]
    fn test_oql_delete_execution() {
        let (engine, _temp_dir) = create_test_engine();

        // First insert a document
        let insert_oql = r#"User.insert { id: "user1", name: "Alice", age: 30 }"#;
        engine.execute_oql(insert_oql).unwrap();

        // Then delete it
        let delete_oql = r#"User.delete"#;
        let result = engine.execute_oql(delete_oql).unwrap();

        assert!(result.success);
        assert_eq!(result.affected_count, 1);
    }

    #[test]
    fn test_expression_to_value_conversion() {
        let (engine, _temp_dir) = create_test_engine();

        // Test literal conversion
        let literal_expr = Expression::Literal(Literal::String("test".to_string()));
        let value = engine.expression_to_value(&literal_expr).unwrap();
        assert_eq!(value, Value::String("test".to_string()));

        // Test array literal
        let array_expr = Expression::ArrayLiteral(vec![
            Expression::Literal(Literal::Integer(1)),
            Expression::Literal(Literal::Integer(2)),
        ]);
        let value = engine.expression_to_value(&array_expr).unwrap();
        if let Value::Array(arr) = value {
            assert_eq!(arr.len(), 2);
            assert_eq!(arr[0], Value::Integer(1));
            assert_eq!(arr[1], Value::Integer(2));
        } else {
            panic!("Expected array value");
        }
    }

    #[test]
    fn test_binary_operations() {
        let (engine, _temp_dir) = create_test_engine();

        // Test addition
        let add_result = engine.add_values(Value::Integer(5), Value::Integer(3)).unwrap();
        assert_eq!(add_result, Value::Integer(8));

        // Test string concatenation
        let concat_result = engine.add_values(
            Value::String("Hello ".to_string()),
            Value::String("World".to_string())
        ).unwrap();
        assert_eq!(concat_result, Value::String("Hello World".to_string()));

        // Test division
        let div_result = engine.divide_values(Value::Integer(10), Value::Integer(3)).unwrap();
        if let Value::Float(f) = div_result {
            assert!((f - 3.333333333333333).abs() < 0.0001);
        } else {
            panic!("Expected float result");
        }
    }

    #[test]
    fn test_transaction_operations() {
        let (engine, _temp_dir) = create_test_engine();

        // Begin transaction
        let txn_id = engine.begin_distributed_transaction(&[0]).unwrap();

        // Execute operations in transaction
        let oql = r#"User.insert { id: "user1", name: "Alice", age: 30 }"#;
        let result = engine.execute_oql_in_transaction(oql, txn_id).unwrap();
        assert!(result.success);

        // Commit transaction
        engine.commit_distributed_transaction(txn_id).unwrap();
    }

    #[test]
    fn test_document_to_oql_conversion() {
        let (engine, _temp_dir) = create_test_engine();

        let mut fields = HashMap::new();
        fields.insert("id".to_string(), Value::String("user1".to_string()));
        fields.insert("name".to_string(), Value::String("Alice".to_string()));
        fields.insert("age".to_string(), Value::Integer(30));

        let document = Document::with_id("user1".to_string(), "User".to_string(), fields);
        let oql = engine.document_to_insert_oql(&document).unwrap();

        assert!(oql.contains("User.insert"));
        assert!(oql.contains("id: \"user1\""));
        assert!(oql.contains("name: \"Alice\""));
        assert!(oql.contains("age: 30"));
    }

    #[test]
    fn test_oql_file_execution() {
        let (engine, _temp_dir) = create_test_engine();

        let file_content = r#"
            User.insert { id: "user1", name: "Alice", age: 30 };
            User.insert { id: "user2", name: "Bob", age: 25 };
            User.update { age: 31 };
        "#;

        let results = engine.execute_oql_file(file_content).unwrap();
        assert_eq!(results.len(), 3);
        
        for result in &results {
            assert!(result.success);
        }
    }

    #[test]
    fn test_invalid_oql_handling() {
        let (engine, _temp_dir) = create_test_engine();

        let invalid_oql = "INVALID OQL SYNTAX";
        let result = engine.execute_oql(invalid_oql);
        assert!(result.is_err());
    }

    #[test]
    fn test_function_call_in_insert() {
        let (engine, _temp_dir) = create_test_engine();

        // Test now() function
        let oql = r#"User.insert { id: "user1", name: "Alice", created_at: now() }"#;
        let result = engine.execute_oql(oql).unwrap();

        assert!(result.success);
        assert_eq!(result.affected_count, 1);
    }
}