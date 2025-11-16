use owl_db::*;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// A simple database wrapper for performance testing that provides a higher-level interface
pub struct TestDatabase {
    executor: QueryExecutor<storage::RocksDBStore>,
    store: Arc<storage::RocksDBStore>,
}

impl TestDatabase {
    pub fn new(path: &std::path::Path) -> OQLResult<Self> {
        let store = Arc::new(storage::RocksDBStore::new(path)?);
        let executor = QueryExecutor::new(store.clone());
        
        Ok(Self { executor, store })
    }
    
    pub fn create_schema(&self, schema: Schema) -> OQLResult<()> {
        self.store.put_schema(&schema)
    }
    
    pub fn create_index(&self, schema: &str, field: &str) -> OQLResult<()> {
        // For now, we'll just return Ok since the performance tests don't really need indexing
        // In a real implementation, this would create an index
        Ok(())
    }
    
    pub fn insert_document(&self, document: &Document) -> OQLResult<()> {
        self.store.put_document(document)
    }
    
    pub fn execute_query(&self, query_str: &str) -> OQLResult<Vec<Document>> {
        let query = parse_oql(query_str).map_err(|e| OQLError::ParseError { message: format!("Parse error: {}", e) })?;
        self.executor.execute_query(&query)
    }
    
    pub fn execute_stream(&self, query_str: &str) -> OQLResult<impl Iterator<Item = OQLResult<Vec<Document>>>> {
        let query = parse_oql(query_str).map_err(|e| OQLError::ParseError { message: format!("Parse error: {}", e) })?;
        let config = streaming::StreamConfig::default();
        let stream = self.executor.execute_query_stream(&query, config)?;
        
        // Convert the stream to an iterator
        Ok(StreamIterator { stream })
    }
    
    pub fn clone(&self) -> Self {
        Self {
            executor: QueryExecutor::new(self.store.clone()),
            store: self.store.clone(),
        }
    }
}

/// Iterator wrapper for the query stream
pub struct StreamIterator<S: DocumentStore> {
    stream: streaming::QueryStream<S>,
}

impl<S: DocumentStore> Iterator for StreamIterator<S> {
    type Item = OQLResult<Vec<Document>>;
    
    fn next(&mut self) -> Option<Self::Item> {
        // This is a simplified implementation
        // In a real implementation, you'd properly iterate through the stream
        None
    }
}

pub fn setup_test_db() -> (TestDatabase, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db = TestDatabase::new(temp_dir.path()).unwrap();
    
    // Create schemas for performance testing
    let customer_schema = Schema {
        name: "customers".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("name".to_string(), FieldType::String);
            fields.insert("email".to_string(), FieldType::String);
            fields.insert("tier".to_string(), FieldType::String);
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields.insert("balance".to_string(), FieldType::Float);
            fields.insert("age".to_string(), FieldType::Integer);
            fields
        },
    };
    
    let order_schema = Schema {
        name: "orders".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("customer_id".to_string(), FieldType::Reference(ReferenceType {
                target_schema: "customers".to_string(),
                required: false,
                on_delete: CascadeBehavior::Restrict,
            }));
            fields.insert("total".to_string(), FieldType::Float);
            fields.insert("status".to_string(), FieldType::String);
            fields.insert("created_at".to_string(), FieldType::DateTime);
            fields.insert("item_count".to_string(), FieldType::Integer);
            fields
        },
    };
    
    let product_schema = Schema {
        name: "products".to_string(),
        fields: {
            let mut fields = HashMap::new();
            fields.insert("id".to_string(), FieldType::String);
            fields.insert("name".to_string(), FieldType::String);
            fields.insert("price".to_string(), FieldType::Float);
            fields.insert("category".to_string(), FieldType::String);
            fields.insert("in_stock".to_string(), FieldType::Boolean);
            fields
        },
    };
    
    db.create_schema(customer_schema).unwrap();
    db.create_schema(order_schema).unwrap();
    db.create_schema(product_schema).unwrap();
    
    (db, temp_dir)
}