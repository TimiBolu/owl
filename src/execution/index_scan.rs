// Index Scan Operator
// Efficient index-based scanning for queries with indexed predicates

use crate::document::Document;
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::PhysicalOperatorExec;
use crate::index_manager::{IndexBackendKind, IndexManager};
use crate::planner::types::FilterCondition;
use crate::simulation::RealClock;
use crate::storage::Storage;
use std::collections::HashMap;
use std::sync::Arc;
use wyhash::wyhash;

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
    pub fn new(schema: String, index_name: String, key_conditions: Vec<FilterCondition>) -> Self {
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

    /// Create a new index scan operator with an injected index manager.
    pub fn new_with_index_manager(
        schema: String,
        index_name: String,
        key_conditions: Vec<FilterCondition>,
        index_manager: Arc<IndexManager>,
    ) -> Self {
        Self {
            schema,
            index_name,
            key_conditions,
            storage: None,
            index_manager: Some(index_manager),
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
            FilterCondition::Comparison {
                left: _,
                operator,
                right,
            } => {
                // Try to extract literal value
                if let crate::parser::Expression::Literal(lit) = right {
                    let key = Self::literal_to_index_key(lit)
                        .ok_or_else(|| OQLError::generic("Unsupported key type for index scan"))?;

                    // Map operator to range
                    use crate::parser::ComparisonOperator;
                    match operator {
                        ComparisonOperator::Equal => Ok((key, key)),
                        ComparisonOperator::LessThan if Self::literal_supports_ordering(lit) => {
                            Ok((0, key.saturating_sub(1)))
                        }
                        ComparisonOperator::LessEqual if Self::literal_supports_ordering(lit) => {
                            Ok((0, key))
                        }
                        ComparisonOperator::GreaterThan if Self::literal_supports_ordering(lit) => {
                            Ok((key.saturating_add(1), u64::MAX))
                        }
                        ComparisonOperator::GreaterEqual
                            if Self::literal_supports_ordering(lit) =>
                        {
                            Ok((key, u64::MAX))
                        }
                        _ => Err(OQLError::generic("Unsupported operator for index scan")),
                    }
                } else {
                    Err(OQLError::generic(
                        "Index scan requires literal comparison value",
                    ))
                }
            }
            _ => Err(OQLError::generic(
                "Unsupported condition type for index scan",
            )),
        }
    }

    /// Load documents using index
    fn load_documents_from_index(&mut self) -> OQLResult<()> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?;

        let index_manager = self
            .index_manager
            .as_ref()
            .ok_or_else(|| OQLError::generic("Index manager not initialized"))?;

        // Get indexes for this schema
        let indexes = index_manager
            .get_indexes(&self.schema)
            .map_err(|e| OQLError::generic(&format!("Failed to get indexes: {}", e)))?;

        // Find the index we want to use
        let index = indexes
            .iter()
            .find(|idx| idx.name == self.index_name)
            .ok_or_else(|| OQLError::generic(&format!("Index '{}' not found", self.index_name)))?;

        // Extract key range from conditions
        let (start_key, end_key) = self.extract_key_range()?;

        // Perform range query on index
        let objrefs = index
            .range_query(start_key, end_key)
            .map_err(|e| OQLError::generic(&format!("Index range query failed: {}", e)))?;

        // Build a one-pass lookup map from ObjRef offset -> Document.
        // This avoids returning wrong documents when multiple rows exist.
        let mut by_offset: HashMap<u64, Document> = HashMap::new();
        let all_docs = storage.scan_documents(&self.schema)?;
        for doc in all_docs {
            if let Some(objref) = storage.get_objref(&self.schema, &doc.id)? {
                by_offset.insert(objref.offset(), doc);
            }
        }

        // Load documents from storage using ObjRef offset lookup.
        self.documents.clear();
        for objref in objrefs {
            if let Some(doc) = by_offset.remove(&objref.offset()) {
                self.documents.push(doc);
            }
        }

        Ok(())
    }

    fn literal_to_index_key(lit: &crate::parser::Literal) -> Option<u64> {
        match lit {
            crate::parser::Literal::Integer(i) => Some(*i as u64),
            crate::parser::Literal::Float(f) => Some(f.to_bits()),
            crate::parser::Literal::Boolean(b) => Some(u64::from(*b)),
            crate::parser::Literal::String(s) => Some(wyhash(s.as_bytes(), 0)),
        }
    }

    fn literal_supports_ordering(lit: &crate::parser::Literal) -> bool {
        matches!(
            lit,
            crate::parser::Literal::Integer(_) | crate::parser::Literal::Float(_)
        )
    }
}

