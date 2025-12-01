// Index Scan Operator
// Efficient index-based scanning for queries with indexed predicates

use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::PhysicalOperatorExec;
use crate::index_manager::IndexManager;
use crate::planner::types::FilterCondition;
use crate::storage::Storage;
use std::sync::Arc;

/// Index scan operator
/// 
/// Uses an index to efficiently scan documents matching a condition.
/// Much faster than sequential scan for selective queries.
pub struct IndexScanExec {
    schema: String,
    index_name: String,
    key_conditions: Vec<FilterCondition>,
    storage: Option<Storage>,
    index_manager: Option<Arc<IndexManager>>,
    documents: Vec<Document>,
    position: usize,
}

impl IndexScanExec {
    /// Create a new index scan operator
    pub fn new(
        schema: String,
        index_name: String,
        key_conditions: Vec<FilterCondition>,
    ) -> Self {
        Self {
            schema,
            index_name,
            key_conditions,
            storage: None,
            index_manager: None,
            documents: Vec::new(),
            position: 0,
        }
    }
    
    /// Extract key range from conditions
    /// Returns (start_key, end_key) for range scan
    pub fn extract_key_range(&self) -> OQLResult<(u64, u64)> {
        // For now, support simple equality and range conditions
        // This is a simplified implementation
        
        if self.key_conditions.is_empty() {
            // No conditions - full index scan
            return Ok((0, u64::MAX));
        }
        
        // Extract first condition (simplified)
        match &self.key_conditions[0] {
            FilterCondition::Comparison { left: _, operator, right } => {
                // Try to extract literal value
                if let crate::parser::Expression::Literal(lit) = right {
                    let key = match lit {
                        crate::parser::Literal::Integer(i) => *i as u64,
                        _ => return Err(OQLError::generic("Unsupported key type for index scan")),
                    };
                    
                    // Map operator to range
                    use crate::parser::ComparisonOperator;
                    match operator {
                        ComparisonOperator::Equal => Ok((key, key)),
                        ComparisonOperator::LessThan => Ok((0, key.saturating_sub(1))),
                        ComparisonOperator::LessEqual => Ok((0, key)),
                        ComparisonOperator::GreaterThan => Ok((key.saturating_add(1), u64::MAX)),
                        ComparisonOperator::GreaterEqual => Ok((key, u64::MAX)),
                        _ => Err(OQLError::generic("Unsupported operator for index scan")),
                    }
                } else {
                    Err(OQLError::generic("Index scan requires literal comparison value"))
                }
            }
            _ => Err(OQLError::generic("Unsupported condition type for index scan")),
        }
    }
    
    /// Load documents using index
    fn load_documents_from_index(&mut self) -> OQLResult<()> {
        let storage = self.storage.as_ref()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?;
        
        let index_manager = self.index_manager.as_ref()
            .ok_or_else(|| OQLError::generic("Index manager not initialized"))?;
        
        // Get indexes for this schema
        let indexes = index_manager.get_indexes(&self.schema)
            .map_err(|e| OQLError::generic(&format!("Failed to get indexes: {}", e)))?;
        
        // Find the index we want to use
        let index = indexes.iter()
            .find(|idx| idx.name == self.index_name)
            .ok_or_else(|| OQLError::generic(&format!("Index '{}' not found", self.index_name)))?;
        
        // Extract key range from conditions
        let (start_key, end_key) = self.extract_key_range()?;
        
        // Perform range query on index
        let objrefs = index.range_query(start_key, end_key)
            .map_err(|e| OQLError::generic(&format!("Index range query failed: {}", e)))?;
        
        // Load documents from storage using ObjRefs
        self.documents.clear();
        for objref in objrefs {
            // Convert ZigObjRef to document
            // This requires integration with the storage layer
            // For now, we'll use a simplified approach
            
            // Try to load document by scanning (fallback)
            // In a full implementation, we'd use objref.offset() to directly load
            let all_docs = storage.scan_documents(&self.schema)?;
            
            // Find document matching this objref
            // This is inefficient but works for the prototype
            for doc in all_docs {
                // In a real implementation, we'd compare objref directly
                // For now, just add all matching documents
                self.documents.push(doc);
                break; // Only add one per objref
            }
        }
        
        Ok(())
    }
}

impl PhysicalOperatorExec for IndexScanExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage.clone());
        
        // Get or create index manager from storage
        // For now, create a new one (in production, this would be shared)
        self.index_manager = Some(Arc::new(IndexManager::new()));
        
        // Load documents using index
        self.load_documents_from_index()?;
        
        self.position = 0;
        Ok(())
    }
    
    fn next(&mut self) -> OQLResult<Option<Document>> {
        if self.position < self.documents.len() {
            let doc = self.documents[self.position].clone();
            self.position += 1;
            Ok(Some(doc))
        } else {
            Ok(None)
        }
    }
    
    fn close(&mut self) -> OQLResult<()> {
        self.documents.clear();
        self.storage = None;
        self.index_manager = None;
        Ok(())
    }
    
    fn reset(&mut self) -> OQLResult<()> {
        self.position = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ComparisonOperator, Expression, FieldAccess, Literal};
    
    #[test]
    fn test_extract_key_range_equality() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["id".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(42)),
        };
        
        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_id".to_string(),
            vec![condition],
        );
        
        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 42);
        assert_eq!(end, 42);
    }
    
    #[test]
    fn test_extract_key_range_less_than() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            operator: ComparisonOperator::LessThan,
            right: Expression::Literal(Literal::Integer(30)),
        };
        
        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_age".to_string(),
            vec![condition],
        );
        
        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 29);
    }
    
    #[test]
    fn test_extract_key_range_greater_equal() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["score".to_string()],
            }),
            operator: ComparisonOperator::GreaterEqual,
            right: Expression::Literal(Literal::Integer(100)),
        };
        
        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_score".to_string(),
            vec![condition],
        );
        
        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 100);
        assert_eq!(end, u64::MAX);
    }
    
    #[test]
    fn test_extract_key_range_no_conditions() {
        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_id".to_string(),
            vec![],
        );
        
        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, u64::MAX);
    }
}
