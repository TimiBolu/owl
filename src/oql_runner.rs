// OQL File Runner - Execute OQL files using the database engine

use crate::{parse_oql_file, execute_oql_file, QueryExecutor};
use crate::storage::{DocumentStore, Storage, create_storage, zig_adapter::ZigStore};
use std::sync::Arc;
use std::fs;
use std::path::Path;

/// OQL file runner that can execute .oql files
pub struct OQLRunner<S: DocumentStore> {
    executor: QueryExecutor<S>,
}

impl<S: DocumentStore> OQLRunner<S> {
    /// Create a new OQL runner with the given store
    pub fn new(store: Arc<S>) -> Self {
        let executor = QueryExecutor::new_with_real_clock(store);
        Self { executor }
    }
    
    /// Execute an OQL file from a file path
    pub fn execute_file<P: AsRef<Path>>(&self, file_path: P) -> Result<(), Box<dyn std::error::Error>> {
        let content = fs::read_to_string(file_path.as_ref())?;
        self.execute_content(&content)
    }
    
    /// Execute OQL content from a string
    pub fn execute_content(&self, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        let oql_file = parse_oql_file(content)?;
        let _result = execute_oql_file(&oql_file, &self.executor)?;
        Ok(())
    }
    
    /// Get access to the underlying executor (for testing)
    pub fn executor(&self) -> &QueryExecutor<S> {
        &self.executor
    }
}

/// Create a new OQL runner with default storage (Zig)
pub fn create_oql_runner<P: AsRef<Path>>(db_path: P) -> Result<OQLRunner<ZigStore>, Box<dyn std::error::Error>> {
    let store = Arc::new(ZigStore::new(db_path.as_ref().to_str().unwrap(), 1024 * 1024 * 1024)?);
    Ok(OQLRunner::new(store))
}

/// Convenience function to execute an OQL file with temporary storage
pub fn execute_oql_file_simple<P: AsRef<Path>>(file_path: P) -> Result<(), Box<dyn std::error::Error>> {
    let runner = create_oql_runner("./temp_oql_db")?;
    let result = runner.execute_file(file_path);
    
    // Cleanup temporary database
    std::fs::remove_dir_all("./temp_oql_db").ok();
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_oql_runner_basic() {
        let temp_dir = TempDir::new().unwrap();
        let runner = create_oql_runner(temp_dir.path().join("test_db")).unwrap();
        
        let oql_content = r#"
            users.insert {
                id: "test_user",
                name: "Test User",
                age: 25
            }
            
            users.where { .id == "test_user" }
        "#;
        
        runner.execute_content(oql_content).unwrap();
    }
    
    #[test]
    fn test_oql_runner_with_schema() {
        let temp_dir = TempDir::new().unwrap();
        let runner = create_oql_runner(temp_dir.path().join("test_db")).unwrap();
        
        let oql_content = r#"
            schema users {
                id: String,
                name: String,
                age: Integer
            }
            
            users.insert {
                id: "user1",
                name: "Alice",
                age: 30
            }
        "#;
        
        runner.execute_content(oql_content).unwrap();
    }
}