impl PhysicalOperatorExec for IndexScanExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage.clone());

        if self.index_manager.is_none() {
            if let Ok(catalog_path) = std::env::var("OWL_INDEX_CATALOG") {
                let loaded = IndexManager::load_catalog_with_backend(
                    catalog_path,
                    Arc::new(RealClock::new()),
                    IndexBackendKind::Zig,
                )
                .map_err(|e| OQLError::generic(&format!("Failed loading index catalog: {}", e)))?;
                self.index_manager = Some(Arc::new(loaded));
            } else {
                if let Some(zig_store) = storage
                    .as_any()
                    .downcast_ref::<crate::storage::zig_adapter::ZigStore>()
                {
                    if let Some(manager) = zig_store.scalar_index_manager()? {
                        self.index_manager = Some(manager);
                    }
                }

                if self.index_manager.is_none() {
                    let scalar_defs = storage.list_scalar_indexes(Some(&self.schema))?;
                    if scalar_defs.is_empty() {
                        // Fallback only if no injected manager, no catalog configured,
                        // and the storage backend has no scalar index metadata.
                        self.index_manager = Some(Arc::new(IndexManager::new()));
                    } else {
                        let loaded = IndexManager::load_scalar_definitions_with_backend(
                            scalar_defs,
                            Arc::new(RealClock::new()),
                            IndexBackendKind::Zig,
                        )
                        .map_err(|e| {
                            OQLError::generic(&format!(
                                "Failed loading scalar indexes from storage metadata: {}",
                                e
                            ))
                        })?;
                        loaded.rebuild_from_store(storage.as_ref()).map_err(|e| {
                            OQLError::generic(&format!(
                                "Failed rebuilding scalar indexes from storage: {}",
                                e
                            ))
                        })?;
                        self.index_manager = Some(Arc::new(loaded));
                    }
                }
            }
        }

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
    use crate::execution::operator::PhysicalOperatorCollect;
    use crate::parser::{ComparisonOperator, Expression, FieldAccess, Literal};
    use crate::simulation::RealClock;
    use crate::storage::zig_adapter::ZigStore;
    use crate::storage::DocumentStore;
    use crate::Value;
    use std::collections::HashMap;
    use tempfile::TempDir;

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
        let scan = IndexScanExec::new("users".to_string(), "idx_users_id".to_string(), vec![]);

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, u64::MAX);
    }

    #[test]
    fn test_index_scan_uses_injected_index_manager_documents_match_objref() {
        let temp = TempDir::new().expect("temp dir");
        let db_path = temp.path().join("index_scan_exec.owl");
        let store = Arc::new(
            ZigStore::new(db_path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );

        // Insert docs
        let mut d1 = HashMap::new();
        d1.insert("age".to_string(), Value::Integer(30));
        let mut d2 = HashMap::new();
        d2.insert("age".to_string(), Value::Integer(40));
        store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                d1,
            ))
            .expect("put u1");
        store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                d2,
            ))
            .expect("put u2");

        // Build index manager from same store ObjRefs.
        let manager = Arc::new(IndexManager::new_with_backend(
            Arc::new(RealClock::new()),
            IndexBackendKind::Zig,
        ));
        manager
            .create_index("users", vec!["age".to_string()], false)
            .expect("create index");

        let obj1 = store
            .get_objref("users", &"u1".to_string())
            .expect("obj1")
            .expect("objref1");
        let obj2 = store
            .get_objref("users", &"u2".to_string())
            .expect("obj2")
            .expect("objref2");
        let mut fv1 = HashMap::new();
        fv1.insert("age".to_string(), 30u64);
        let mut fv2 = HashMap::new();
        fv2.insert("age".to_string(), 40u64);
        manager
            .on_insert("users", owl_storage_zig::ObjRef::new(obj1.offset()), &fv1)
            .expect("index u1");
        manager
            .on_insert("users", owl_storage_zig::ObjRef::new(obj2.offset()), &fv2)
            .expect("index u2");

        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(30)),
        };
        let mut scan = IndexScanExec::new_with_index_manager(
            "users".to_string(),
            "idx_users_age".to_string(),
            vec![condition],
            manager,
        );
        scan.open(store.clone()).expect("open scan");
        let docs = scan.collect_all().expect("collect docs");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "u1".to_string());
    }

    #[test]
    fn test_extract_key_range_string_equality() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["profile".to_string(), "country".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("NG".to_string())),
        };

        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_profile_country".to_string(),
            vec![condition],
        );

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, end);
        assert_eq!(start, wyhash("NG".as_bytes(), 0));
    }

    #[test]
    fn test_index_scan_supports_nested_path_string_equality() {
        let temp = TempDir::new().expect("temp dir");
        let db_path = temp.path().join("index_scan_nested_path.owl");
        let store = Arc::new(
            ZigStore::new(db_path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );

        let mut profile1 = HashMap::new();
        profile1.insert("country".to_string(), Value::String("NG".to_string()));
        let mut d1 = HashMap::new();
        d1.insert("profile".to_string(), Value::Object(profile1));

        let mut profile2 = HashMap::new();
        profile2.insert("country".to_string(), Value::String("US".to_string()));
        let mut d2 = HashMap::new();
        d2.insert("profile".to_string(), Value::Object(profile2));

        store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                d1,
            ))
            .expect("put u1");
        store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                d2,
            ))
            .expect("put u2");

        let manager = Arc::new(IndexManager::new_with_backend(
            Arc::new(RealClock::new()),
            IndexBackendKind::InMemory,
        ));
        manager
            .create_index("users", vec!["profile.country".to_string()], false)
            .expect("create nested index");

        let obj1 = store
            .get_objref("users", &"u1".to_string())
            .expect("obj1")
            .expect("objref1");
        let obj2 = store
            .get_objref("users", &"u2".to_string())
            .expect("obj2")
            .expect("objref2");

        let mut fv1 = HashMap::new();
        fv1.insert("profile.country".to_string(), wyhash("NG".as_bytes(), 0));
        let mut fv2 = HashMap::new();
        fv2.insert("profile.country".to_string(), wyhash("US".as_bytes(), 0));

        manager
            .on_insert("users", owl_storage_zig::ObjRef::new(obj1.offset()), &fv1)
            .expect("index u1");
        manager
            .on_insert("users", owl_storage_zig::ObjRef::new(obj2.offset()), &fv2)
            .expect("index u2");

        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["profile".to_string(), "country".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("NG".to_string())),
        };

        let mut scan = IndexScanExec::new_with_index_manager(
            "users".to_string(),
            "idx_users_profile.country".to_string(),
            vec![condition],
            manager,
        );
        scan.open(store.clone()).expect("open scan");
        let docs = scan.collect_all().expect("collect docs");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "u1");
    }

    #[test]
    fn test_index_scan_bootstraps_from_storage_scalar_index_metadata() {
        let temp = TempDir::new().expect("temp dir");
        let db_path = temp.path().join("index_scan_store_metadata.owl");
        let store = Arc::new(
            ZigStore::new(db_path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );

        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_users_profile.country".to_string(),
                schema: "users".to_string(),
                fields: vec!["profile.country".to_string()],
                unique: false,
            })
            .expect("persist scalar index metadata");

        let mut profile1 = HashMap::new();
        profile1.insert("country".to_string(), Value::String("NG".to_string()));
        let mut d1 = HashMap::new();
        d1.insert("profile".to_string(), Value::Object(profile1));

        let mut profile2 = HashMap::new();
        profile2.insert("country".to_string(), Value::String("US".to_string()));
        let mut d2 = HashMap::new();
        d2.insert("profile".to_string(), Value::Object(profile2));

        store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                d1,
            ))
            .expect("put u1");
        store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                d2,
            ))
            .expect("put u2");

        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["profile".to_string(), "country".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("NG".to_string())),
        };

        let mut scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_profile.country".to_string(),
            vec![condition],
        );
        scan.open(store.clone()).expect("open scan");
        let docs = scan.collect_all().expect("collect docs");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "u1");
    }
}
